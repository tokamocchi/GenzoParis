//! コマンドの引数・応答・イベントで使う型（serde の素直な形。将来 TypeScript の型を生成するため、
//! 列挙は `snake_case` の文字列か `type` / `status` で種類を表すオブジェクトにする）。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use genzo_model::{
    AssetId, AssetKind, CaptureTime, CollectionId, CollectionKind, ColorLabel, DevelopSettings,
    FileId, FileRole, FileStatus, Flag, FolderId, GpsCoord, HistoryEntryId, IptcMetadata,
    KeywordId, MetadataField, Orientation, OutputColorSpace, Rating, RenderQuality, SmartRule,
    SnapshotId, StackId, VariantId, VideoMetadata, VolumeId,
};
use genzo_pipeline::ImageTile;
use serde::{Deserialize, Serialize, Serializer};

use crate::error::ErrorInfo;

// ---------------------------------------------------------------------------
// パス（JSON の文字列）
// ---------------------------------------------------------------------------

/// パスを JSON の文字列にする（UI に返す型の `PathBuf`・`Option<PathBuf>`・`Vec<PathBuf>` の項目に
/// `#[serde(serialize_with = "lossy_path")]` で付ける）。
///
/// UTF-8 で表せない名前（Linux などの UTF-8 でないバイト列・Windows の対にならないサロゲート）は、その部分を
/// U+FFFD に置き換える（`to_string_lossy`）。serde の既定の `Path` の Serialize はエラーにするため、1 つの
/// ファイルの名前のせいで応答全体（取り込みの結果など）を JSON にできなくなる。置き換えた名前は表示用で、
/// そのパスでファイルを開けるとは限らない。UTF-8 で表せるパスは今までと同じ文字列になる。読み込み
/// （Deserialize）は今までどおり `PathBuf` として読む。
pub(crate) fn lossy_path<P, S>(path: &P, serializer: S) -> Result<S::Ok, S::Error>
where
    P: LossyPath + ?Sized,
    S: Serializer,
{
    path.serialize_lossy(serializer)
}

/// [`lossy_path`] で文字列にできるパス（とその `Option`・`Vec`）。
pub(crate) trait LossyPath {
    fn serialize_lossy<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;
}

impl LossyPath for PathBuf {
    fn serialize_lossy<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string_lossy())
    }
}

impl<T: LossyPath> LossyPath for Option<T> {
    fn serialize_lossy<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Some(p) => serializer.serialize_some(&Lossy(p)),
            None => serializer.serialize_none(),
        }
    }
}

impl<T: LossyPath> LossyPath for Vec<T> {
    fn serialize_lossy<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(Lossy))
    }
}

/// `Option`・`Vec` の中身を [`LossyPath`] で書く。
struct Lossy<'a, T: ?Sized>(&'a T);

impl<T: LossyPath + ?Sized> Serialize for Lossy<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize_lossy(serializer)
    }
}

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
    /// ファイルの内容のハッシュ（全体の BLAKE3）の計算（ORG-03。3.11 節。専用の 1 本のスレッドで、
    /// 現像などの対話の処理の合間に進める）。
    HashFiles,
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
    /// ファイルの内容のハッシュの計算。
    HashFiles(HashReport),
}

/// ファイルの問題（読めない・登録できないなど）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIssue {
    /// ファイル（UTF-8 で表せない名前のファイルもそのまま入る。JSON では、表せない部分を U+FFFD に
    /// 置き換える。`lossy_path`）。
    #[serde(serialize_with = "lossy_path")]
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

/// 取り込みの設定（IMP-02〜IMP-06）。`skip_duplicates`・`separate_raw_jpeg` の `None` は、カタログの設定
/// （[`CoreSettings`] の `import_skip_duplicates`・`import_separate_raw_jpeg`）に従う。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    /// 重複と思われるもの（内容が同じファイルが別の場所で登録済みのもの。全体のハッシュで確かめる）を
    /// 取り込まない。
    pub skip_duplicates: Option<bool>,
    /// 同じフォルダで拡張子だけが違う RAW と JPEG を、別々の写真として取り込む（`false` なら 1 枚にまとめる）。
    pub separate_raw_jpeg: Option<bool>,
    /// コピー取り込み（IMP-02・IMP-03）。`None` なら、今の場所のまま登録する（コピーしない。IMP-01）。
    pub copy: Option<CopySettings>,
    /// 取り込んだ（新しく登録した）写真にまとめて付けるキーワード・説明のメタデータ（IMP-05）。
    pub metadata: ImportMetadata,
    /// コピー取り込みで、コピー先の名前を決めるだけで、コピー・登録をしない（確認用。結果の
    /// [`CopyReport::copied`] に予定を入れる）。コピーしない取り込みでは使えない。
    pub dry_run: bool,
}

/// コピー取り込みの設定（IMP-02・IMP-03。04 の 3.12 節）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopySettings {
    /// コピー先のフォルダ（あるフォルダ。中のフォルダは規則に従って作る）。
    pub destination: PathBuf,
    /// フォルダの規則（既定は撮影日で `2026/2026-10-13`）。
    #[serde(default)]
    pub folder_rule: crate::naming::FolderRule,
    /// ファイル名の規則（`None` なら元の名前のまま。同じ名前があれば `-1` などを付ける）。
    #[serde(default)]
    pub rename: Option<crate::naming::RenameRule>,
}

