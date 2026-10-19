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
    /// 現像中の写真のビフォー（DEV-31。[`crate::Core::set_before`]）のプレビューが更新された（ピクセルは
    /// [`crate::Core::before_frame`] で取る）。
    BeforeUpdated {
        /// variant。
        variant_id: VariantId,
        /// ビフォーの描画の世代（[`crate::BeforeInfo::generation`]）。
        generation: u64,
        /// 幅。
        width: u32,
        /// 高さ。
        height: u32,
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
        /// 件数（表示する写真の数。折りたたんだスタックは 1 枚に数える）。
        count: u64,
        /// 折りたたんだスタックの中に隠した写真の数（LIB-12。0 なら JSON に書かない）。
        #[serde(default, skip_serializing_if = "crate::types::is_zero")]
        hidden: u64,
    },
    /// 警告（GPU から CPU への切り替え、前回の異常終了など。6.3 節）。
    Warning {
        /// 種類。
        code: WarningCode,
        /// 表示用の説明。
        message: String,
        /// 関係する variant（あれば）。
        variant_id: Option<VariantId>,
        /// 関係するファイル（あれば。UTF-8 で表せない部分は U+FFFD に置き換えて JSON にする。
        /// `lossy_path`）。
        #[serde(serialize_with = "crate::types::lossy_path")]
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
    /// 説明のメタデータ（タイトル・作成者・著作権など。キャプションを含む。LIB-15）。
    Metadata,
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
    /// キーワード（写真への付与・外す、キーワードの作成・名前の変更・移動・削除・書き出しの印。LIB-09）。
    /// キーワードそのものの変更では `variant_ids` は空。
    Keywords,
    /// コレクション（メンバーの追加・削除、コレクションの作成・名前の変更・移動・削除・条件の変更。
    /// LIB-10・LIB-11）。コレクションそのものの変更では `variant_ids` は空。
    Collections,
    /// スタック（作る・解除・写真の追加と除外・一番上の写真・折りたたみ・自動スタック。LIB-12）。
    /// `variant_ids` は、印（位置・数・折りたたみ）の変わったスタックの写真。
    Stacks,
    /// 仮想コピーの名前・マスターの入れ替え（LIB-13）。
    Variants,
    /// ファイルの場所（アプリの中での移動・リネーム・再リンク。FILE-02・FILE-04）。`variant_ids` は場所の
    /// 変わった写真。
    Files,
    /// フォルダ・ボリューム（作成・移動・名前の変更・場所の指定し直し・見つからない印・つながりの変化。
    /// FILE-02〜FILE-04）。`variant_ids` は空で、`all` が真（フォルダの一覧を読み直す）。
    Folders,
    /// 後から付けた位置（付ける・消す・取り消し。LIB-21）。付ける・消すでは、`variant_ids` は対象の写真の
    /// すべての variant（仮想コピーを含む）で、`all` は偽。取り消しでは `variant_ids` は空で、`all` が真。
    /// 検索結果の並び・絞り込み・グリッドの要約には位置を使わない。
    Location,
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
    /// コピー取り込みで、カタログへの登録に失敗したため、そのバッチでコピーしたファイルを消した（IMP-02。
    /// 元のファイルはそのまま）。
    ImportCopyRolledBack,
    /// 以前の形式（パスで識別）のボリュームのフォルダを、ドライブの ID で識別するボリュームへ移した（FILE-03）。
    VolumesRebound,
    /// 動画のコピーの警告（登録の後に元のファイルが変わっていた・同じ名前があったので連番を付けたなど。EXP-05）。
    VideoCopy,
    /// 別のドライブへ移した写真の元のファイルを、ゴミ箱へ移せなかった（FILE-02。カタログは移動先を指し、元の
    /// ファイルは元の場所に残る）。
    MoveSourceLeft,
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
            hidden: 0,
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
            hidden: 0,
        });
        assert_eq!(
            rx.try_recv().unwrap(),
            Event::SearchUpdated {
                generation: 2,
                count: 3,
                hidden: 0,
            }
        );
        drop(rx);
        hub.emit(Event::SearchUpdated {
            generation: 3,
            count: 0,
            hidden: 0,
        });
        assert!(hub.subscribers.lock().is_empty(), "drop した購読者は外す");
        // JSON の形。
        let json = serde_json::to_value(&hub.sticky()[0]).unwrap();
        assert_eq!(json["type"], "warning");
        assert_eq!(json["code"], "unclean_shutdown");
    }
}
