//! コマンド（UI → コア。04 の 1.5 節）と応答。
//!
//! 案 A（Tauri）では JSON のコマンドとして受け取り、[`Core::execute`] に渡して応答を JSON で返す。
//! 案 B と CLI は、同じ機能を [`Core`] の型付きのメソッドで直接呼べる。
//!
//! JSON では `{"type": "set_rating", "variant_ids": [1, 2], "rating": 3}` のように、`type` で種類を表す。
//! 応答も `{"type": "updated", "count": 2}` の形。失敗は [`ApiError`]（UI には [`crate::ErrorInfo`]）。
//!
//! サムネイルのバイト列（[`Response::Thumbnail`]）は JSON では数値の配列になり大きいので、UI は
//! カスタム URI スキーム（`genzo://thumb/{variant_id}?rev={rev}`）の処理で [`Core::thumbnail`] を直接
//! 呼ぶこと。現像のプレビューのピクセルも、[`Core::develop_frame`] で受け取ってビューポートに直接描く。

use std::path::PathBuf;
use std::time::Duration;

use genzo_model::{ColorLabel, DevelopSettings, ExportSettings, Flag, Rating, VariantId};
use serde::{Deserialize, Serialize};

use crate::core::Core;
use crate::error::ApiError;
use crate::maintenance::RestorePlan;
use crate::types::{
    BackupEntry, Capabilities, CatalogStats, CoreSettings, DeleteKind, DeletePlan, DeleteReport,
    DevelopState, DisplayInfo, FileCheckReport, FolderInfo, FrameInfo, HistoryItem, IndexOf,
    JobInfo, RangeResult, SearchFilter, SearchResult, SearchSort, SettingGroups, SettingsUpdate,
    StartupReport, ThumbnailData, VariantDetails,
};