/// 取り込んだ写真にまとめて付けるもの（IMP-05）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportMetadata {
    /// キーワードの階層のパス（`[["場所", "京都"], ["家族"]]`。なければ作る。LIB-09）。
    pub keywords: Vec<Vec<String>>,
    /// 説明のメタデータ（`{"creator": "岡本", "copyright": "© 2026 岡本"}`。空の値は付けない。LIB-15）。
    pub fields: std::collections::BTreeMap<MetadataField, String>,
}

impl ImportMetadata {
    /// 付けるものがないか。
    pub fn is_empty(&self) -> bool {
        self.to_tags().is_empty()
    }

    /// カタログに渡す形。
    pub(crate) fn to_tags(&self) -> genzo_catalog::ImportTags {
        genzo_catalog::ImportTags {
            keyword_paths: self.keywords.clone(),
            metadata: self.fields.iter().map(|(f, v)| (*f, v.clone())).collect(),
        }
    }
}

/// コピーした（する）ファイル（IMP-02）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiedFile {
    /// 元のファイル。
    #[serde(serialize_with = "lossy_path")]
    pub source: PathBuf,
    /// コピー先のファイル。
    #[serde(serialize_with = "lossy_path")]
    pub destination: PathBuf,
    /// コピーせずに、コピー先に既にあった同じ内容のファイル（前の取り込みが途中で終わった場合など）を登録した
    /// （する）か。
    #[serde(default)]
    pub existing: bool,
}

/// コピー取り込みの結果（IMP-02・IMP-03）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyReport {
    /// コピー先のフォルダ。
    #[serde(serialize_with = "lossy_path")]
    pub destination: PathBuf,
    /// コピーして登録したファイル（予定だけのときはその予定）。元のファイルの名前の順ではなく、撮影日時の順。
    pub copied: Vec<CopiedFile>,
    /// コピーしたバイト数（既にあったファイルは含まない）。
    pub bytes_copied: u64,
    /// 撮影日時が分からず、ファイルの更新日時でフォルダ・名前の日付を決めたファイルの数。
    pub dated_by_file_time: u64,
    /// 名前を決めるだけで、コピー・登録をしなかった（[`ImportOptions::dry_run`]）か。
    pub dry_run: bool,
    /// 次の連番（この取り込みで使った最後の番号の次。複数のフォルダを続けて取り込むときに、次のフォルダの
    /// 開始番号にする。名前を変えないときも数える）。
    pub next_sequence: u64,
}

/// コピー先の名前の例（[`crate::Core::import_name_preview`]。IMP-03 の画面の確認用）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NamePreview {
    /// 見本の写真の名前の例（RAW と JPEG のペア・続けて撮った写真・カメラの分からない動画）。
    pub examples: Vec<NameExample>,
    /// 設定の誤り（あれば例は空）。
    pub error: Option<String>,
}

/// 名前の例の 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameExample {
    /// 見本の元のファイル（取り込むフォルダからの相対パス。'/' 区切り）。
    pub source: String,
    /// コピー先（コピー先のフォルダからの相対パス。'/' 区切り）。
    pub destination: String,
    /// 見本の撮影日時（現地の時刻）。
    pub captured: String,
}

/// 取り込みで、内容が同じファイル（重複）として見つけたもの（IMP-04）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateFile {
    /// 取り込もうとしたファイル。
    #[serde(serialize_with = "lossy_path")]
    pub path: PathBuf,
    /// 同じ内容の、登録済みのファイル（同じ取り込みで先に登録したものを含む。場所が分からなければ `None`）。
    #[serde(serialize_with = "lossy_path")]
    pub existing_path: Option<PathBuf>,
    /// そのファイルの写真（マスターの variant。分からなければ `None`）。
    pub existing_variant_id: Option<VariantId>,
    /// 取り込まなかったか（`false` なら、設定により別の写真として登録した）。
    pub skipped: bool,
}

/// 取り込みの結果（IMP-01・VID-01・PRV-01）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportReport {
    /// 取り込んだフォルダ。
    #[serde(serialize_with = "lossy_path")]
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
    /// 内容が同じファイルが登録済みだったもの（IMP-04。取り込まなかったもの・設定により登録したもの）。
    #[serde(default)]
    pub duplicates: Vec<DuplicateFile>,
    /// RAW と組にして 1 枚の写真にまとめたファイルの数（IMP-06。新しく登録したもの）。
    #[serde(default)]
    pub paired: u64,
    /// 重複と思われるものを取り込まない設定だったか。
    #[serde(default)]
    pub skip_duplicates: bool,
    /// RAW と JPEG を別々の写真として取り込む設定だったか。
    #[serde(default)]
    pub separate_raw_jpeg: bool,
    /// コピー取り込みの結果（コピーしない取り込みでは `None`。IMP-02）。
    #[serde(default)]
    pub copy: Option<Box<CopyReport>>,
    /// キーワード・説明のメタデータをまとめて付けた写真の数（IMP-05。RAW と JPEG のペアは 1 枚）。
    #[serde(default)]
    pub tagged: u64,
}

