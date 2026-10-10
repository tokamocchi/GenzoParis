//! コマンドの引数・応答・イベントで使う型（serde の素直な形。将来 TypeScript の型を生成するため、
//! 列挙は `snake_case` の文字列か `type` / `status` で種類を表すオブジェクトにする）。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use genzo_model::{
    AssetId, AssetKind, CaptureTime, ColorLabel, DevelopSettings, FileId, FileRole, FileStatus,
    Flag, FolderId, GpsCoord, HistoryEntryId, KeywordId, Orientation, OutputColorSpace, Rating,
    RenderQuality, VariantId, VideoMetadata, VolumeId,
};
use genzo_pipeline::ImageTile;
use serde::{Deserialize, Serialize};

use crate::error::ErrorInfo;

// ---------------------------------------------------------------------------
// ジョブ
// ---------------------------------------------------------------------------

/// ジョブの種類（P3 のバックグラウンドの処理。6.1 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// フォルダの取り込み（IMP-01）。
    Import,
    /// 書き出し（EXP-01）。
    Export,
    /// サムネイル（L0）・標準プレビュー（L1）の作り直し（PRV-02）。
    RegeneratePreviews,
    /// カタログの詳細チェック（DATA-05）。
    IntegrityCheck,
    /// 既定のタイムゾーンの変更に合わせた撮影日時の推定し直し。
    ReresolveCaptureTimes,
    /// キャッシュの回収（存在しない variant のサムネイル、L1 の上限容量。4.1 節「回収」）。
    CollectGarbage,
}

/// ジョブの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// 実行を待っている・実行中。
    Running,
    /// 成功した。
    Succeeded,
    /// 失敗した。
    Failed,
    /// 取り消された。
    Cancelled,
}

impl JobState {
    /// 終わった状態か。
    pub const fn is_finished(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// ジョブの情報（[`crate::Core::job`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    /// ジョブ。
    pub job_id: u64,
    /// 種類。
    pub kind: JobKind,
    /// 状態。
    pub state: JobState,
    /// 済んだ数。
    pub done: u64,
    /// 全体の数（分からなければ 0）。
    pub total: u64,
    /// 結果（成功した場合。取り消した場合も途中までの結果を入れることがある）。
    pub result: Option<JobResult>,
    /// 失敗の情報（失敗した場合）。
    pub error: Option<ErrorInfo>,
}

/// ジョブの結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobResult {
    /// 取り込み。
    Import(ImportReport),
    /// 書き出し。
    Export(ExportReport),
    /// サムネイル・プレビューの作り直し。
    RegeneratePreviews(RegenerateReport),
    /// 詳細チェック。
    IntegrityCheck(IntegrityReport),
    /// 撮影日時の推定し直し。
    ReresolveCaptureTimes {
        /// 変更した asset の数。
        updated: u64,
    },
    /// キャッシュの回収。
    CollectGarbage {
        /// 削除したサムネイル（L0）の数。
        thumbnails_removed: u64,
        /// 削除した標準プレビュー（L1）の数（上限容量を超えた分と、索引にないファイル）。
        previews_removed: u64,
        /// 削除した、生成の途中で終了した一時ファイルの数。
        temp_files_removed: u64,
    },
}

/// ファイルの問題（読めない・登録できないなど）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIssue {
    /// ファイル。
    pub path: PathBuf,
    /// 理由。
    pub reason: String,
}

/// variant の問題。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantIssue {
    /// variant。
    pub variant_id: VariantId,
    /// 理由。
    pub reason: String,
}

// ---------------------------------------------------------------------------
// 取り込み
// ---------------------------------------------------------------------------

