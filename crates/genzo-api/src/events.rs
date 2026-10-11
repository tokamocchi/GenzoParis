//! イベント（コア → UI。04 の 1.5 節）と、その配信。
//!
//! [`crate::Core::subscribe`] で受け取り側（`crossbeam_channel::Receiver<Event>`）を作る。案 A（Tauri）
//! ではイベントを受け取るスレッドが Tauri のイベントとして送り、案 B では UI のスレッドが直接受け取る。
//! チャネルには上限がないので、受け取り側を作ったら受け取り続けること（drop すると購読をやめる）。
//!
//! 起動時の警告（前回の異常終了など。DATA-05）は、後から購読した受け取り側にも最初に送る
//! （起動の処理は購読より前に終わるため）。

use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender};
use genzo_model::{RenderQuality, VariantId};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::types::{JobKind, JobState, RenderBackend};

/// コアから UI へのイベント。
///
/// JSON では `{"type": "job_progress", "job_id": 3, ...}` のように、`type` で種類を表す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// ジョブの進捗（P3 の取り込み・書き出しなど。SYS-02）。
    JobProgress {
        /// ジョブ。
        job_id: u64,
        /// 種類。
        kind: JobKind,
        /// 済んだ数（ファイルの数など）。
        done: u64,
        /// 全体の数（分からなければ 0）。
        total: u64,
    },
    /// ジョブが終わった（成功・失敗・取り消し）。結果は [`crate::Core::job`] で取れる。
    JobFinished {
        /// ジョブ。
        job_id: u64,
        /// 種類。
        kind: JobKind,
        /// 終わった状態。
        state: JobState,
        /// 失敗の説明（失敗した場合）。
        message: Option<String>,
    },
    /// カタログが変更された（グリッドの表示を更新する）。
    CatalogChanged {
        /// 変更の種類。
        change: CatalogChange,
        /// 影響を受けた variant（多い場合は空で、`all` が真）。
        variant_ids: Vec<VariantId>,
        /// 影響が広く、表示中のものをすべて読み直すべきか。
        all: bool,
    },
    /// 現像中の写真のプレビューが更新された（ピクセルは [`crate::Core::develop_frame`] で取る）。
    PreviewUpdated {
        /// variant。
        variant_id: VariantId,
        /// 設定の世代（[`crate::Core::set_settings`] が返した値）。
        generation: u64,
        /// 品質（WB のドラッグ中は簡易）。
        quality: RenderQuality,
        /// 幅。
        width: u32,
        /// 高さ。
        height: u32,
        /// 処理した側。
        backend: RenderBackend,
    },
    /// サムネイル（L0）・標準プレビュー（L1）が更新された（`genzo://thumb/{variant_id}?rev={rev}`）。
    ThumbnailUpdated {
        /// variant。
        variant_id: VariantId,
        /// L0 のキャッシュキーのハッシュ（URI の `rev`）。
        rev: String,
    },
    /// 検索結果が新しい世代になった（3.7 節。UI は新しい世代で範囲を要求し直し、選択中の variant は
    /// [`crate::Core::index_of`] で位置を求め直す）。
    SearchUpdated {
        /// 新しい世代。
        generation: u64,
        /// 件数。
        count: u64,
    },
    /// 警告（GPU から CPU への切り替え、前回の異常終了など。6.3 節）。
    Warning {
        /// 種類。
        code: WarningCode,
        /// 表示用の説明。
        message: String,
        /// 関係する variant（あれば）。
        variant_id: Option<VariantId>,
        /// 関係するファイル（あれば）。
        path: Option<PathBuf>,
    },
}

/// カタログの変更の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogChange {
    /// 取り込み（登録・更新）。
    Imported,
    /// 評価・フラグ・カラーラベル（LIB-04）。
    Marks,
    /// キャプション。
    Caption,
    /// 現像設定（保存・Undo / Redo・一括適用）。
    Develop,
    /// variant の追加（仮想コピー）。
    VariantAdded,
    /// 削除（仮想コピーの削除・カタログからの除去・ゴミ箱への移動）。
    Removed,
    /// ファイルの状態（missing / error / ok）。
    FileStatus,
    /// 撮影日時（既定のタイムゾーンの変更など）。
    CaptureTime,
}