impl ImportReport {
    /// 重複のため取り込まなかったファイルの数。
    pub fn duplicates_skipped(&self) -> u64 {
        self.duplicates.iter().filter(|d| d.skipped).count() as u64
    }
}

// ---------------------------------------------------------------------------
// 内容による同一性（ORG-03・IMP-04）
// ---------------------------------------------------------------------------

/// ファイルの内容のハッシュの計算の結果（[`JobKind::HashFiles`]。ORG-03）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HashReport {
    /// 全体のハッシュを記録したファイルの数。
    pub hashed: u64,
    /// ネットワークのボリューム（SMB・NFS など）にあるため計算しなかったファイルの数（設定で含められる）。
    pub skipped_network: u64,
    /// ボリュームが接続されていない（マウント先が分からない・見えない）ため計算しなかったファイルの数。
    pub skipped_unavailable: u64,
    /// 登録の後に内容が変わっていた・計算の間に変わったため記録しなかったファイルの数（ファイルの確認で
    /// 登録し直すと、次の計算の対象になる）。
    pub changed: u64,
    /// 読めなかったファイルの数。
    pub failed_count: u64,
    /// 読めなかったファイル（最初の [`crate::HASH_FAILURES_KEPT`] 件）。
    pub failed: Vec<FileIssue>,
    /// 取り消されて途中で終わったか。
    pub cancelled: bool,
}

/// ファイルの内容のハッシュの状況（[`crate::Core::content_hash_status`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContentHashStatus {
    /// 登録されているファイルの数（RAW と組の JPEG も 1 つに数える）。
    pub files: u64,
    /// 全体のハッシュを記録したファイルの数。
    pub hashed: u64,
    /// 計算の対象（全体のハッシュがなく、見つからない状態でない）ファイルの数。
    pub pending: u64,
    /// 内容が同じファイルの組の数（全体のハッシュを記録したファイルの中で）。
    pub duplicate_groups: u64,
    /// 実行中の計算のジョブ（なければ `None`）。
    pub job_id: Option<u64>,
}

/// 内容が同じファイルの 1 つ（[`DuplicateGroup`]・[`FileIdentity`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentFile {
    /// ファイル。
    pub file_id: FileId,
    /// asset。
    pub asset_id: AssetId,
    /// 写真（asset のマスターの variant）。
    pub variant_id: VariantId,
    /// 役割（RAW と組の JPEG なら `sidecar_jpeg`）。
    pub role: FileRole,
    /// 絶対パス（マウント先が分からなければ `None`）。
    #[serde(serialize_with = "lossy_path")]
    pub path: Option<PathBuf>,
    /// 大きさ（バイト）。
    pub size: u64,
    /// 状態。
    pub status: FileStatus,
}

/// 内容が同じファイルの組（[`crate::Core::duplicate_groups`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateGroup {
    /// 全体のハッシュ（BLAKE3 の 16 進数）。
    pub full_hash: String,
    /// 同じ内容のファイル（2 つ以上）。
    pub files: Vec<ContentFile>,
}

/// 内容が同じファイルの組の一覧。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DuplicateGroupList {
    /// 組の総数（`groups` は上限までの数）。
    pub total: u64,
    /// 組（最初に登録したファイルの順）。
    pub groups: Vec<DuplicateGroup>,
}

/// ファイルの内容による識別の結果（[`crate::Core::identify_file`]。ORG-03）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIdentity {
    /// 調べたファイル。
    #[serde(serialize_with = "lossy_path")]
    pub path: PathBuf,
    /// 大きさ（バイト）。
    pub size: u64,
    /// ファイル全体の BLAKE3（16 進数。`b3sum` と同じ値）。
    pub full_hash: String,
    /// 同じ内容の登録済みのファイル（場所・名前によらない。全体のハッシュで確かめたもの）。
    pub matches: Vec<ContentFile>,
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
    /// コレクション（LIB-10・LIB-11。手動のコレクションはメンバー、スマートコレクションは条件に一致する
    /// もの、コレクションセットは中のコレクションのいずれかに一致するもの）。
    pub collection_id: Option<CollectionId>,
    /// スマートコレクションの条件（保存していないもの。条件の編集中の件数など。LIB-11）。
    pub smart_rule: Option<SmartRule>,
    /// スタック（LIB-12）を、保存した折りたたみの状態によらず、すべて展開して並べるか（[`crate::Core::search`]
    /// の結果のまとめ方だけに効く。一致する写真・件数（[`crate::Core::count`]）は変わらない）。
    pub expand_stacks: bool,
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
            collection: self.collection_id,
            smart_rule: self.smart_rule.clone(),
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
    /// 件数（表示する写真の数。折りたたんだスタックは 1 枚に数える。3.10 節）。
    pub count: u64,
    /// 条件に一致したが、折りたたんだスタックの中に隠した写真の数（LIB-12。0 なら JSON に書かない）。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hidden: u64,
}

/// 0 か（JSON に書かない項目の判定）。
pub(crate) fn is_zero(n: &u64) -> bool {
    *n == 0
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
    /// スタック（LIB-12）に入っていれば、その印（位置・写真の数・折りたたみ）。
    #[serde(default)]
    pub stack: Option<StackBadge>,
}