/// 取り込みの結果（IMP-01・VID-01・PRV-01）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportReport {
    /// 取り込んだフォルダ。
    pub root: PathBuf,
    /// 対象の拡張子のファイルの数。
    pub files_found: u64,
    /// 新しく登録した数。
    pub added: u64,
    /// 登録済みで、内容の変化を検知した数。
    pub updated: u64,
    /// 登録済みで、変化がなかった数。
    pub unchanged: u64,
    /// メタデータを読めず `status = error` で登録したファイル（6.3 節）。
    pub errors: Vec<FileIssue>,
    /// 読めない（ハッシュを求められない）・ワーカーを起動できないなどのため、登録しなかったファイル
    /// （もう一度取り込むと処理する）。
    pub not_registered: Vec<FileIssue>,
    /// たどらなかったシンボリックリンク・ジャンクションの数（3.5 節）。
    pub skipped_links: u64,
    /// 作ったサムネイル（L0）の数。
    pub thumbnails: u64,
    /// サムネイルを作れなかったファイル。
    pub thumbnail_failures: Vec<FileIssue>,
    /// 取り消されて途中で終わったか（やり直すと続きから処理する）。
    pub cancelled: bool,
}

// ---------------------------------------------------------------------------
// 検索
// ---------------------------------------------------------------------------

/// フィルターの条件（LIB-07）。`None`・空のものは条件にしない。すべての条件を AND で組み合わせる。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchFilter {
    /// 評価の下限（この値を含む）。
    pub rating_min: Option<Rating>,
    /// 評価の上限（この値を含む）。
    pub rating_max: Option<Rating>,
    /// フラグ（いずれかに一致。-1 / 0 / 1）。
    pub flags: Option<Vec<Flag>>,
    /// カラーラベル（いずれかに一致。`null` は「ラベルなし」）。
    pub color_labels: Option<Vec<Option<ColorLabel>>>,
    /// 撮影日時（UTC）の下限（含む）。
    pub captured_from: Option<DateTime<Utc>>,
    /// 撮影日時（UTC）の上限（含まない）。
    pub captured_until: Option<DateTime<Utc>>,
    /// カメラ（完全一致。いずれかに一致）。
    pub cameras: Option<Vec<String>>,
    /// レンズ（完全一致。いずれかに一致）。
    pub lenses: Option<Vec<String>>,
    /// 種別（写真 / 動画）。
    pub kind: Option<AssetKind>,
    /// 長辺の画素数（幅と高さの大きいほう。向きによらない）の下限（含む）。写真にも動画にも効く。
    pub long_edge_min: Option<u32>,
    /// 長辺の画素数の上限（含む）。
    pub long_edge_max: Option<u32>,
    /// 動画の長さ（秒）の下限（含む。VID-03）。動画の条件（長さ・fps・コーデック）を指定すると、写真と、
    /// その値の分からない動画は除く。
    pub duration_min_s: Option<f64>,
    /// 動画の長さ（秒）の上限（含む）。
    pub duration_max_s: Option<f64>,
    /// 動画のフレームレートの下限（含む）。
    pub fps_min: Option<f64>,
    /// 動画のフレームレートの上限（含む）。
    pub fps_max: Option<f64>,
    /// 動画のコーデック（`hevc`・`h264` など。大文字・小文字を区別しない。いずれかに一致）。
    pub codecs: Option<Vec<String>>,
    /// テキスト（ファイル名とキャプション。3.6 節）。
    pub text: Option<String>,
    /// フォルダ。
    pub folder_id: Option<FolderId>,
    /// フォルダの配下も含めるか。
    pub include_subfolders: bool,
    /// キーワード（子のキーワードを含めて、すべて持つ）。
    pub keywords: Vec<KeywordId>,
    /// 主となるファイルの状態（いずれかに一致）。
    pub file_statuses: Option<Vec<FileStatus>>,
    /// マスターだけにする（仮想コピーを除く）。
    pub masters_only: bool,
}

impl SearchFilter {
    /// genzo-catalog の条件にする。
    pub(crate) fn to_catalog(&self) -> genzo_catalog::Filter {
        genzo_catalog::Filter {
            rating_min: self.rating_min,
            rating_max: self.rating_max,
            flags: self.flags.clone(),
            color_labels: self.color_labels.clone(),
            captured_from: self.captured_from,
            captured_until: self.captured_until,
            cameras: self.cameras.clone(),
            lenses: self.lenses.clone(),
            kind: self.kind,
            long_edge_min: self.long_edge_min,
            long_edge_max: self.long_edge_max,
            duration_min_s: self.duration_min_s,
            duration_max_s: self.duration_max_s,
            fps_min: self.fps_min,
            fps_max: self.fps_max,
            codecs: self.codecs.clone(),
            text: self.text.clone().filter(|t| !t.trim().is_empty()),
            folder: self.folder_id.map(|folder_id| genzo_catalog::FolderFilter {
                folder_id,
                include_subfolders: self.include_subfolders,
            }),
            keywords: self.keywords.clone(),
            file_statuses: self.file_statuses.clone(),
            masters_only: self.masters_only,
        }
    }