/// 警告の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    /// 前回は正常に終了しなかった（DATA-05。詳細チェックを案内する）。
    UncleanShutdown,
    /// カタログのスキーマを移行した（移行の前のバックアップがある。DATA-08）。
    CatalogMigrated,
    /// 終わっていないファイル操作を確定した（DATA-07）。
    FileOperationRecovered,
    /// 自動バックアップに失敗した（DATA-04）。
    BackupFailed,
    /// GPU の処理に失敗したので CPU 版で処理した（6.3 節）。
    GpuFallback,
    /// GPU の失敗が続いたので、このセッションでは GPU を使わない。
    GpuDisabled,
    /// モニターのプロファイルが使えないので sRGB とみなした（IQ-05）。
    DisplayProfileAssumedSrgb,
    /// 現像設定の保存に失敗した（DATA-03）。
    SaveFailed,
    /// サムネイル・プレビューを作れなかった。
    PreviewFailed,
    /// 現像の警告（カメラ行列がない、v1 の項目を適用しないなど）。
    Render,
    /// 元ファイルが見つからない（3.3 節）。
    FileMissing,
    /// カタログに保存された設定の値を読めないので、既定値を使った（SYS-05）。
    InvalidSetting,
    /// 既定のタイムゾーン（カタログに保存した値）が、今の OS のオフセットと違う（自動では変えない）。
    DefaultTimeZoneDiffers,
    /// 作り直せるキャッシュ（サムネイル DB・プレビューのフォルダ）を使えないため、退避して作り直した、
    /// または既定の場所に切り替えた（DATA-03b）。
    CacheRebuilt,
    /// カタログのボリュームでファイルロックが使えないため、同時に開くことを防げない。
    CatalogLockUnavailable,
    /// 現像設定に記録したカメラ行列・RAW デコーダ（render_deps）が、今の値と違う（LibRaw を更新したなど。
    /// 同じ設定でも色が変わりうる）。
    RenderDepsChanged,
}

/// イベントの購読者の一覧。
#[derive(Default)]
pub(crate) struct EventHub {
    subscribers: Mutex<Vec<Sender<Event>>>,
    /// 後から購読した受け取り側にも送る、起動時の警告。
    sticky: Mutex<Vec<Event>>,
}

impl EventHub {
    /// 新しい受け取り側を作る（起動時の警告を先に入れる）。
    pub(crate) fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = crossbeam_channel::unbounded();
        for e in self.sticky.lock().iter() {
            let _ = tx.send(e.clone());
        }
        self.subscribers.lock().push(tx);
        rx
    }

    /// すべての受け取り側に送る（受け取り側が drop されたものは外す）。
    pub(crate) fn emit(&self, event: Event) {
        let mut subs = self.subscribers.lock();
        subs.retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// 起動時の警告として記録し、送る。
    pub(crate) fn emit_sticky(&self, event: Event) {
        self.sticky.lock().push(event.clone());
        self.emit(event);
    }

    /// 起動時の警告の一覧。
    pub(crate) fn sticky(&self) -> Vec<Event> {
        self.sticky.lock().clone()
    }

    /// 警告を送る。
    pub(crate) fn warn(
        &self,
        code: WarningCode,
        message: impl Into<String>,
        variant_id: Option<VariantId>,
        path: Option<PathBuf>,
    ) {
        let message = message.into();
        tracing::warn!(?code, %message, "警告");
        self.emit(Event::Warning {
            code,
            message,
            variant_id,
            path,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_events_are_replayed_to_late_subscribers() {
        let hub = EventHub::default();
        hub.emit(Event::SearchUpdated {
            generation: 1,
            count: 0,
        });
        hub.emit_sticky(Event::Warning {
            code: WarningCode::UncleanShutdown,
            message: "x".into(),
            variant_id: None,
            path: None,
        });
        let rx = hub.subscribe();
        let first = rx.try_recv().unwrap();
        assert!(matches!(
            first,
            Event::Warning {
                code: WarningCode::UncleanShutdown,
                ..
            }
        ));
        assert!(rx.try_recv().is_err(), "購読の前の通常のイベントは届かない");
        hub.emit(Event::SearchUpdated {
            generation: 2,
            count: 3,
        });
        assert_eq!(
            rx.try_recv().unwrap(),
            Event::SearchUpdated {
                generation: 2,
                count: 3
            }
        );
        drop(rx);
        hub.emit(Event::SearchUpdated {
            generation: 3,
            count: 0,
        });
        assert!(hub.subscribers.lock().is_empty(), "drop した購読者は外す");
        // JSON の形。
        let json = serde_json::to_value(&hub.sticky()[0]).unwrap();
        assert_eq!(json["type"], "warning");
        assert_eq!(json["code"], "unclean_shutdown");
    }
}