/// グリッドのセルに出すスタックの印（LIB-12。[`VariantSummary::stack`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackBadge {
    /// スタック。
    pub stack_id: StackId,
    /// スタックの中の位置（1 から。1 が一番上）。
    pub position: u32,
    /// スタックの写真の数（2 以上）。
    pub count: u32,
    /// 折りたたんでいるか。
    pub collapsed: bool,
}

impl From<genzo_catalog::StackBadge> for StackBadge {
    fn from(b: genzo_catalog::StackBadge) -> Self {
        Self {
            stack_id: b.stack_id,
            position: b.position,
            count: b.count,
            collapsed: b.collapsed,
        }
    }
}

/// スタック（LIB-12）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackInfo {
    /// スタック。
    pub stack_id: StackId,
    /// 一番上の写真（`members` の最初）。
    pub top_variant_id: VariantId,
    /// 折りたたんでいるか。
    pub collapsed: bool,
    /// 写真（スタックの中の順。最初が一番上）。
    pub members: Vec<VariantId>,
}

impl From<genzo_catalog::StackInfo> for StackInfo {
    fn from(s: genzo_catalog::StackInfo) -> Self {
        Self {
            stack_id: s.id,
            top_variant_id: s.top_variant_id,
            collapsed: s.collapsed,
            members: s.members,
        }
    }
}

/// 撮影時刻の間隔による自動スタックの結果（LIB-12。[`crate::Core::auto_stack`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AutoStackReport {
    /// 作った（見込みなら作る）スタックの数。
    pub stacks: u64,
    /// 作ったスタックに入れた写真の数。
    pub photos: u64,
    /// 対象にした写真（条件に一致し、スタックに入っておらず、撮影日時の分かるもの）の数。
    pub considered: u64,
    /// すでにスタックに入っていたので対象にしなかった写真の数。
    pub skipped_in_stacks: u64,
    /// 撮影日時が分からないので対象にしなかった写真の数。
    pub skipped_without_time: u64,
    /// 見込みだけか（何も変えていない）。
    pub dry_run: bool,
}

/// 同じ写真（asset）のマスターと仮想コピーの 1 件（LIB-13。[`VariantDetails::variants`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantBrief {
    /// variant。
    pub variant_id: VariantId,
    /// マスター（元の写真）か。
    pub is_master: bool,
    /// 仮想コピーの名前（マスターは `null`）。
    pub name: Option<String>,
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
            stack: s.stack.map(StackBadge::from),
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

/// 絞り込みの候補の値と件数（[`Facets`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetValue {
    /// 値（[`SearchFilter`] の `cameras`・`lenses`・`codecs` にそのまま渡すと一致する文字列）。
    pub value: String,
    /// 件数（**asset（写真・動画 1 つ）の数**。仮想コピーは数えない。genzo-catalog の `Facets` の doc）。
    pub count: u64,
}

/// 絞り込みの候補（[`crate::Core::facets`]。カタログ全体の、カメラ・レンズ・動画のコーデックと件数）。
///
/// - 値の順（バイト列の順）に並べる。値が分からない（NULL・空の文字列）ものは候補にしない。
/// - 件数は asset の数（[`FacetValue::count`]）。その値で絞り込んだ検索の件数（variant の数）は、仮想
///   コピーの分だけ多くなりうる（マスターだけの検索の件数と同じ）。
/// - 今の検索の条件にはよらない（カタログ全体の候補）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Facets {
    /// カメラ（メーカーと機種。`asset.camera` の値）。
    pub cameras: Vec<FacetValue>,
    /// レンズ。
    pub lenses: Vec<FacetValue>,
    /// 動画のコーデック（ASCII の小文字にそろえた値。`hevc`・`h264` など）。
    pub codecs: Vec<FacetValue>,
}

impl From<genzo_catalog::Facets> for Facets {
    fn from(f: genzo_catalog::Facets) -> Self {
        let values = |v: Vec<(String, u64)>| {
            v.into_iter()
                .map(|(value, count)| FacetValue { value, count })
                .collect()
        };
        Self {
            cameras: values(f.cameras),
            lenses: values(f.lenses),
            codecs: values(f.codecs),
        }
    }
}

/// variant の位置（[`crate::Core::index_of`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexOf {
    /// 世代。
    pub generation: u64,
    /// 位置（結果になければ `None`）。
    pub index: Option<u64>,
}

/// 表示中の検索結果に出ている写真（[`crate::Core::shown_variants`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownVariants {
    /// 世代。
    pub generation: u64,
    /// 問い合わせた写真のうち、結果に出ているもの（問い合わせた順。同じ写真は 1 つ）。
    pub variant_ids: Vec<VariantId>,
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
    /// キャプション（[`VariantDetails::metadata`] の `caption` と同じ。MVP からの項目）。
    pub caption: Option<String>,
    /// カタログの中で編集した説明のメタデータ（LIB-15。タイトル・キャプション・作成者・著作権など。asset に
    /// 付き、仮想コピーも同じ値）。
    #[serde(default)]
    pub metadata: IptcMetadata,
    /// 動画の情報。
    pub video: Option<VideoMetadata>,
    /// ファイル（主となるファイルが先）。
    pub files: Vec<FileInfo>,
    /// 同じ写真のマスターと仮想コピー（LIB-13。マスターが先、仮想コピーは作った順。自分を含む）。
    #[serde(default)]
    pub variants: Vec<VariantBrief>,
    /// 入っているスタック（LIB-12。入っていなければ `null`）。
    #[serde(default)]
    pub stack: Option<StackInfo>,
}