    /// 選別の操作（評価・フラグ・カラーラベル）で一致しなくなりうる条件を除いた条件（3.7 節の
    /// 「次にフィルターを適用し直すまで残す」に使う）。
    pub(crate) fn without_marks(&self) -> Self {
        Self {
            rating_min: None,
            rating_max: None,
            flags: None,
            color_labels: None,
            ..self.clone()
        }
    }
}

/// 並べ替えの項目（LIB-08）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    /// 撮影日時（撮影日時のないものは常に最後）。
    #[default]
    CaptureTime,
    /// ファイル名。
    FileName,
    /// 評価。
    Rating,
    /// 登録順。
    ImportOrder,
}

/// 並べ替えの向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    /// 昇順。
    #[default]
    Ascending,
    /// 降順。
    Descending,
}

/// 並べ替え（同じ値のものは variant の id の順）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSort {
    /// 項目。
    pub key: SortKey,
    /// 向き。
    pub direction: SortDirection,
}

impl SearchSort {
    /// genzo-catalog の並べ替えにする。
    pub(crate) fn to_catalog(self) -> genzo_catalog::Sort {
        genzo_catalog::Sort::new(
            match self.key {
                SortKey::CaptureTime => genzo_catalog::SortKey::CaptureTime,
                SortKey::FileName => genzo_catalog::SortKey::FileName,
                SortKey::Rating => genzo_catalog::SortKey::Rating,
                SortKey::ImportOrder => genzo_catalog::SortKey::ImportOrder,
            },
            match self.direction {
                SortDirection::Ascending => genzo_catalog::SortDirection::Ascending,
                SortDirection::Descending => genzo_catalog::SortDirection::Descending,
            },
        )
    }
}

/// 検索の結果（id はコアのメモリに保持し、UI は範囲を要求する。3.2 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// 世代（3.7 節）。
    pub generation: u64,
    /// 件数。
    pub count: u64,
}

/// グリッドのセル 1 つの内容（表示範囲の詳細）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantSummary {
    /// variant。
    pub variant_id: VariantId,
    /// asset。
    pub asset_id: AssetId,
    /// マスターか。
    pub is_master: bool,
    /// 仮想コピーの名前。
    pub variant_name: Option<String>,
    /// 写真か動画か。
    pub kind: AssetKind,
    /// 主となるファイル。
    pub file_id: FileId,
    /// 主となるファイルの名前。
    pub file_name: String,
    /// フォルダ。
    pub folder_id: FolderId,
    /// ファイルの状態（missing / error の印。3.3 節・6.3 節）。
    pub file_status: FileStatus,
    /// ファイルのリビジョン。
    pub file_revision: u32,
    /// RAW と同時に記録された JPEG があるか。
    pub has_sidecar_jpeg: bool,
    /// 評価。
    pub rating: Rating,
    /// フラグ。
    pub flag: Flag,
    /// カラーラベル。
    pub color_label: Option<ColorLabel>,
    /// 撮影日時。
    pub capture: CaptureTime,
    /// カメラ。
    pub camera: Option<String>,
    /// レンズ。
    pub lens: Option<String>,
    /// 幅（向きを反映する前）。
    pub width: Option<u32>,
    /// 高さ（向きを反映する前）。
    pub height: Option<u32>,
    /// 向き。
    pub orientation: Orientation,
    /// 現像設定のハッシュ。
    pub develop_hash: String,
    /// サムネイル（L0）のキャッシュキーのハッシュ（`genzo://thumb/{variant_id}?rev={thumb_rev}`）。
    /// まだなければ `None`。
    pub thumb_rev: Option<String>,
}