/// コマンド（UI → コア）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    // --- ジョブ ---
    /// フォルダを取り込む（P3 のジョブ）。→ [`Response::JobStarted`]
    ImportFolder {
        /// フォルダ。
        path: PathBuf,
        /// 配下のフォルダも取り込むか。
        recursive: bool,
    },
    /// ジョブの情報。→ [`Response::Job`]
    Job {
        /// ジョブ。
        job_id: u64,
    },
    /// ジョブが終わるまで待つ（`timeout_ms` を過ぎたら、その時点の情報）。→ [`Response::Job`]
    WaitJob {
        /// ジョブ。
        job_id: u64,
        /// 待つ時間の上限（ミリ秒。`None` なら終わるまで）。
        timeout_ms: Option<u64>,
    },
    /// ジョブを取り消す。→ [`Response::Ok`]
    CancelJob {
        /// ジョブ。
        job_id: u64,
    },

    // --- 検索・選別 ---
    /// 検索する。→ [`Response::Search`]
    Search {
        /// 条件。
        #[serde(default)]
        filter: SearchFilter,
        /// 並べ替え。
        #[serde(default)]
        sort: SearchSort,
    },
    /// 表示範囲の詳細。→ [`Response::Range`]
    Range {
        /// 世代。
        generation: u64,
        /// 先頭の位置。
        start: u64,
        /// 件数。
        len: u64,
    },
    /// variant の位置。→ [`Response::IndexOf`]
    IndexOf {
        /// variant。
        variant_id: VariantId,
    },
    /// 評価を変える。→ [`Response::Updated`]
    SetRating {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// 評価（0〜5）。
        rating: Rating,
    },
    /// フラグを変える。→ [`Response::Updated`]
    SetFlag {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// フラグ（-1 / 0 / 1）。
        flag: Flag,
    },
    /// カラーラベルを変える。→ [`Response::Updated`]
    SetColorLabel {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// ラベル（`null` でラベルなし）。
        label: Option<ColorLabel>,
    },
    /// キャプションを変える。→ [`Response::Updated`]
    SetCaption {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// キャプション（`null` で削除）。
        caption: Option<String>,
    },
    /// variant の詳細。→ [`Response::Details`]
    VariantDetails {
        /// variant。
        variant_id: VariantId,
    },
    /// フォルダの一覧。→ [`Response::Folders`]
    Folders,
    /// ファイルの状態を確かめる。→ [`Response::FileCheck`]
    CheckFiles {
        /// variant。
        variant_ids: Vec<VariantId>,
    },

    // --- サムネイル・プレビュー ---
    /// サムネイル（L0）。→ [`Response::Thumbnail`]
    Thumbnail {
        /// variant。
        variant_id: VariantId,
    },
    /// 標準プレビュー（L1）のファイル。→ [`Response::Path`]
    PreviewPath {
        /// variant。
        variant_id: VariantId,
    },
    /// サムネイル・プレビューを作り直す（P3 のジョブ）。→ [`Response::JobStarted`]
    RegeneratePreviews {
        /// variant。
        variant_ids: Vec<VariantId>,
    },

    // --- 現像 ---
    /// 写真を現像のために開く。→ [`Response::Develop`]
    OpenDevelop {
        /// variant。
        variant_id: VariantId,
    },
    /// 現像中の写真の状態。→ [`Response::DevelopOrNone`]
    DevelopState,
    /// 現像設定を変える（描画を投入して、すぐに返る）。→ [`Response::Generation`]
    SetSettings {
        /// 設定。
        settings: Box<DevelopSettings>,
        /// スライダーのドラッグ中か。
        dragging: bool,
    },
    /// ドラッグを終える（履歴に 1 件記録して保存）。→ [`Response::Develop`]
    EndDrag,
    /// 現在の設定で同期で描く。→ [`Response::Frame`]
    RenderDevelop,
    /// 世代 `generation` 以降のプレビューを待つ。→ [`Response::FrameOrNone`]
    WaitForFrame {
        /// 世代。
        generation: u64,
        /// 待つ時間の上限（ミリ秒）。
        timeout_ms: u64,
    },
    /// 保存待ちの変更をすぐに保存する。→ [`Response::Saved`]
    FlushDevelop,
    /// 現像中の写真を閉じる。→ [`Response::Ok`]
    CloseDevelop,
    /// 取り消す（Undo）。→ [`Response::SettingsOrNone`]
    Undo {
        /// variant。
        variant_id: VariantId,
    },
    /// やり直す（Redo）。→ [`Response::SettingsOrNone`]
    Redo {
        /// variant。
        variant_id: VariantId,
    },
    /// 履歴。→ [`Response::History`]
    History {
        /// variant。
        variant_id: VariantId,
    },
    /// 現像設定（コピー）。→ [`Response::Settings`]
    CopySettings {
        /// variant。
        variant_id: VariantId,
    },
    /// 設定を複数の variant に適用する（DEV-30）。→ [`Response::Updated`]
    PasteSettings {
        /// 設定。
        settings: Box<DevelopSettings>,
        /// 適用先。
        targets: Vec<VariantId>,
        /// 写す項目（省略時はすべて）。
        #[serde(default)]
        groups: SettingGroups,
    },
    /// 仮想コピーを作る。→ [`Response::VariantCreated`]
    CreateVirtualCopy {
        /// 元の variant。
        variant_id: VariantId,
        /// 名前（省略時は「コピー n」）。
        name: Option<String>,
    },
    /// 画面の ICC プロファイルを設定する。→ [`Response::Display`]
    SetDisplayProfile {
        /// ICC プロファイルのバイト列（`null` なら取得できないとして sRGB とみなす）。
        icc: Option<Vec<u8>>,
    },

    // --- 書き出し ---
    /// 書き出す（P3 のジョブ）。→ [`Response::JobStarted`]
    Export {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// 書き出しの設定。
        settings: ExportSettings,
        /// 書き出し先のフォルダ。
        dest_dir: PathBuf,
    },

    // --- 削除 ---
    /// 削除の計画（影響の一覧）。→ [`Response::DeletePlan`]
    PlanDelete {
        /// 種類。
        kind: DeleteKind,
        /// variant。
        variant_ids: Vec<VariantId>,
    },
    /// 確認した削除を実行する。→ [`Response::Deleted`]
    ExecuteDelete {
        /// 計画の ID。
        plan_id: u64,
    },

    // --- 設定・保守 ---
    /// 設定。→ [`Response::CoreSettings`]
    GetSettings,
    /// 設定を変える。→ [`Response::CoreSettings`]
    UpdateSettings {
        /// 変更。
        update: SettingsUpdate,
    },
    /// 手動のバックアップ。→ [`Response::Backup`]
    BackupNow,
    /// バックアップの一覧。→ [`Response::Backups`]
    ListBackups,
    /// 詳細チェック（P3 のジョブ）。→ [`Response::JobStarted`]
    CheckIntegrity,
    /// キャッシュの回収（P3 のジョブ）。→ [`Response::JobStarted`]
    CollectCacheGarbage,
    /// バックアップからの復元の準備（別のファイルに復元し、差し替えの手順を返す）。→ [`Response::Restore`]
    PrepareRestore {
        /// バックアップ。
        backup: PathBuf,
    },
    /// 起動時の確認の結果。→ [`Response::Startup`]
    StartupReport,
    /// カタログの件数とスキーマの版。→ [`Response::CatalogStats`]
    CatalogStats,
    /// 機能の有無。→ [`Response::Capabilities`]
    Capabilities,
}