/// 複数の写真の説明のメタデータのまとめ（LIB-15 の一括編集。[`crate::Core::metadata_summary`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MetadataSummary {
    /// 対象の asset の数（同じ asset の仮想コピーは 1 つに数える。存在しないものは数えない）。
    pub assets: u64,
    /// 項目ごとのまとめ（genzo-model の `MetadataField::ALL` の順）。
    pub fields: Vec<MetadataFieldState>,
}

/// 1 つの項目のまとめ（[`MetadataSummary`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataFieldState {
    /// 項目。
    pub field: MetadataField,
    /// すべての写真で同じ値ならその値（すべてなしなら `null`。値が異なれば `null`）。
    pub value: Option<String>,
    /// 写真によって値が異なるか（値のあるものとないものが混ざる場合を含む。画面は「複数の値」と出す）。
    pub mixed: bool,
}

impl From<genzo_catalog::MetadataSummary> for MetadataSummary {
    fn from(s: genzo_catalog::MetadataSummary) -> Self {
        Self {
            assets: s.assets,
            fields: s
                .fields
                .into_iter()
                .map(|f| MetadataFieldState {
                    field: f.field,
                    value: f.value,
                    mixed: f.mixed,
                })
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// キーワード・コレクション（LIB-09〜11）
// ---------------------------------------------------------------------------

/// キーワードの一覧の 1 件（[`crate::Core::keyword_tree`]。画面は `parent_id` で木にする）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeywordInfo {
    /// キーワード。
    pub keyword_id: KeywordId,
    /// 親（最上位は `null`）。
    pub parent_id: Option<KeywordId>,
    /// 名前。
    pub name: String,
    /// 書き出すファイルのメタデータ（XMP の `dc:subject`）に書くか。
    pub include_on_export: bool,
    /// このキーワードが直接付いている写真（variant）の数（子のキーワードの分は数えない）。
    pub photo_count: u64,
}

impl From<genzo_catalog::KeywordNode> for KeywordInfo {
    fn from(k: genzo_catalog::KeywordNode) -> Self {
        Self {
            keyword_id: k.id,
            parent_id: k.parent_id,
            name: k.name,
            include_on_export: k.include_on_export,
            photo_count: k.photo_count,
        }
    }
}

/// 名前で探したキーワード（[`crate::Core::find_keywords`]。入力欄の補完）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeywordHit {
    /// キーワード。
    pub keyword_id: KeywordId,
    /// 名前。
    pub name: String,
    /// 最上位から自分までの名前（`["場所", "京都"]`）。
    pub path: Vec<String>,
}

/// 選択中の写真に付いているキーワード（[`crate::Core::variant_keywords`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantKeyword {
    /// キーワード。
    pub keyword_id: KeywordId,
    /// 名前。
    pub name: String,
    /// 最上位から自分までの名前。
    pub path: Vec<String>,
    /// このキーワードが付いている写真の数（[`VariantKeywords::variants`] より少なければ一部の写真だけ）。
    pub count: u64,
}

/// 選択中の写真のキーワードのまとめ（[`crate::Core::variant_keywords`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VariantKeywords {
    /// 対象の写真の数（重複を除く）。
    pub variants: u64,
    /// キーワード（名前の順）。
    pub keywords: Vec<VariantKeyword>,
}

/// コレクションへの参照（ID と名前）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionRef {
    /// コレクション。
    pub collection_id: CollectionId,
    /// 名前。
    pub name: String,
}

/// キーワードの使われ方（削除の確認。[`crate::Core::keyword_usage`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KeywordUsage {
    /// このキーワードが直接付いている写真の数。
    pub photos: u64,
    /// このキーワードか子孫のキーワードが付いている写真の数（削除で付与が消える写真の数）。
    pub photos_with_descendants: u64,
    /// 子孫のキーワードの数（削除で一緒に消える数）。
    pub descendants: u64,
    /// このキーワードか子孫を条件に使っているスマートコレクション（削除するとその条件はどの写真にも
    /// 一致しなくなる）。
    pub smart_collections: Vec<CollectionRef>,
}

impl From<genzo_catalog::KeywordUsage> for KeywordUsage {
    fn from(u: genzo_catalog::KeywordUsage) -> Self {
        Self {
            photos: u.photos,
            photos_with_descendants: u.photos_with_descendants,
            descendants: u.descendants,
            smart_collections: u
                .smart_collections
                .into_iter()
                .map(|(collection_id, name)| CollectionRef {
                    collection_id,
                    name,
                })
                .collect(),
        }
    }
}