impl VariantSummary {
    pub(crate) fn from_catalog(
        s: genzo_catalog::VariantSummary,
        thumb_rev: Option<String>,
    ) -> Self {
        Self {
            variant_id: s.variant_id,
            asset_id: s.asset_id,
            is_master: s.is_master,
            variant_name: s.variant_name,
            kind: s.kind,
            file_id: s.file_id,
            file_name: s.file_name,
            folder_id: s.folder_id,
            file_status: s.file_status,
            file_revision: s.file_revision,
            has_sidecar_jpeg: s.has_sidecar_jpeg,
            rating: s.rating,
            flag: s.flag,
            color_label: s.color_label,
            capture: s.capture,
            camera: s.camera,
            lens: s.lens,
            width: s.width,
            height: s.height,
            orientation: s.orientation,
            develop_hash: s.develop_hash,
            thumb_rev,
        }
    }
}

/// 表示範囲の詳細（[`crate::Core::range`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RangeResult {
    /// 世代。
    pub generation: u64,
    /// 先頭の位置。
    pub start: u64,
    /// 詳細（検索の後に削除されたものは飛ばす）。
    pub items: Vec<VariantSummary>,
}

/// variant の位置（[`crate::Core::index_of`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexOf {
    /// 世代。
    pub generation: u64,
    /// 位置（結果になければ `None`）。
    pub index: Option<u64>,
}

/// variant の詳細（メタデータの表示。LIB-14）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantDetails {
    /// グリッドと同じ内容。
    pub summary: VariantSummary,
    /// ISO 感度。
    pub iso: Option<u32>,
    /// 絞り値。
    pub aperture: Option<f64>,
    /// シャッター速度（秒）。
    pub shutter_s: Option<f64>,
    /// 焦点距離（mm）。
    pub focal_mm: Option<f64>,
    /// GPS の座標。
    pub gps: Option<GpsCoord>,
    /// キャプション。
    pub caption: Option<String>,
    /// 動画の情報。
    pub video: Option<VideoMetadata>,
    /// ファイル（主となるファイルが先）。
    pub files: Vec<FileInfo>,
}

/// ファイルの情報。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    /// ファイル。
    pub file_id: FileId,
    /// 役割。
    pub role: FileRole,
    /// 絶対パス（マウント先が分からなければ `None`）。
    pub path: Option<PathBuf>,
    /// 大きさ（バイト）。
    pub size: u64,
    /// 状態。
    pub status: FileStatus,
    /// missing / error の理由。
    pub status_reason: Option<String>,
}

/// フォルダ（LIB-06 のフォルダツリー）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderInfo {
    /// フォルダ。
    pub folder_id: FolderId,
    /// ボリューム。
    pub volume_id: VolumeId,
    /// 親のフォルダ（ボリュームのルートは `None`）。
    pub parent_id: Option<FolderId>,
    /// ボリューム内の相対パス（'/' 区切り）。
    pub rel_path: String,
    /// 絶対パス（マウント先が分からなければ `None`）。
    pub path: Option<PathBuf>,
}

/// ファイルの状態の確認の結果（3.3 節）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileCheckReport {
    /// 確かめたファイルの数。
    pub checked: u64,
    /// 見つかったファイルの数。
    pub ok: u64,
    /// 見つからなかったファイル（`status = missing` にした）。
    pub missing: Vec<FileIssue>,
    /// 内容の変化を検知した（リビジョンを上げた）ファイル。
    pub changed: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// サムネイル・プレビュー
// ---------------------------------------------------------------------------

/// サムネイル（L0）の JPEG（UI の `genzo://thumb/{variant_id}?rev={rev}` 用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbnailData {
    /// variant。
    pub variant_id: VariantId,
    /// キャッシュキーのハッシュ（16 進数 64 文字。内容が変わると変わる）。
    pub rev: String,
    /// JPEG のバイト列（Display P3 の ICC プロファイル付き。B5）。
    pub jpeg: Vec<u8>,
}

/// サムネイル・プレビューの作り直しの結果。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RegenerateReport {
    /// 作った（L0 と、写真なら L1）variant の数。
    pub rendered: u64,
    /// 作る必要がなかった・作れなかったが問題ではない（動画、見つからないファイル、古い世代）数。
    pub skipped: u64,
    /// 失敗した variant。
    pub failed: Vec<VariantIssue>,
    /// 作ったが、現像の警告があった variant（カメラ行列がないなど。`reason` は警告の説明）。
    #[serde(default)]
    pub warnings: Vec<VariantIssue>,
}