/// 応答（[`Command`] ごとの結果）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// 成功した（返す値がない）。
    Ok,
    /// ジョブを投入した。
    JobStarted {
        /// ジョブ。
        job_id: u64,
    },
    /// ジョブの情報。
    Job(JobInfo),
    /// 検索の結果。
    Search(SearchResult),
    /// 表示範囲の詳細。
    Range(RangeResult),
    /// variant の位置。
    IndexOf(IndexOf),
    /// 変更した数。
    Updated {
        /// 数。
        count: u64,
    },
    /// variant の詳細。
    Details(Box<VariantDetails>),
    /// フォルダの一覧。
    Folders {
        /// フォルダ。
        folders: Vec<FolderInfo>,
    },
    /// ファイルの状態の確認の結果。
    FileCheck(FileCheckReport),
    /// サムネイル（なければ `null`）。
    Thumbnail {
        /// サムネイル。
        thumbnail: Option<ThumbnailData>,
    },
    /// ファイル（なければ `null`）。
    Path {
        /// パス。
        path: Option<PathBuf>,
    },
    /// 現像中の写真の状態。
    Develop(Box<DevelopState>),
    /// 現像中の写真の状態（開いていなければ `null`）。
    DevelopOrNone {
        /// 状態。
        state: Option<Box<DevelopState>>,
    },
    /// 設定の世代。
    Generation {
        /// 世代。
        generation: u64,
    },
    /// プレビューの情報。
    Frame(Box<FrameInfo>),
    /// プレビューの情報（まだなければ `null`）。
    FrameOrNone {
        /// 情報。
        frame: Option<Box<FrameInfo>>,
    },
    /// 保存したか。
    Saved {
        /// 保存したか。
        saved: bool,
    },
    /// 現像設定。
    Settings {
        /// 設定。
        settings: Box<DevelopSettings>,
    },
    /// 現像設定（なければ `null`。Undo / Redo できなかった）。
    SettingsOrNone {
        /// 設定。
        settings: Option<Box<DevelopSettings>>,
    },
    /// 履歴。
    History {
        /// 履歴（古い順）。
        entries: Vec<HistoryItem>,
    },
    /// 作った variant。
    VariantCreated {
        /// variant。
        variant_id: VariantId,
    },
    /// 画面のプロファイルの情報。
    Display(DisplayInfo),
    /// 削除の計画。
    DeletePlan(DeletePlan),
    /// 削除の結果。
    Deleted(DeleteReport),
    /// 設定。
    CoreSettings(CoreSettings),
    /// バックアップ。
    Backup(BackupEntry),
    /// バックアップの一覧。
    Backups {
        /// バックアップ（新しい順）。
        backups: Vec<BackupEntry>,
    },
    /// 復元の計画。
    Restore(RestorePlan),
    /// 起動時の確認の結果。
    Startup(StartupReport),
    /// カタログの件数とスキーマの版。
    CatalogStats(CatalogStats),
    /// 機能の有無。
    Capabilities(Capabilities),
}

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