/// コレクションの一覧の 1 件（[`crate::Core::collections`]。画面は `parent_id` で木にする）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionInfo {
    /// コレクション。
    pub collection_id: CollectionId,
    /// 親のコレクションセット（最上位は `null`）。
    pub parent_id: Option<CollectionId>,
    /// 名前。
    pub name: String,
    /// 種類（`manual`・`smart`・`set`）。
    pub kind: CollectionKind,
    /// スマートコレクションの条件（読めた場合）。
    pub rule: Option<SmartRule>,
    /// スマートコレクションの条件を読めなかった理由（アプリより新しい版など。その間はどの写真にも一致しない）。
    pub rule_error: Option<String>,
    /// 写真の数（手動のコレクションはメンバーの数。スマートコレクション・セットは検索で数えた数。数えなかった・
    /// 数えられなかったときは `null`）。
    pub count: Option<u64>,
}

/// ファイルの情報。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    /// ファイル。
    pub file_id: FileId,
    /// 役割。
    pub role: FileRole,
    /// 絶対パス（マウント先が分からなければ `None`）。
    #[serde(serialize_with = "lossy_path")]
    pub path: Option<PathBuf>,
    /// 大きさ（バイト）。
    pub size: u64,
    /// 状態。
    pub status: FileStatus,
    /// missing / error の理由。
    pub status_reason: Option<String>,
    /// ファイル全体の BLAKE3（ORG-03。まだ計算していなければ `None`）。
    #[serde(default)]
    pub full_hash: Option<String>,
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
    #[serde(serialize_with = "lossy_path")]
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
    #[serde(serialize_with = "lossy_path")]
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
    /// 幅（向きを反映する前。RAW は既定の切り抜き（DefaultCrop）の幅で、利用者の切り抜き
    /// （`CropRect`）の割合の基準。既定の切り抜きのない RAW は有効画素の幅）。
    pub width: u32,
    /// 高さ（向きを反映する前。幅と同じく、RAW は既定の切り抜きの高さ）。
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

/// WB のスポイトの結果（[`crate::Core::white_balance_at`]。DEV-03）。`WhiteBalance::Custom` にそのまま
/// 使える（適用はしない。UI が `set_settings` する）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PickedWhiteBalance {
    /// 色温度（K。設定できる範囲 2000〜50000 K に丸めた値）。
    pub temperature_k: f32,
    /// 色かぶり補正（tint。設定できる範囲 ±150 に丸めた値）。
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
    /// ビフォー / アフターの表示のビフォー（DEV-31。[`crate::Core::set_before`]。設定していなければ
    /// `None`。写真を開き直すと `None` に戻る）。
    #[serde(default)]
    pub before: Option<BeforeInfo>,
}

/// ビフォー / アフターの表示（DEV-31）のビフォーの設定の出どころ（[`crate::Core::set_before`]）。
///
/// どの出どころでも、切り抜き・角度補正（`geometry`）は今の設定（アフター）のものを使う（位置を合わせて
/// 比べ、拡大・パンを同期するため。決定事項ログ No.79）。後のパッケージ（スナップショット・履歴。DEV-28）は
/// `History`・`Snapshot` を画面から選べるようにする。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BeforeSource {
    /// variant を作ったときの設定（履歴の最初の 1 件。マスターは「読み込み」= 取り込んだ直後の既定の設定、
    /// 仮想コピーは「仮想コピーの作成」のときの設定）。既定。
    Initial,
    /// 履歴の 1 件（[`HistoryItem::id`]）の設定。
    History {
        /// 履歴の項目。
        entry_id: HistoryEntryId,
    },
    /// スナップショット（DEV-28）の設定。
    Snapshot {
        /// スナップショット。
        snapshot_id: SnapshotId,
    },
    /// 呼んだ時点の今の設定（アフター）の写し（Lightroom の「アフターの設定をビフォーにコピー」）。
    Current,
}

/// ビフォーの状態（[`crate::Core::set_before`]。ピクセルは [`crate::Core::before_frame`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeforeInfo {
    /// variant。
    pub variant_id: VariantId,
    /// 出どころ。
    pub source: BeforeSource,
    /// 表示名（「読み込み」「履歴: 露光量 +0.30」「スナップショット: 白黒」など）。
    pub label: String,
    /// ビフォーの設定の処理バージョン（今の設定と違うことがある）。
    pub process_version: u32,
    /// 最後に要求したビフォーの描画の世代（[`crate::Event::BeforeUpdated`] の世代と比べる）。
    pub generation: u64,
}