// ---------------------------------------------------------------------------
// 現像
// ---------------------------------------------------------------------------

/// 処理した側。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderBackend {
    /// GPU 版（genzo-gpu）。
    Gpu,
    /// CPU 版（genzo-pipeline）。
    Cpu,
}

/// 現像する写真の入力の情報。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// RAW か。
    pub is_raw: bool,
    /// 幅（向きを反映する前）。
    pub width: u32,
    /// 高さ（向きを反映する前）。
    pub height: u32,
    /// RAW のデコーダの識別子。
    pub decoder: Option<String>,
    /// 撮影時の WB を色温度と tint で表したもの（RAW だけ。WB を「撮影時の値」から「カスタム」に切り替える
    /// ときのスライダーの初期値。DEV-03）。現像で使うのと同じカメラ行列（行列がなければ BT.2020 とみなす）と
    /// 撮影時の係数から求める。求められない・設定できる範囲（2000〜50000 K、tint ±150）の外なら `None`。
    #[serde(default)]
    pub as_shot_white_balance: Option<AsShotWhiteBalance>,
}

/// 撮影時の WB の色温度と tint（[`SourceInfo::as_shot_white_balance`]。`WhiteBalance::Custom` に
/// そのまま使える）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AsShotWhiteBalance {
    /// 色温度（K）。
    pub temperature_k: f32,
    /// 色かぶり補正（tint）。
    pub tint: f32,
}

/// 現像のセッションの状態（[`crate::Core::open_develop`] など）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevelopState {
    /// variant。
    pub variant_id: VariantId,
    /// 現在の設定。
    pub settings: DevelopSettings,
    /// 設定の世代（設定を変えるたびに増える）。
    pub generation: u64,
    /// 取り消せる履歴があるか（保存待ちの変更があれば、それも取り消せる）。
    pub can_undo: bool,
    /// やり直せる履歴があるか。
    pub can_redo: bool,
    /// 保存待ちの変更があるか（DATA-03）。
    pub dirty: bool,
    /// ドラッグ中か。
    pub dragging: bool,
    /// 入力。
    pub source: SourceInfo,
}

/// 履歴の 1 件（DEV-27）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    /// ID。
    pub id: HistoryEntryId,
    /// 記録した日時。
    pub created_at: DateTime<Utc>,
    /// 操作の名前。
    pub label: String,
    /// 現在の位置か。
    pub is_current: bool,
}

/// 白飛び・黒つぶれの画素の数（DEV-26。表示用の 8bit の階級の端）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClipCounts {
    /// R。
    pub red: u64,
    /// G。
    pub green: u64,
    /// B。
    pub blue: u64,
    /// どれか 1 つ以上。
    pub any: u64,
    /// 3 つとも。
    pub all: u64,
}

impl From<genzo_pipeline::finish::output::ClipCounts> for ClipCounts {
    fn from(c: genzo_pipeline::finish::output::ClipCounts) -> Self {
        Self {
            red: c.red,
            green: c.green,
            blue: c.blue,
            any: c.any,
            all: c.all,
        }
    }
}

/// ヒストグラム（17c。画面への出力の符号化した値の 256 階級）とクリッピングの情報。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HistogramData {
    /// R の度数（256 個）。
    pub red: Vec<u64>,
    /// G の度数。
    pub green: Vec<u64>,
    /// B の度数。
    pub blue: Vec<u64>,
    /// 輝度の度数。
    pub luma: Vec<u64>,
    /// 画素数。
    pub pixel_count: u64,
    /// 白飛び（階級 255）。
    pub clipped_high: ClipCounts,
    /// 黒つぶれ（階級 0）。
    pub clipped_low: ClipCounts,
}

impl From<&genzo_pipeline::finish::Histogram> for HistogramData {
    fn from(h: &genzo_pipeline::finish::Histogram) -> Self {
        Self {
            red: h.red.to_vec(),
            green: h.green.to_vec(),
            blue: h.blue.to_vec(),
            luma: h.luma.to_vec(),
            pixel_count: h.pixel_count,
            clipped_high: h.clipped_high.into(),
            clipped_low: h.clipped_low.into(),
        }
    }
}

