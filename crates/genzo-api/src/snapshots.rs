//! スナップショット（DEV-28。01 の DEV-28、04 の 3.1 節の `snapshot`）。
//!
//! 現像の途中の状態に名前を付けて、写真（variant）ごとにカタログに保存する（genzo-catalog の `snapshot` の表。
//! スキーマは版 1 からあり、変えない）。
//!
//! - **作る**（[`Core::create_snapshot`]）: 今の状態（現像中の写真なら、保存待ちの変更を先に保存する）か、履歴の
//!   1 件（[`SnapshotSource::History`]）の状態から作る。今の設定・履歴の位置は変えない。動画（現像設定がない）は
//!   作れない。
//! - **適用する**（[`Core::apply_snapshot`]）: スナップショットの写真に、保存した設定を戻す。**履歴に 1 件
//!   （「スナップショット「名前」を適用」）を記録する**ので、元に戻せる。処理バージョンも保存した版に戻す（履歴の
//!   位置へ戻すのと同じく、その時点の状態をそのまま戻す。外部データ `render_deps` は写真の今の値のまま）。現像中の
//!   写真なら、保存待ちの変更を先に保存し、戻した設定で描き直す（一括適用と同じ処理。`develop` の
//!   `apply_to_variants`）。
//! - 名前の変更・削除（[`Core::rename_snapshot`]・[`Core::delete_snapshot`]）。名前は前後の空白を除いて
//!   200 文字まで（genzo-catalog の `MAX_SNAPSHOT_NAME_CHARS`）で、制御文字は使えない。同じ名前でもよい。
//! - ビフォー / アフター（DEV-31）のビフォーには、[`crate::BeforeSource::Snapshot`]・[`crate::BeforeSource::History`]
//!   で設定する（`before` の doc）。

use genzo_model::{DevelopSettings, SnapshotId, VariantId};

use crate::core::Core;
use crate::error::ApiError;
use crate::types::{SnapshotInfo, SnapshotSource};

/// スナップショットを適用したときの履歴の名前（「スナップショット「明るめ」を適用」。genzo-catalog の
/// `Catalog::apply_snapshot` と同じ形）。
pub fn apply_snapshot_label(name: &str) -> String {
    format!("スナップショット「{name}」を適用")
}

impl Core {
    /// 写真のスナップショットの一覧（作った順。DEV-28）。
    pub fn snapshots(&self, variant_id: VariantId) -> Result<Vec<SnapshotInfo>, ApiError> {
        self.inner.check_open()?;
        Ok(self
            .inner
            .with_catalog(|c| c.snapshots(variant_id))?
            .into_iter()
            .map(SnapshotInfo::from)
            .collect())
    }

    /// スナップショットの 1 件（ない ID は NotFound）。
    pub fn snapshot(&self, snapshot_id: SnapshotId) -> Result<SnapshotInfo, ApiError> {
        self.inner.check_open()?;
        Ok(self.inner.with_catalog(|c| c.snapshot(snapshot_id))?.into())
    }

    /// スナップショットを作る（DEV-28。モジュールの doc）。作ったものを返す。
    ///
    /// - [`SnapshotSource::Current`] のとき: 今の状態。現像中の写真なら、ドラッグを終えたものとして保存待ちの変更を
    ///   先に保存する（保存した後の設定を写す）。
    /// - [`SnapshotSource::History`] のとき: 履歴の 1 件の状態（その写真の履歴になければ [`ApiError::Catalog`] の
    ///   NotFound）。
    ///
    /// 動画の variant は [`ApiError::InvalidArgument`]。名前が規則に合わなければ InvalidInput。
    pub fn create_snapshot(
        &self,
        variant_id: VariantId,
        name: &str,
        source: SnapshotSource,
    ) -> Result<SnapshotInfo, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        // ロックの順序は state → カタログ（保存と同じ）。今の状態から作るときは、保存してから同じロックの中で写す
        // （間に画面の変更が入って、写したものと保存したものが食い違わないように）。
        let mut state = inner.develop.state.lock();
        if source == SnapshotSource::Current {
            crate::develop::save_session_of(inner, &mut state, variant_id)?;
        }
        let snapshot = inner.with_catalog_api(|c| {
            crate::develop::reject_videos(c, &[variant_id])?;
            let id = match source {
                SnapshotSource::Current => c.create_snapshot(variant_id, name)?,
                SnapshotSource::History { entry_id } => {
                    c.create_snapshot_from_history(variant_id, entry_id, name)?
                }
            };
            Ok(c.snapshot(id)?)
        })?;
        drop(state);
        Ok(snapshot.into())
    }

    /// スナップショットの名前を変える（DEV-28）。変えた後のものを返す。
    pub fn rename_snapshot(
        &self,
        snapshot_id: SnapshotId,
        name: &str,
    ) -> Result<SnapshotInfo, ApiError> {
        self.inner.check_open()?;
        let s = self.inner.with_catalog(|c| {
            c.rename_snapshot(snapshot_id, name)?;
            c.snapshot(snapshot_id)
        })?;
        Ok(s.into())
    }

    /// スナップショットを削除する（DEV-28。写真の設定・履歴は変えない。ビフォーに設定していても、表示中の
    /// ビフォーはそのまま）。
    pub fn delete_snapshot(&self, snapshot_id: SnapshotId) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner.with_catalog(|c| c.delete_snapshot(snapshot_id))
    }

    /// スナップショットの設定を、そのスナップショットの写真に戻す（DEV-28。モジュールの doc）。履歴に
    /// 「スナップショット「名前」を適用」を 1 件記録し、戻した後の設定を返す。
    pub fn apply_snapshot(&self, snapshot_id: SnapshotId) -> Result<DevelopSettings, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let (snapshot, settings) = inner.with_catalog(|c| {
            let s = c.snapshot(snapshot_id)?;
            let settings = c.snapshot_settings(snapshot_id)?;
            Ok((s, settings))
        })?;
        let label = apply_snapshot_label(&snapshot.name);
        self.apply_to_variants(&[snapshot.variant_id], false, |_, _| {
            Ok(Some((settings.clone(), label.clone())))
        })?;
        // 外部データ（render_deps）は写真の今の値にして保存しているので、保存した設定を読み直して返す。
        self.develop_settings(snapshot.variant_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(
            apply_snapshot_label("明るめ"),
            "スナップショット「明るめ」を適用"
        );
    }
}