/// ビフォーのプレビュー（ピクセルを含む。[`crate::Core::before_frame`]。DEV-31）。
#[derive(Debug, Clone)]
pub struct BeforeFrame {
    /// 情報（ヒストグラムは求めない（空）。寸法・出力の寸法の意味は現像のプレビューと同じ）。
    pub info: FrameInfo,
    /// ステージ 17a の出力を 8bit にしたもの（R, G, B の並び、行優先）。
    pub display_rgb8: Arc<Vec<u8>>,
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
    /// 切り抜く前の全体（G の外接矩形）を描いたものか（切り抜きの編集中の表示。
    /// [`crate::Core::set_develop_view`]）。真なら、画像は設定の切り抜きを使わず G の外接矩形の全体で、
    /// 回転で画像の外になった四隅は黒（0）で塗ってある（ヒストグラムには数えない）。UI は、この画像の上に
    /// 設定の切り抜き（`CropRect`。G の外接矩形の各軸の割合）をそのまま重ねられる。
    #[serde(default)]
    pub uncropped: bool,
    /// この描画の設定での出力の画像（C。切り抜いた後）のフル解像度の幅（`uncropped` なら G の外接矩形の
    /// フル解像度の幅）。表示範囲の描画（[`crate::Core::render_region`] の [`RegionSpace::Output`]）の座標・
    /// [`RegionInfo::full_width`] と同じ格子。求められなければ 0。
    #[serde(default)]
    pub output_width: u32,
    /// 出力の画像のフル解像度の高さ（`output_width` と同じ）。
    #[serde(default)]
    pub output_height: u32,
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

/// 表示範囲の座標系（04 の 2.7 節。[`crate::Core::render_region`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionSpace {
    /// C：出力座標（切り抜いた後の画像。フル解像度の画素の単位。左上が (0, 0)）。切り抜く前の全体の表示
    /// （[`crate::Core::set_develop_view`]）の間は、G の外接矩形の全体の画像の座標（[`FrameInfo::uncropped`]）。
    #[default]
    Output,
    /// G：ジオメトリ補正後の全体（切り抜く前の画像全体）。G の外接矩形の長辺を 1 とした値。
    Geometry,
}

/// 表示範囲の描画の要求（ルーペの等倍表示 PRV-03・拡大とパン LIB-02。[`crate::Core::render_region`]）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RegionRequest {
    /// 現像中の写真（[`crate::Core::open_develop`] で開いたもの）。
    pub variant_id: VariantId,
    /// 範囲の座標系。
    #[serde(default)]
    pub space: RegionSpace,
    /// 範囲の左上の x。
    pub x: f64,
    /// 範囲の左上の y。
    pub y: f64,
    /// 範囲の幅。
    pub width: f64,
    /// 範囲の高さ。
    pub height: f64,
    /// 表示の倍率（出力の 1 画素を画面の何画素で表すか。1.0 が等倍、0.5 が縮小、2.0 が拡大）。
    #[serde(default = "default_region_scale")]
    pub scale: f64,
}

fn default_region_scale() -> f64 {
    1.0
}

/// 表示範囲の描画の結果の情報（ピクセルを除く）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionInfo {
    /// variant。
    pub variant_id: VariantId,
    /// 描いた現像設定の世代（[`crate::Core::set_settings`] が返した値。古い世代の結果は捨ててよい）。
    pub generation: u64,
    /// 要求の通しの番号（新しい要求ほど大きい）。
    pub request: u64,
    /// 描いた範囲（C の格子、フル解像度の画素。出力の画像の内側に収めたもの）の左上の x。
    pub x: u32,
    /// 描いた範囲の左上の y。
    pub y: u32,
    /// 描いた範囲の幅（フル解像度の画素）。
    pub region_width: u32,
    /// 描いた範囲の高さ（フル解像度の画素）。
    pub region_height: u32,
    /// 出力の画像全体の寸法（C の格子、フル解像度）。
    pub full_width: u32,
    /// 出力の画像全体の高さ。
    pub full_height: u32,
    /// 表示の倍率（要求の値）。
    pub scale: f64,
    /// 返した画像の幅（範囲の幅 × 倍率を四捨五入。1 以上）。
    pub width: u32,
    /// 返した画像の高さ。
    pub height: u32,
    /// 処理したタイルの数。
    pub tiles: u64,
    /// 画面の出力を sRGB とみなしたか（IQ-05）。
    pub display_assumed_srgb: bool,
    /// 現像の警告。
    pub warnings: Vec<String>,
    /// ビフォー（[`crate::Core::set_before`] の設定）を描いたものか（[`crate::Core::render_before_region`]。
    /// DEV-31）。偽なら今の設定（アフター）。
    #[serde(default)]
    pub before: bool,
}