/// 現像のプレビューの情報（ピクセルを除く。コマンドの応答とイベントで使う）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameInfo {
    /// variant。
    pub variant_id: VariantId,
    /// 設定の世代。
    pub generation: u64,
    /// 品質。
    pub quality: RenderQuality,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
    /// 処理した側。
    pub backend: RenderBackend,
    /// ヒストグラムとクリッピングの情報。
    pub histogram: HistogramData,
    /// 画面の出力を sRGB とみなしたか（IQ-05）。
    pub display_assumed_srgb: bool,
    /// 現像の警告。
    pub warnings: Vec<String>,
}

/// 現像のプレビュー（ピクセルを含む。[`crate::Core::develop_frame`]）。
#[derive(Debug, Clone)]
pub struct DevelopFrame {
    /// 情報。
    pub info: FrameInfo,
    /// ステージ 16 の出力（B3。ビューポートが自分で 17a を行う場合に使う）。
    pub b3: Arc<ImageTile>,
    /// ステージ 17a の出力（モニターのプロファイルで符号化した値）を 8bit にしたもの
    /// （R, G, B の並び、行優先）。
    pub display_rgb8: Arc<Vec<u8>>,
}

/// 設定の貼り付けで写す項目（DEV-30。DEV-29 の「一部の項目だけを適用」にも使う）。既定はすべて。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingGroups {
    /// ホワイトバランス。
    pub white_balance: bool,
    /// 露光量・コントラスト・ハイライト・シャドウ・白・黒。
    pub basic_tone: bool,
    /// トーンカーブ。
    pub tone_curve: bool,
    /// 彩度・自然な彩度など。
    pub color: bool,
    /// 切り抜き・角度補正。
    pub geometry: bool,
    /// レンズ補正。
    pub lens: bool,
    /// シャープ・ノイズ軽減。
    pub detail: bool,
    /// ローカル補正。
    pub masks: bool,
}

impl Default for SettingGroups {
    fn default() -> Self {
        Self::ALL
    }
}

impl SettingGroups {
    /// すべての項目。
    pub const ALL: Self = Self {
        white_balance: true,
        basic_tone: true,
        tone_curve: true,
        color: true,
        geometry: true,
        lens: true,
        detail: true,
        masks: true,
    };

    /// すべての項目か。
    pub fn is_all(&self) -> bool {
        *self == Self::ALL
    }

    /// `target` に `source` の選んだ項目を写したもの。処理バージョンと外部データは `target` のまま
    /// （すべての項目を写す場合は `source` をそのまま使う）。
    pub fn merge(&self, source: &DevelopSettings, target: &DevelopSettings) -> DevelopSettings {
        if self.is_all() {
            return source.clone();
        }
        let mut out = target.clone();
        if self.white_balance {
            out.white_balance = source.white_balance;
        }
        if self.basic_tone {
            out.exposure_ev = source.exposure_ev;
            out.contrast = source.contrast;
            out.tone = source.tone;
        }
        if self.tone_curve {
            out.tone_curve = source.tone_curve.clone();
        }
        if self.color {
            out.color = source.color;
        }
        if self.geometry {
            out.geometry = source.geometry;
        }
        if self.lens {
            out.lens = source.lens;
        }
        if self.detail {
            out.detail = source.detail;
        }
        if self.masks {
            out.masks = source.masks.clone();
        }
        out
    }
}

/// 画面のプロファイルの情報（IQ-05）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// sRGB とみなしたか（UI にその旨を表示する）。
    pub assumed_srgb: bool,
    /// sRGB とみなした理由。
    pub reason: Option<String>,
    /// プロファイルの説明。
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// 書き出し
// ---------------------------------------------------------------------------