impl Core {
    /// コマンドを実行する（04 の 1.5 節）。ブロックしうるので、Tauri からは `spawn_blocking` で呼ぶ。
    pub fn execute(&self, command: Command) -> Result<Response, ApiError> {
        use Command as C;
        use Response as R;
        Ok(match command {
            C::ImportFolder { path, recursive } => R::JobStarted {
                job_id: self.import_folder(&path, recursive)?,
            },
            C::Job { job_id } => R::Job(self.job(job_id)?),
            C::WaitJob { job_id, timeout_ms } => R::Job(self.wait_job(job_id, timeout_ms.map(ms))?),
            C::CancelJob { job_id } => {
                self.cancel_job(job_id)?;
                R::Ok
            }
            C::Search { filter, sort } => R::Search(self.search(&filter, sort)?),
            C::Range {
                generation,
                start,
                len,
            } => R::Range(self.range(generation, start, len)?),
            C::IndexOf { variant_id } => R::IndexOf(self.index_of(variant_id)?),
            C::SetRating {
                variant_ids,
                rating,
            } => R::Updated {
                count: self.set_rating(&variant_ids, rating)? as u64,
            },
            C::SetFlag { variant_ids, flag } => R::Updated {
                count: self.set_flag(&variant_ids, flag)? as u64,
            },
            C::SetColorLabel { variant_ids, label } => R::Updated {
                count: self.set_color_label(&variant_ids, label)? as u64,
            },
            C::SetCaption {
                variant_ids,
                caption,
            } => R::Updated {
                count: self.set_caption(&variant_ids, caption.as_deref())? as u64,
            },
            C::VariantDetails { variant_id } => {
                R::Details(Box::new(self.variant_details(variant_id)?))
            }
            C::Folders => R::Folders {
                folders: self.folders()?,
            },
            C::CheckFiles { variant_ids } => R::FileCheck(self.check_files(&variant_ids)?),
            C::Thumbnail { variant_id } => R::Thumbnail {
                thumbnail: self.thumbnail(variant_id)?,
            },
            C::PreviewPath { variant_id } => R::Path {
                path: self.preview_path(variant_id)?,
            },
            C::RegeneratePreviews { variant_ids } => R::JobStarted {
                job_id: self.regenerate_previews(&variant_ids)?,
            },
            C::OpenDevelop { variant_id } => R::Develop(Box::new(self.open_develop(variant_id)?)),
            C::DevelopState => R::DevelopOrNone {
                state: self.develop_state()?.map(Box::new),
            },
            C::SetSettings { settings, dragging } => R::Generation {
                generation: self.set_settings(&settings, dragging)?,
            },
            C::EndDrag => R::Develop(Box::new(self.end_drag()?)),
            C::RenderDevelop => R::Frame(Box::new(self.render_develop()?)),
            C::WaitForFrame {
                generation,
                timeout_ms,
            } => R::FrameOrNone {
                frame: self
                    .wait_for_frame(generation, ms(timeout_ms))?
                    .map(Box::new),
            },
            C::FlushDevelop => R::Saved {
                saved: self.flush_develop()?,
            },
            C::CloseDevelop => {
                self.close_develop()?;
                R::Ok
            }
            C::Undo { variant_id } => R::SettingsOrNone {
                settings: self.undo(variant_id)?.map(Box::new),
            },
            C::Redo { variant_id } => R::SettingsOrNone {
                settings: self.redo(variant_id)?.map(Box::new),
            },
            C::History { variant_id } => R::History {
                entries: self.history(variant_id)?,
            },
            C::CopySettings { variant_id } => R::Settings {
                settings: Box::new(self.develop_settings(variant_id)?),
            },
            C::PasteSettings {
                settings,
                targets,
                groups,
            } => R::Updated {
                count: self.paste_settings(&settings, &targets, groups)? as u64,
            },
            C::CreateVirtualCopy { variant_id, name } => R::VariantCreated {
                variant_id: self.create_virtual_copy(variant_id, name.as_deref())?,
            },
            C::SetDisplayProfile { icc } => R::Display(self.set_display_profile(icc.as_deref())?),
            C::Export {
                variant_ids,
                settings,
                dest_dir,
            } => R::JobStarted {
                job_id: self.export(&variant_ids, &settings, &dest_dir)?,
            },
            C::PlanDelete { kind, variant_ids } => {
                R::DeletePlan(self.plan_delete(kind, &variant_ids)?)
            }
            C::ExecuteDelete { plan_id } => R::Deleted(self.execute_delete(plan_id)?),
            C::GetSettings => R::CoreSettings(self.settings()),
            C::UpdateSettings { update } => R::CoreSettings(self.update_settings(&update)?),
            C::BackupNow => R::Backup(self.backup_now()?),
            C::ListBackups => R::Backups {
                backups: self.list_backups()?,
            },
            C::CheckIntegrity => R::JobStarted {
                job_id: self.check_integrity()?,
            },
            C::CollectCacheGarbage => R::JobStarted {
                job_id: self.collect_cache_garbage()?,
            },
            C::PrepareRestore { backup } => R::Restore(crate::maintenance::prepare_restore(
                &backup,
                &self.inner.config.catalog_path,
            )?),
            C::StartupReport => R::Startup(self.startup_report()),
            C::CatalogStats => R::CatalogStats(self.catalog_stats()?),
            C::Capabilities => R::Capabilities(self.capabilities()?),
        })
    }