/// 表示範囲の描画の結果（ピクセルを含む。[`crate::Core::render_region`]）。
#[derive(Debug, Clone)]
pub struct RegionFrame {
    /// 情報。
    pub info: RegionInfo,
    /// ステージ 17a の出力（モニターのプロファイルで符号化した値）を 8bit にしたもの（R, G, B の並び、
    /// 行優先。寸法は `info.width` × `info.height`）。
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
        #[serde(serialize_with = "lossy_path")]
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
        #[serde(serialize_with = "lossy_path")]
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
    #[serde(serialize_with = "lossy_path")]
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
    #[serde(serialize_with = "lossy_path")]
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
    #[serde(serialize_with = "lossy_path")]
    pub trashed_files: Vec<PathBuf>,
    /// ゴミ箱へ移す対象だったが、元の場所になかった（フォルダはあり、アプリの外で削除・移動された）
    /// ため、移さなかったファイル（その asset はカタログから除いた）。
    #[serde(default, serialize_with = "lossy_path")]
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
    #[serde(serialize_with = "lossy_path")]
    pub preview_cache_dir: PathBuf,
    /// L1 プレビューのキャッシュの上限（バイト。SCL-04）。
    pub preview_cache_bytes: u64,
    /// 書き出しの既定の色空間。
    pub default_export_color_space: OutputColorSpace,
    /// 既定のタイムゾーンのオフセット（分。UTC より東が正）。撮影日時にオフセットがない写真・動画の
    /// UTC の推定に使う（3.1 節）。
    pub default_utc_offset_minutes: i32,
    /// 取り込みの既定: 重複と思われるもの（内容が同じファイルが登録済みのもの）を取り込まない（IMP-04。既定は
    /// `true`）。
    #[serde(default = "default_true")]
    pub import_skip_duplicates: bool,
    /// 取り込みの既定: RAW と同時記録の JPEG を別々の写真として取り込む（IMP-06。既定は `false` = 1 枚に
    /// まとめる）。
    #[serde(default)]
    pub import_separate_raw_jpeg: bool,
    /// ファイルの内容のハッシュ（全体）をバックグラウンドで計算する（ORG-03。既定は `true`。アプリが起動の
    /// 設定で許したときだけ動く。CLI は動かさない）。
    #[serde(default = "default_true")]
    pub hash_in_background: bool,
    /// バックグラウンドの計算で、ネットワークのボリューム（SMB・NFS など）のファイルも読む（既定は `false`）。
    #[serde(default)]
    pub hash_network_volumes: bool,
}

fn default_true() -> bool {
    true
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
    /// 取り込みの既定: 重複と思われるものを取り込まない。
    pub import_skip_duplicates: Option<bool>,
    /// 取り込みの既定: RAW と同時記録の JPEG を別々の写真として取り込む。
    pub import_separate_raw_jpeg: Option<bool>,
    /// ファイルの内容のハッシュをバックグラウンドで計算する（`false` にすると実行中の計算を取り消す）。
    pub hash_in_background: Option<bool>,
    /// バックグラウンドの計算で、ネットワークのボリュームのファイルも読む。
    pub hash_network_volumes: Option<bool>,
}

/// バックアップ（DATA-04）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupEntry {
    /// ファイル。
    #[serde(serialize_with = "lossy_path")]
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
    /// スタックの数（LIB-12）。
    #[serde(default)]
    pub stacks: u64,
}

/// カタログの構造（ORG-07。[`crate::Core::catalog_schema`]。CLI の `catalog schema`、画面の「カタログの構造」）。
///
/// 文書は docs/catalog_schema.md。外部から SQL で読むときは、公開ビュー（[`CatalogSchema::public_views`]）を
/// すすめる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSchema {
    /// スキーマの版（`PRAGMA user_version`）。
    pub schema_version: u32,
    /// `PRAGMA application_id`（GenzoParis のカタログの印。ASCII の `GZCT`）。
    pub application_id: i32,
    /// 公開ビューの名前（列の名前・意味を変えない約束のビュー）。
    pub public_views: Vec<String>,
    /// 表・ビュー（名前の順。SQLite の内部の表と FTS5 の中身の表は除く）。
    pub tables: Vec<SchemaTable>,
    /// 表・ビュー・索引・トリガーと、それを作った SQL（種類・名前の順）。
    pub objects: Vec<SchemaObject>,
}

/// カタログの表・ビュー（[`CatalogSchema::tables`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaTable {
    /// 名前。
    pub name: String,
    /// 種類（`table`・`virtual`（FTS5）・`view`）。
    pub kind: String,
    /// 列（定義の順）。
    pub columns: Vec<SchemaColumn>,
}

/// 表・ビューの列。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaColumn {
    /// 名前。
    pub name: String,
    /// 宣言した型（ビューの式の列などでは空）。
    pub decl_type: String,
    /// `NOT NULL` か。
    pub not_null: bool,
    /// 既定値（SQL の式）。
    pub default_value: Option<String>,
    /// 主キーの何番目の列か（主キーでなければ 0）。
    pub primary_key: u32,
}

/// 表・ビュー・索引・トリガー（[`CatalogSchema::objects`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaObject {
    /// 種類（`table`・`view`・`index`・`trigger`）。
    pub kind: String,
    /// 名前。
    pub name: String,
    /// 対象の表。
    pub table: String,
    /// 作った SQL。
    pub sql: String,
}

impl From<genzo_catalog::SchemaDescription> for CatalogSchema {
    fn from(d: genzo_catalog::SchemaDescription) -> Self {
        Self {
            schema_version: d.schema_version,
            application_id: d.application_id,
            public_views: genzo_catalog::PUBLIC_VIEWS
                .iter()
                .map(|v| (*v).to_owned())
                .collect(),
            tables: d
                .tables
                .into_iter()
                .map(|t| SchemaTable {
                    name: t.name,
                    kind: t.kind,
                    columns: t
                        .columns
                        .into_iter()
                        .map(|c| SchemaColumn {
                            name: c.name,
                            decl_type: c.decl_type,
                            not_null: c.not_null,
                            default_value: c.default_value,
                            primary_key: c.primary_key,
                        })
                        .collect(),
                })
                .collect(),
            objects: d
                .objects
                .into_iter()
                .map(|o| SchemaObject {
                    kind: o.kind,
                    name: o.name,
                    table: o.table,
                    sql: o.sql,
                })
                .collect(),
        }
    }
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