/// 書き出しの 1 件の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExportOutcome {
    /// 書き出した。
    Written {
        /// 書き出したファイル（連番を付けた場合は付けた後の名前）。
        path: PathBuf,
        /// 既存のファイルを置き換えたか。
        replaced: bool,
        /// 現像を処理した側。
        backend: RenderBackend,
        /// 現像の警告（カメラ行列がないため色が正確でない、この処理バージョンでは適用しない項目など。
        /// 表示用の説明）。
        #[serde(default)]
        warnings: Vec<String>,
        /// 実際に使った外部データ（RAW のカメラ行列・デコーダ。04 の 2.5 節。RAW 以外は `None`）。
        #[serde(default)]
        render_deps: Option<genzo_model::RenderDeps>,
    },
    /// 書き出さなかった（衝突時の「スキップ」・動画など）。
    Skipped {
        /// 理由。
        reason: String,
        /// 既にあったファイル（衝突の場合）。
        existing: Option<PathBuf>,
    },
    /// 失敗した。
    Failed {
        /// エラーの情報。
        error: ErrorInfo,
    },
}

/// 書き出しの 1 件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportItem {
    /// variant。
    pub variant_id: VariantId,
    /// 結果。
    pub outcome: ExportOutcome,
}

/// 書き出しの結果の一覧（ストーリー 7）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ExportReport {
    /// 書き出し先のフォルダ。
    pub dest_dir: PathBuf,
    /// 1 件ずつの結果（要求の順）。
    pub items: Vec<ExportItem>,
    /// 書き出した数。
    pub written: u64,
    /// 書き出さなかった数。
    pub skipped: u64,
    /// 失敗した数。
    pub failed: u64,
    /// 取り消されて途中で終わったか。
    pub cancelled: bool,
}

// ---------------------------------------------------------------------------
// 削除
// ---------------------------------------------------------------------------

/// 削除の種類（6.4 節の 3 つの別々の操作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteKind {
    /// 仮想コピーの削除（マスターは削除できない。元ファイルは変更しない）。
    VirtualCopies,
    /// カタログからの除去（asset と、そのすべての variant。元ファイルは変更しない）。
    RemoveFromCatalog,
    /// ゴミ箱へ移動（asset のすべてのファイルを OS のゴミ箱へ移し、カタログから除く）。
    Trash,
}

/// 削除で消える variant。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedVariant {
    /// variant。
    pub variant_id: VariantId,
    /// asset。
    pub asset_id: AssetId,
    /// マスターか。
    pub is_master: bool,
    /// 仮想コピーの名前。
    pub name: Option<String>,
}

/// ゴミ箱へ移すファイル。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedFile {
    /// ファイル。
    pub file_id: FileId,
    /// asset。
    pub asset_id: AssetId,
    /// 役割（RAW と JPEG のペアなど）。
    pub role: FileRole,
    /// 絶対パス（マウント先が分からなければ `None`）。
    pub path: Option<PathBuf>,
}

/// 削除の計画（実行前に、影響するファイルと variant の一覧を示して確認する。6.4 節）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletePlan {
    /// 計画の ID（[`crate::Core::execute_delete`] に渡す）。
    pub plan_id: u64,
    /// 種類。
    pub kind: DeleteKind,
    /// 対象の asset（仮想コピーの削除では空）。
    pub assets: Vec<AssetId>,
    /// カタログから消える variant（仮想コピーを含む）。
    pub variants: Vec<PlannedVariant>,
    /// ゴミ箱へ移すファイル（ゴミ箱への移動のときだけ）。
    pub files: Vec<PlannedFile>,
}

/// 削除の結果。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeleteReport {
    /// カタログから除いた variant。
    pub removed_variants: Vec<VariantId>,
    /// カタログから除いた asset。
    pub removed_assets: Vec<AssetId>,
    /// ゴミ箱へ移したファイル。
    pub trashed_files: Vec<PathBuf>,
    /// ゴミ箱へ移す対象だったが、元の場所になかった（フォルダはあり、アプリの外で削除・移動された）
    /// ため、移さなかったファイル（その asset はカタログから除いた）。
    #[serde(default)]
    pub skipped_missing: Vec<PathBuf>,
    /// 失敗（ゴミ箱へ移せなかった asset のファイル、元の場所が見えない（ドライブが外れているなど）
    /// ファイル、登録の後に別の内容に置き換わったファイルなど。その asset はカタログに残る）。
    pub failed: Vec<FileIssue>,
}

// ---------------------------------------------------------------------------
// 設定・保守
// ---------------------------------------------------------------------------