    /// JSON のコマンドを実行し、JSON の結果を返す（案 A の Tauri のコマンドの入口の例）。
    ///
    /// 成功は `{"ok": <Response>}`、失敗は `{"error": <ErrorInfo>}`。
    pub fn execute_json(&self, command_json: &str) -> String {
        let result = serde_json::from_str::<Command>(command_json)
            .map_err(|e| ApiError::InvalidArgument(format!("コマンドを解釈できません: {e}")))
            .and_then(|c| self.execute(c));
        let value = match result {
            Ok(r) => serde_json::json!({ "ok": r }),
            Err(e) => serde_json::json!({ "error": e.info() }),
        };
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_have_plain_json_forms() {
        let c: Command =
            serde_json::from_str(r#"{"type":"set_rating","variant_ids":[1,2],"rating":3}"#)
                .unwrap();
        assert_eq!(
            c,
            Command::SetRating {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                rating: Rating::new(3).unwrap(),
            }
        );
        let c: Command = serde_json::from_str(r#"{"type":"search"}"#).unwrap();
        assert_eq!(
            c,
            Command::Search {
                filter: SearchFilter::default(),
                sort: SearchSort::default()
            }
        );
        let c: Command = serde_json::from_str(
            r#"{"type":"search","filter":{"rating_min":3,"kind":"video","text":"京都"},"sort":{"key":"capture_time","direction":"descending"}}"#,
        )
        .unwrap();
        let Command::Search { filter, .. } = c else {
            panic!()
        };
        assert_eq!(filter.rating_min, Rating::new(3));
        assert_eq!(filter.kind, Some(genzo_model::AssetKind::Video));
        let json = serde_json::to_value(Response::Updated { count: 2 }).unwrap();
        assert_eq!(json, serde_json::json!({"type": "updated", "count": 2}));
        let json = serde_json::to_value(Response::Search(SearchResult {
            generation: 3,
            count: 10,
        }))
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "search", "generation": 3, "count": 10})
        );
        // 不正な評価は解釈の段階で弾く。
        assert!(
            serde_json::from_str::<Command>(
                r#"{"type":"set_rating","variant_ids":[1],"rating":6}"#
            )
            .is_err()
        );
    }

    #[test]
    fn every_command_round_trips() {
        let commands = vec![
            Command::ImportFolder {
                path: "/p".into(),
                recursive: true,
            },
            Command::Range {
                generation: 1,
                start: 0,
                len: 10,
            },
            Command::SetSettings {
                settings: Box::default(),
                dragging: true,
            },
            Command::PasteSettings {
                settings: Box::default(),
                targets: vec![VariantId::new(1)],
                groups: SettingGroups::default(),
            },
            Command::Export {
                variant_ids: vec![VariantId::new(1)],
                settings: ExportSettings::default(),
                dest_dir: "/out".into(),
            },
            Command::PlanDelete {
                kind: DeleteKind::Trash,
                variant_ids: vec![VariantId::new(2)],
            },
            Command::UpdateSettings {
                update: SettingsUpdate {
                    default_utc_offset_minutes: Some(540),
                    ..Default::default()
                },
            },
            Command::Folders,
            Command::Capabilities,
            Command::CatalogStats,
        ];
        for c in commands {
            let json = serde_json::to_string(&c).unwrap();
            let back: Command = serde_json::from_str(&json).unwrap();
            assert_eq!(back, c, "{json}");
        }
    }
}
