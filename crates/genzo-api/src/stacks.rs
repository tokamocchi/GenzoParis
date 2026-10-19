//! スタック（LIB-12）と仮想コピー（LIB-13）の操作（docs/04_architecture.md の 3.10 節）。
//!
//! - 変えたら [`Event::CatalogChanged`]（[`CatalogChange::Stacks`]・[`CatalogChange::Variants`]）で知らせ、
//!   表示中の検索結果を作り直す（スタックでまとめた表示が変わるため。[`crate::search`]）。知らせる variant
//!   は、印（位置・写真の数・折りたたみ）の変わったスタックの写真（変える前と後の両方）。すべてのスタックの
//!   折りたたみを変えたときは写真を挙げない（`all` が真）。
//! - 選択とスタックの関係（決定事項ログ No.86）: グリッドで選べるのは表示している写真（折りたたんだスタックは
//!   代表の 1 枚）だけで、評価などの操作は表示している写真だけに付く（隠れた写真には付かない）。削除の確認で
//!   隠れた写真も含めるかは画面が選ぶ（[`Core::variant_stacks`] でスタックの写真を調べる）。
//! - 撮影時刻の間隔による自動スタック（[`Core::auto_stack`]）は、条件（[`SearchFilter`]。画面は表示中の
//!   条件）に合う写真のうち、スタックに入っていないものが対象。`dry_run` で作る数の見込みだけを返す。
//! - 仮想コピー: 名前の変更（[`Core::rename_virtual_copy`]）、マスターとの入れ替え（[`Core::set_as_master`]）。
//!   作る・削除は `develop.rs`・`delete.rs`（作った仮想コピーは元の写真と同じスタックに入る。genzo-catalog の
//!   `Catalog::create_virtual_copy`）。

use std::collections::BTreeSet;
use std::time::Duration;

use genzo_catalog::Catalog;
use genzo_model::{StackId, VariantId};

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::types::{AutoStackReport, SearchFilter, StackInfo};

/// 自動スタックの間隔の上限（秒。genzo-catalog の `MAX_AUTO_STACK_GAP`。**仮置き**: 3,600）。
pub const MAX_AUTO_STACK_GAP_SECONDS: f64 = genzo_catalog::MAX_AUTO_STACK_GAP.as_secs() as f64;

/// `ids` が入っているスタックの写真すべて（変える前の印を消すため）。
pub(crate) fn stack_members_of(
    c: &Catalog,
    ids: &[VariantId],
) -> genzo_catalog::Result<Vec<VariantId>> {
    Ok(c.stacks_of_variants(ids)?
        .into_iter()
        .flat_map(|s| s.members)
        .collect())
}

/// スタックを変えた後: 写真を知らせて、表示中の検索結果を作り直す。
pub(crate) fn after_change(
    inner: &Inner,
    change: CatalogChange,
    ids: impl IntoIterator<Item = VariantId>,
) {
    let unique: BTreeSet<VariantId> = ids.into_iter().collect();
    inner.events.emit(Event::CatalogChanged {
        change,
        variant_ids: unique.into_iter().collect(),
        all: false,
    });
    crate::search::refresh(inner, &[]);
}

impl Core {
    /// 写真をまとめて 1 つのスタックにする（Ctrl / ⌘ + G）。既にスタックに入っている写真が含まれていれば、
    /// そのスタックの写真もまとめる。順は撮影日時の順で、一番上の写真は `top`（なければ最初のもの）。作った
    /// スタックは折りたたむ。作ったスタックを返す。2 枚未満なら種類 `invalid_input`。
    pub fn create_stack(
        &self,
        variant_ids: &[VariantId],
        top: Option<VariantId>,
    ) -> Result<StackInfo, ApiError> {
        self.inner.check_open()?;
        let (before, info) = self.inner.with_catalog(|c| {
            let before = stack_members_of(c, variant_ids)?;
            let id = c.create_stack(variant_ids, top)?;
            Ok((before, c.stack(id)?))
        })?;
        after_change(
            &self.inner,
            CatalogChange::Stacks,
            before.into_iter().chain(info.members.iter().copied()),
        );
        Ok(info.into())
    }

    /// スタックに写真を足す（最後に、撮影日時の順で。ほかのスタックの写真は移す）。新しく足した数を返す。
    pub fn add_to_stack(
        &self,
        stack_id: StackId,
        variant_ids: &[VariantId],
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let (before, n, after) = self.inner.with_catalog(|c| {
            let mut ids = variant_ids.to_vec();
            ids.extend(c.stack(stack_id)?.members);
            let before = stack_members_of(c, &ids)?;
            let n = c.add_to_stack(stack_id, variant_ids)?;
            Ok((before, n, c.stack(stack_id)?.members))
        })?;
        after_change(
            &self.inner,
            CatalogChange::Stacks,
            before.into_iter().chain(after),
        );
        Ok(n)
    }