/// 設定（SYS-05。カタログの設定テーブルに保存する）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreSettings {
    /// L1 プレビューのキャッシュの場所（変更は次に開いたときに有効になる）。
    pub preview_cache_dir: PathBuf,
    /// L1 プレビューのキャッシュの上限（バイト。SCL-04）。
    pub preview_cache_bytes: u64,
    /// 書き出しの既定の色空間。
    pub default_export_color_space: OutputColorSpace,
    /// 既定のタイムゾーンのオフセット（分。UTC より東が正）。撮影日時にオフセットがない写真・動画の
    /// UTC の推定に使う（3.1 節）。
    pub default_utc_offset_minutes: i32,
}

/// 設定の変更（`None` の項目は変えない）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsUpdate {
    /// L1 プレビューのキャッシュの場所。
    pub preview_cache_dir: Option<PathBuf>,
    /// L1 プレビューのキャッシュの上限（バイト）。
    pub preview_cache_bytes: Option<u64>,
    /// 書き出しの既定の色空間。
    pub default_export_color_space: Option<OutputColorSpace>,
    /// 既定のタイムゾーンのオフセット（分）。変えると、既定のオフセットで推定していた撮影日時を
    /// バックグラウンドで推定し直す。
    pub default_utc_offset_minutes: Option<i32>,
}

/// バックアップ（DATA-04）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupEntry {
    /// ファイル。
    pub path: PathBuf,
    /// 作った日時。
    pub created_at: DateTime<Utc>,
}

impl From<genzo_catalog::BackupInfo> for BackupEntry {
    fn from(b: genzo_catalog::BackupInfo) -> Self {
        Self {
            path: b.path,
            created_at: b.created_at,
        }
    }
}

/// 詳細チェックの結果（DATA-05）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IntegrityReport {
    /// 問題がないか。
    pub ok: bool,
    /// `PRAGMA integrity_check` の問題。
    pub integrity_errors: Vec<String>,
    /// 外部キーの違反（「テーブル rowid → 参照先」の形）。
    pub foreign_key_violations: Vec<String>,
    /// テキスト検索の索引の不一致。
    pub fts_error: Option<String>,
}

impl From<genzo_catalog::IntegrityReport> for IntegrityReport {
    fn from(r: genzo_catalog::IntegrityReport) -> Self {
        Self {
            ok: r.is_ok(),
            foreign_key_violations: r
                .foreign_key_violations
                .iter()
                .map(|v| format!("{} {:?} → {}", v.table, v.rowid, v.parent))
                .collect(),
            integrity_errors: r.integrity_errors,
            fts_error: r.fts_error,
        }
    }
}

/// カタログの件数とスキーマの版（[`crate::Core::catalog_stats`]。CLI の `catalog info` など）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogStats {
    /// スキーマの版（`PRAGMA user_version`。3.4 節のマイグレーション）。
    pub schema_version: u32,
    /// asset の数。
    pub assets: u64,
    /// variant の数（仮想コピーを含む）。
    pub variants: u64,
    /// ファイルの数。
    pub files: u64,
    /// 現像の履歴の件数。
    pub history_entries: u64,
}

/// 前回の終了の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviousShutdown {
    /// 初めて開いた。
    FirstOpen,
    /// 正常に終了した。
    Clean,
    /// 正常に終了しなかった（詳細チェックを案内する。DATA-05）。
    Unclean,
}

/// 起動時の確認の結果（[`crate::Core::startup_report`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupReport {
    /// 前回の終了の状態。
    pub previous_shutdown: PreviousShutdown,
    /// マイグレーションを適用した場合の、適用前のスキーマの版。
    pub migrated_from: Option<u32>,
    /// 確定させた、終わっていなかったファイル操作の数（DATA-07）。
    pub recovered_file_ops: u64,
    /// 起動時に作った自動バックアップ。
    pub backup: Option<BackupEntry>,
}

/// コアの機能の有無（[`crate::Core::capabilities`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// ワーカーの RAW のデコーダ（LibRaw が無効な build では `None`）。
    pub raw_decoder: Option<String>,
    /// ワーカーの版。
    pub worker_version: String,
    /// GPU（使えれば、アダプターの説明）。
    pub gpu: Option<String>,
}