    /// 写真をスタックから外す（写真は消えない。1 枚以下になったスタックは消える）。外した数を返す。
    pub fn remove_from_stack(&self, variant_ids: &[VariantId]) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let (before, n) = self.inner.with_catalog(|c| {
            let before = stack_members_of(c, variant_ids)?;
            Ok((before, c.remove_from_stack(variant_ids)?))
        })?;
        after_change(&self.inner, CatalogChange::Stacks, before);
        Ok(n)
    }

    /// 写真が入っているスタックを解除する（Ctrl / ⌘ + Shift + G。写真は消えない）。解除したスタックの数を返す。
    pub fn unstack(&self, variant_ids: &[VariantId]) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let (before, n) = self.inner.with_catalog(|c| {
            let stacks = c.stacks_of_variants(variant_ids)?;
            let ids: Vec<StackId> = stacks.iter().map(|s| s.id).collect();
            let before: Vec<VariantId> = stacks.into_iter().flat_map(|s| s.members).collect();
            Ok((before, c.unstack(&ids)?))
        })?;
        after_change(&self.inner, CatalogChange::Stacks, before);
        Ok(n)
    }

    /// 写真をそのスタックの一番上にする（Shift + S）。スタックを返す。スタックに入っていなければ種類
    /// `invalid_input`。
    pub fn set_stack_top(&self, variant_id: VariantId) -> Result<StackInfo, ApiError> {
        self.inner.check_open()?;
        let info = self.inner.with_catalog(|c| {
            let id = c.set_stack_top(variant_id)?;
            c.stack(id)
        })?;
        after_change(&self.inner, CatalogChange::Stacks, info.members.clone());
        Ok(info.into())
    }

    /// スタックの折りたたみの状態を変える（S。`stack_ids` が `None` ならすべてのスタック）。状態の変わった
    /// スタックの数を返す。
    pub fn set_stacks_collapsed(
        &self,
        stack_ids: Option<&[StackId]>,
        collapsed: bool,
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let (n, members) = self.inner.with_catalog(|c| {
            let n = c.set_stacks_collapsed(stack_ids, collapsed)?;
            let members: Option<Vec<VariantId>> = match stack_ids {
                Some(ids) => {
                    let mut out = Vec::new();
                    for &id in ids {
                        match c.stack(id) {
                            Ok(s) => out.extend(s.members),
                            Err(genzo_catalog::CatalogError::NotFound(_)) => {}
                            Err(e) => return Err(e),
                        }
                    }
                    Some(out)
                }
                None => None,
            };
            Ok((n, members))
        })?;
        if n > 0 {
            match members {
                Some(ids) => after_change(&self.inner, CatalogChange::Stacks, ids),
                None => {
                    self.inner.events.emit(Event::CatalogChanged {
                        change: CatalogChange::Stacks,
                        variant_ids: Vec::new(),
                        all: true,
                    });
                    crate::search::refresh(&self.inner, &[]);
                }
            }
        }
        Ok(n)
    }

    /// 写真が入っているスタック（重ならないように最初に現れた順。スタックに入っていない写真は飛ばす）。
    pub fn variant_stacks(&self, variant_ids: &[VariantId]) -> Result<Vec<StackInfo>, ApiError> {
        self.inner.check_open()?;
        Ok(self
            .inner
            .with_catalog(|c| c.stacks_of_variants(variant_ids))?
            .into_iter()
            .map(StackInfo::from)
            .collect())
    }

    /// 撮影時刻の間隔で自動的にスタックを作る（LIB-12。モジュールの doc）。`gap_seconds` は 0 から
    /// [`MAX_AUTO_STACK_GAP_SECONDS`] まで（小数は 1 ミリ秒の単位に丸める）。`dry_run` なら数えるだけ。
    pub fn auto_stack(
        &self,
        filter: &SearchFilter,
        gap_seconds: f64,
        dry_run: bool,
    ) -> Result<AutoStackReport, ApiError> {
        self.inner.check_open()?;
        if !gap_seconds.is_finite() || !(0.0..=MAX_AUTO_STACK_GAP_SECONDS).contains(&gap_seconds) {
            return Err(ApiError::InvalidArgument(format!(
                "間隔は 0〜{MAX_AUTO_STACK_GAP_SECONDS} 秒で指定してください（{gap_seconds}）"
            )));
        }
        let gap = Duration::from_millis((gap_seconds * 1000.0).round() as u64);
        let r = self
            .inner
            .with_catalog(|c| c.auto_stack(&filter.to_catalog(), gap, dry_run))?;
        if !dry_run && r.stacks > 0 {
            self.inner.events.emit(Event::CatalogChanged {
                change: CatalogChange::Stacks,
                variant_ids: Vec::new(),
                all: true,
            });
            crate::search::refresh(&self.inner, &[]);
        }
        Ok(AutoStackReport {
            stacks: r.stacks,
            photos: r.photos,
            considered: r.considered,
            skipped_in_stacks: r.skipped_in_stacks,
            skipped_without_time: r.skipped_without_time,
            dry_run,
        })
    }

    /// 仮想コピーの名前を変える（LIB-13）。マスターには名前を付けられない（種類 `invalid_input`）。名前は
    /// 200 文字まで（genzo-catalog の `MAX_VARIANT_NAME_CHARS`）で、制御文字は使えない。
    pub fn rename_virtual_copy(&self, variant_id: VariantId, name: &str) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner
            .with_catalog(|c| c.rename_virtual_copy(variant_id, name))?;
        after_change(&self.inner, CatalogChange::Variants, [variant_id]);
        Ok(())
    }

    /// 仮想コピーをマスター（元の写真）にする（LIB-13。前のマスターは仮想コピーになり、名前を受け継ぐ）。
    /// 前のマスターを返す（すでにマスターなら自分）。現像設定・評価・キーワードなどは variant のまま。
    pub fn set_as_master(&self, variant_id: VariantId) -> Result<VariantId, ApiError> {
        self.inner.check_open()?;
        let previous = self.inner.with_catalog(|c| c.set_as_master(variant_id))?;
        if previous != variant_id {
            after_change(&self.inner, CatalogChange::Variants, [variant_id, previous]);
        }
        Ok(previous)
    }
}
