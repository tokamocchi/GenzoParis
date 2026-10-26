//! コマンドの引数・応答・イベントで使う型（serde の素直な形。将来 TypeScript の型を生成するため、
//! 列挙は `snake_case` の文字列か `type` / `status` で種類を表すオブジェクトにする）。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Utc};
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
    /// 写真の移動（FILE-02。別のボリュームへはコピーして照合し、元をゴミ箱へ移す）。
    MoveFiles,
    /// 見つからない写真の確認（FILE-04。すべてのファイルの存在を確かめて印を付ける。内容のハッシュの計算と
    /// 同じ 1 本のスレッド）。
    CheckMissing,
    /// 再リンクの候補の探索（FILE-04。利用者が選んだフォルダから、見つからないファイルと同じものを探す）。
    FindRelinkCandidates,
    /// 動画の再生用のファイルの作成（VID-04。WebView で再生できない動画を、利用者の ffmpeg で変換する。専用の
    /// 1 本のスレッドで、1 度に 1 本だけ）。
    VideoProxy,
    /// 比較明・比較暗合成（DEV-34。docs/composite_design.md）。
    Composite,
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
        /// 削除した動画の再生用のファイル（VID-04）の数（上限容量を超えた分と、カタログにない・内容が変わった
        /// ファイルのもの。生成の途中で終了した一時ファイルは `temp_files_removed` に数える）。
        #[serde(default)]
        video_proxies_removed: u64,
    },
    /// ファイルの内容のハッシュの計算。
    HashFiles(HashReport),
    /// 写真の移動。
    MoveFiles(FileMoveReport),
    /// 見つからない写真の確認。
    CheckMissing(MissingCheckReport),
    /// 再リンクの候補の探索。
    RelinkCandidates(RelinkSearch),
    /// 動画の再生用のファイルの作成。
    VideoProxy(VideoProxyReport),
    /// 比較明・比較暗合成。
    Composite(CompositeReport),
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
    /// カメラの本体のシリアル番号（LIB-16 の機材の区別。記録しないカメラ・版 11 より前に取り込んだものは
    /// `null`）。
    #[serde(default)]
    pub camera_serial: Option<String>,
    /// ISO 感度。
    pub iso: Option<u32>,
    /// 絞り値。
    pub aperture: Option<f64>,
    /// シャッター速度（秒）。
    pub shutter_s: Option<f64>,
    /// 焦点距離（mm）。
    pub focal_mm: Option<f64>,
    /// 使う位置（LIB-21。後から付けた位置があればそれ、なければカメラ（ファイル）の位置。書き出しの Exif に
    /// 入る位置と同じ）。
    pub gps: Option<GpsCoord>,
    /// `gps` の出どころ（LIB-21。`camera`・`assigned`。位置がなければ `null`。LIB-21 より前のコアには無い）。
    #[serde(default)]
    pub gps_source: Option<GpsSource>,
    /// カメラ（ファイル）が記録した位置（LIB-21。後から付けた位置を使っていても、ファイルの値。なければ
    /// `null`。LIB-21 より前のコアには無い）。
    #[serde(default)]
    pub camera_gps: Option<GpsCoord>,
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
    /// 絶対パス（マウント先が分からなければ `None`。つながっていないボリュームは、最後に確かめたマウント先からの
    /// パスで、表示用）。
    #[serde(serialize_with = "lossy_path")]
    pub path: Option<PathBuf>,
    /// ボリュームがつながっているか（FILE-03。つながっていないボリュームのフォルダは「オフライン」と表示する）。
    #[serde(default = "default_true")]
    pub online: bool,
    /// フォルダが見つからない（ボリュームはつながっているのに、フォルダがない。FILE-04）。
    #[serde(default)]
    pub missing: bool,
    /// このフォルダ（配下は含まない）の見つからないファイルの数。
    #[serde(default)]
    pub missing_files: u64,
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
// 動画の再生（VID-04）
// ---------------------------------------------------------------------------

/// 動画の再生の情報（[`crate::Core::video_playback`]）。UI は、元のファイルを WebView で再生できるか
/// （`canPlayType`・再生の失敗）を確かめ、再生できなければ再生用のファイルを使う（なければ作る）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoPlayback {
    /// variant。
    pub variant_id: VariantId,
    /// 元のファイルを読めるか（見つからない・ドライブが外れているなら `false`）。
    pub available: bool,
    /// 読めない理由（`available` が `false` のとき）。
    pub unavailable_reason: Option<String>,
    /// 元のファイルの MIME の型（`video/mp4`・`video/quicktime`）。
    pub content_type: String,
    /// 元のファイルの大きさ（バイト。読めなければ `None`）。
    pub size: Option<u64>,
    /// 動画の情報（ffprobe で読めなかったものは `None`）。
    pub video: Option<VideoMetadata>,
    /// 今の内容の再生用のファイル（なければ `None`）。
    pub proxy: Option<VideoProxyInfo>,
    /// この動画の再生用のファイルを作っているジョブ（なければ `None`）。
    pub proxy_job: Option<u64>,
    /// 再生用のファイルを作れない理由（ffmpeg がない・H.264 のエンコーダーがない。作れるなら `None`）。
    pub proxy_unavailable: Option<String>,
    /// 再生用のファイルを作るのに使うエンコーダー（ffmpeg の名前。作れないなら `None`）。
    pub proxy_encoder: Option<String>,
}

/// 再生用のファイル（H.264 ＋ AAC の MP4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoProxyInfo {
    /// 大きさ（バイト）。
    pub size: u64,
    /// 長辺の上限（画素。genzo-media の `PROXY_LONG_EDGE`）。
    pub long_edge: u32,
}

/// 配信する動画のファイル（[`crate::Core::video_stream`]。アプリの `genzo://` の URI スキームが使う）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoStream {
    /// variant。
    pub variant_id: VariantId,
    /// ファイル（カタログに登録された動画の元のファイルか、キャッシュの再生用のファイル）。
    #[serde(serialize_with = "lossy_path")]
    pub path: PathBuf,
    /// 大きさ（バイト。確かめた時点）。
    pub size: u64,
    /// MIME の型。
    pub content_type: String,
    /// 再生用のファイルか。
    pub proxy: bool,
}

/// 再生用のファイルの作成の結果（[`JobResult::VideoProxy`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoProxyReport {
    /// variant。
    pub variant_id: VariantId,
    /// 再生用のファイルの大きさ（バイト）。
    pub size: u64,
    /// すでにあったので作らなかったか。
    pub reused: bool,
    /// 使ったエンコーダー（ffmpeg の名前。作らなかったら `None`）。
    pub encoder: Option<String>,
}

/// 再生用のファイルのキャッシュの状況（[`crate::Core::video_proxy_cache`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoProxyCacheStatus {
    /// フォルダ（データのフォルダの `video-proxies`）。
    #[serde(serialize_with = "lossy_path")]
    pub dir: PathBuf,
    /// 再生用のファイルの数。
    pub count: u64,
    /// 合計の大きさ（バイト）。
    pub total_bytes: u64,
    /// 上限（バイト）。
    pub capacity_bytes: u64,
    /// この操作で消した数（[`crate::Core::clear_video_proxies`]。状況を読むだけなら 0）。
    pub removed: u64,
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
    /// RAW に埋め込まれたレンズ補正の情報（DEV-14。Sony の ARW のタグ・DNG の opcode）の要約。
    /// 情報がない・RAW 以外は `None`（手動の補正だけを使える）。
    #[serde(default)]
    pub lens_correction: Option<LensCorrectionInfo>,
    /// 埋め込みのレンズ補正の情報が読めない・不正だったため使わなかった理由（DEV-14。genzo-raw の
    /// `RawDetails::lens_correction_note`。そのときは `lens_correction` は `None`）。画面は「情報が壊れていたため
    /// 使えません」と、もともとない場合と区別して出す。読めた・なかった・RAW 以外は `None`。
    #[serde(default)]
    pub lens_correction_note: Option<String>,
    /// 機種（メーカーと機種の名前。genzo-model の `PhotoMetadata::camera_name`。RAW だけ）。カメラプロファイル（DCP）の
    /// UniqueCameraModel と比べて、この機種用のプロファイルを選ぶ（DEV-17。[`ProfileEntry::matches_camera`]）。
    /// 分からない・RAW 以外は `None`。
    #[serde(default)]
    pub camera: Option<String>,
}

/// 埋め込みのレンズ補正の情報の要約（[`SourceInfo::lens_correction`]。画面のレンズ補正のパネルの表示用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensCorrectionInfo {
    /// 出どころ（`"sony_arw"`・`"dng_opcode"`）。
    pub source: String,
    /// 説明（例: `"Sony の ARW（歪曲・周辺減光・色収差）"`）。
    pub description: String,
    /// 歪曲の情報があるか。
    pub distortion: bool,
    /// 周辺減光の情報があるか。
    pub vignetting: bool,
    /// 倍率色収差の情報があるか。
    pub chromatic_aberration: bool,
}

impl LensCorrectionInfo {
    /// 埋め込みの情報から作る。
    pub fn of(l: &genzo_raw::LensCorrectionData) -> Self {
        Self {
            source: l.source_id().to_owned(),
            description: l.describe(),
            distortion: l.has_distortion(),
            vignetting: l.has_vignetting(),
            chromatic_aberration: l.has_chromatic_aberration(),
        }
    }
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

/// 色域の範囲（DEV-19）のスポイトの結果（[`crate::Core::mask_color_at`]）。genzo-model の `ColorSample` にそのまま
/// 使える（適用はしない。UI が `set_settings` する）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PickedMaskColor {
    /// 点の G 座標の x（04 の 2.7 節。切り抜く前の全体の外接矩形の長辺を 1 とする）。
    pub x: f32,
    /// 点の G 座標の y。
    pub y: f32,
    /// 点のまわりの平均の色の OKLab（ステージ 9 の出力。露光量の前のシーンリニアの値から）。
    pub lab: [f32; 3],
}

/// 自動補正（DEV-32。[`crate::Core::auto_tone`]）の結果。露光量・コントラスト・ハイライト・シャドウ・白レベル・
/// 黒レベルの値で、そのまま設定に入れられる（適用はしない。UI・CLI が `set_settings` する。
/// [`AutoTone::apply`]）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AutoTone {
    /// 露光量（EV。0.01 段に丸めた値。±5 の範囲）。
    pub exposure_ev: f32,
    /// コントラスト（整数。0 以上）。
    pub contrast: f32,
    /// ハイライト（整数。0 以下）。
    pub highlights: f32,
    /// シャドウ（整数。0 以上）。
    pub shadows: f32,
    /// 白レベル（整数。±100）。
    pub whites: f32,
    /// 黒レベル（整数。±100）。
    pub blacks: f32,
    /// 決めるのに使った代表の明るさ（確認・CLI の表示用）。
    pub stats: AutoToneStats,
}

impl AutoTone {
    /// 設定に入れる（6 つの値だけを置き換える。ほかの項目は変えない）。
    pub fn apply(&self, settings: &mut DevelopSettings) {
        settings.exposure_ev = self.exposure_ev;
        settings.contrast = self.contrast;
        settings.tone.highlights = self.highlights;
        settings.tone.shadows = self.shadows;
        settings.tone.whites = self.whites;
        settings.tone.blacks = self.blacks;
    }
}

/// 自動補正に使った代表の明るさ（露光量の前のシーンリニアの輝度の log2。genzo-pipeline の `AutoToneStats`）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AutoToneStats {
    /// 数えた画素の数（プレビューの、画像の上の画素）。
    pub samples: u64,
    /// 黒の点（下から 0.5%）。
    pub black_log2: f32,
    /// 中央値。
    pub median_log2: f32,
    /// 白の点（下から 99.5%）。
    pub white_log2: f32,
}

/// マスクの重ねて表示（O キー。DEV-18）の画像の情報（[`crate::Core::mask_overlay`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskOverlayInfo {
    /// variant。
    pub variant_id: VariantId,
    /// 求めたときの設定の世代。
    pub generation: u64,
    /// マスクの番号（設定の `masks` の位置）。
    pub index: usize,
    /// 幅（同じ表示のプレビュー（[`FrameInfo`]）と同じ格子）。
    pub width: u32,
    /// 高さ。
    pub height: u32,
    /// 切り抜く前の全体の表示の画像か（[`FrameInfo::uncropped`]）。
    pub uncropped: bool,
}

/// AI マスク（DEV-20）の計算の結果（[`crate::Core::compute_ai_mask`]）。`shape` をマスクの部品の形として設定に入れる
/// （適用はしない。UI・CLI が `set_settings` する。色域のスポイトと同じ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiMaskResult {
    /// 写真。
    pub variant_id: VariantId,
    /// 種類。
    pub kind: genzo_model::AiMaskKind,
    /// 設定に入れる形（`MaskShape::Ai`。ラスターの参照・大きさ・G 座標の範囲）。
    pub shape: genzo_model::MaskShape,
    /// 使ったモデルの ID。
    pub model: String,
    /// ラスターの内容の SHA-256（カタログの `ai_mask` の行）。
    pub sha256: String,
    /// ラスターの幅（画素）。
    pub width: u32,
    /// ラスターの高さ（画素）。
    pub height: u32,
    /// 推論に渡す画像の描画にかかった時間（ミリ秒）。
    pub render_ms: u64,
    /// 推論にかかった時間（ミリ秒。前処理・後処理を含む）。
    pub inference_ms: u64,
}

/// AI マスクの推論の仕組みの状態（[`crate::Core::ai_mask_status`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMaskStatusInfo {
    /// 使えるか（使える種類が 1 つ以上ある）。
    pub available: bool,
    /// 使える種類。
    pub kinds: Vec<genzo_model::AiMaskKind>,
    /// 種類ごとに使うモデルの ID（`kinds` の順）。
    pub models: Vec<String>,
    /// 推論の backend（例: `onnxruntime ...`。使えなければ空）。
    pub backend: String,
    /// 使えない理由・読めなかった台帳（使えるなら空）。
    pub problems: Vec<String>,
    /// モデルのフォルダ（分からなければ `None`）。
    pub models_dir: Option<PathBuf>,
    /// 配布物に同梱したモデルのフォルダ（なければ `None`）。利用者のフォルダの後に読む。
    pub bundled_models_dir: Option<PathBuf>,
    /// 見つかった ONNX Runtime の共有ライブラリ（なければ `None`）。
    pub onnxruntime: Option<PathBuf>,
    /// カタログの中の AI マスクのラスターの数。
    pub rasters: u64,
    /// カタログの中の AI マスクのラスターのバイト数の合計（符号化したもの）。
    pub raster_bytes: u64,
}

/// 参照のない AI マスクのラスターの回収の結果（[`crate::Core::collect_ai_masks`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMaskCollectReport {
    /// 消した数。
    pub removed: u64,
    /// 残った数。
    pub remaining: u64,
    /// 残ったラスターのバイト数の合計。
    pub remaining_bytes: u64,
}

/// マスクの重ねて表示の画像（重み 0〜1 を 0〜255 にしたもの。行優先）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskOverlayFrame {
    /// 情報。
    pub info: MaskOverlayInfo,
    /// 重み（幅 × 高さ。0〜255）。
    pub weights: Vec<u8>,
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
    /// このアプリの最新の処理バージョン（genzo-model の `CURRENT_PROCESS_VERSION`）。`settings` の
    /// 処理バージョンがこれより古ければ、画面は「処理バージョンを更新」を出す
    /// （[`crate::Core::upgrade_process_version`]。DATA-09）。
    #[serde(default)]
    pub latest_process_version: u32,
}

/// ビフォー / アフターの表示（DEV-31）のビフォーの設定の出どころ（[`crate::Core::set_before`]）。
///
/// どの出どころでも、切り抜き・角度補正（`geometry`）は今の設定（アフター）のものを使う（位置を合わせて
/// 比べ、拡大・パンを同期するため。決定事項ログ No.79）。`History`・`Snapshot` は、画面の履歴・スナップショットの
/// メニューの「ビフォーに設定」で選ぶ（DEV-28。決定事項ログ No.164）。
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

/// スナップショットの 1 件（DEV-28。[`crate::Core::snapshots`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// ID。
    pub id: SnapshotId,
    /// 写真（variant）。
    pub variant_id: VariantId,
    /// 名前。
    pub name: String,
    /// 作った日時。
    pub created_at: DateTime<Utc>,
    /// 保存した設定の処理バージョン（作ったときの写真の版。適用すると写真の版もこの版になる）。
    pub process_version: u32,
}

impl From<genzo_catalog::Snapshot> for SnapshotInfo {
    fn from(s: genzo_catalog::Snapshot) -> Self {
        Self {
            id: s.id,
            variant_id: s.variant_id,
            name: s.name,
            created_at: s.created_at,
            process_version: s.process_version,
        }
    }
}

/// スナップショットの出どころ（DEV-28。[`crate::Core::create_snapshot`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SnapshotSource {
    /// 今の状態（現像中の写真なら、保存待ちの変更を保存してから）。既定。
    #[default]
    Current,
    /// 履歴の 1 件（[`HistoryItem::id`]）の状態。
    History {
        /// 履歴の項目。
        entry_id: HistoryEntryId,
    },
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
    /// この画像の縁の G 座標（左・上・右・下。04 の 2.7 節の G 座標（切り抜く前の全体の外接矩形の長辺を 1 とする）。
    /// ローカル補正（マスク。DEV-18）の位置を画像の上に描くための対応: 画像の幅に対する割合 `fx` の G 座標の x は
    /// `左 + fx · (右 − 左)`（y も同じ）。古いコアの応答では欠ける。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub g_bounds: Option<[f64; 4]>,
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
    /// 現像中の写真（[`crate::Core::open_develop`] で開いたもの。比較表示の
    /// [`crate::Core::render_view_region`] では任意の写真）。
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
    /// 比較表示（[`crate::Core::render_view_region`]）で、現像中でない写真をカタログに保存した設定で描いたときは 0。
    pub generation: u64,
    /// 要求の通しの番号（新しい要求ほど大きい。比較表示は面ごとの番号）。
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
    /// 比較表示の面（LIB-03。[`crate::Core::render_view_region`] で描いたものだけ。0 から）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u32>,
}

/// 比較表示（LIB-03）の面で使う写真の、出力の画像（C。切り抜いた後）のフル解像度の大きさ
/// （[`crate::Core::view_output_size`]。等倍表示の準備）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewOutputSize {
    /// variant。
    pub variant_id: VariantId,
    /// 幅（フル解像度の画素）。
    pub width: u32,
    /// 高さ。
    pub height: u32,
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

// ---------------------------------------------------------------------------
// カメラプロファイル・ルック（DEV-17。`profiles` の doc）
// ---------------------------------------------------------------------------

/// ライブラリの項目の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// カメラプロファイル（DCP。現像設定の `profile.camera`）。
    Camera,
    /// ルック（3D LUT の .cube。現像設定の `profile.look`）。
    Look,
}

/// 取り込んだカメラプロファイル・ルック（[`crate::Core::list_profiles`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileEntry {
    /// 種類。
    pub kind: ProfileKind,
    /// ID（現像設定に保存する値。元のファイルの名前から拡張子を除いたもの）。
    pub id: String,
    /// 表示の名前（DCP の ProfileName・.cube の TITLE。なければ ID）。
    pub name: String,
    /// DCP の機種（UniqueCameraModel）。ルック・記録のない DCP は `None`。
    pub camera_model: Option<String>,
    /// 一覧を求めたときに渡した機種用か（カメラプロファイルだけ。機種を渡さなかった・ルックは `None`）。
    pub matches_camera: Option<bool>,
    /// 使えるか（写しがない・内容が一覧と違う・解析できないなら `false`）。
    pub usable: bool,
    /// 中身の説明（光源・マップの大きさ・LUT の大きさなど）。
    pub description: String,
    /// 使わない情報（DCP のトーンカーブ・露出のずれなど）の説明。
    pub notes: Vec<String>,
    /// 著作権の表示（DCP の ProfileCopyright。そのまま表示する）。
    pub copyright: Option<String>,
    /// 埋め込みの方針（DCP の ProfileEmbedPolicy の説明）。
    pub embed_policy: Option<String>,
    /// 取り込んだ元のファイルの名前。
    pub original_name: String,
    /// 内容の SHA-256（小文字の 16 進数 64 文字。`render_deps` に記録する値）。
    pub sha256: String,
    /// 大きさ（バイト）。
    pub size: u64,
    /// 取り込んだ日時（UTC。RFC 3339）。
    pub imported_at: String,
}

/// 取り込めなかったファイル。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileImportFailure {
    /// ファイル。
    pub path: PathBuf,
    /// 理由。
    pub message: String,
}

/// 取り込みの結果（[`crate::Core::import_profiles`]）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileImportReport {
    /// 新しく取り込んだもの。
    pub imported: Vec<ProfileEntry>,
    /// 同じ ID のものを置き換えたもの（使っている写真は、次に開いたときに「内容が違う」と知らせる）。
    pub replaced: Vec<ProfileEntry>,
    /// 同じ ID・同じ内容のものがあり、消えた・壊れた写しを書き直したもの（使えるようになった）。
    #[serde(default)]
    pub repaired: Vec<ProfileEntry>,
    /// 同じ ID・同じ内容のものがあったので何もしなかったもの。
    pub unchanged: Vec<ProfileEntry>,
    /// 取り込めなかったもの。
    pub failed: Vec<ProfileImportFailure>,
}

/// 設定の貼り付けで写す項目（DEV-30。DEV-29 の「一部の項目だけを適用」にも使う）。既定はすべて。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingGroups {
    /// ホワイトバランス。
    pub white_balance: bool,
    /// 露光量・コントラスト・ハイライト・シャドウ・白・黒・ハイライト復元（DEV-06）。
    pub basic_tone: bool,
    /// テクスチャ・明瞭度・かすみの除去（DEV-11）。
    pub presence: bool,
    /// トーンカーブ。
    pub tone_curve: bool,
    /// 色（彩度・自然な彩度・カラーミキサー・カラーグレーディング・白黒。DEV-08〜10・DEV-25）。
    pub color: bool,
    /// 切り抜き・角度補正。
    pub geometry: bool,
    /// レンズ補正。
    pub lens: bool,
    /// シャープ（DEV-12）・ノイズ軽減（DEV-13）。
    pub detail: bool,
    /// 効果（切り抜き後の周辺光量補正・粒子。DEV-24）。
    pub effects: bool,
    /// カメラプロファイル・ルック・ルックの適用量（DEV-17）。
    pub profile: bool,
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
        presence: true,
        tone_curve: true,
        color: true,
        geometry: true,
        lens: true,
        detail: true,
        effects: true,
        profile: true,
        masks: true,
    };

    /// 現像プリセット（DEV-29）を作るときの既定の項目: 切り抜きと角度・マスクを除くすべて（写真ごとに位置・構図が
    /// 違うことが多いため。画面の貼り付け・プリセットのダイアログの既定の選択と同じ。決定事項ログ No.161）。
    pub const PRESET_DEFAULT: Self = Self {
        geometry: false,
        masks: false,
        ..Self::ALL
    };

    /// すべての項目か。
    pub fn is_all(&self) -> bool {
        *self == Self::ALL
    }

    /// `target` に `source` の選んだ項目を写したもの。**処理バージョンと外部データ（`render_deps`）は、すべての
    /// 項目を写す場合も `target` のまま**（DATA-09: 処理バージョンは明示的な更新なしに変えない。版を変えるかは
    /// 貼り付けの側（[`PasteVersion`]）で決める）。
    pub fn merge(&self, source: &DevelopSettings, target: &DevelopSettings) -> DevelopSettings {
        if self.is_all() {
            return DevelopSettings {
                process_version: target.process_version,
                render_deps: target.render_deps.clone(),
                ..source.clone()
            };
        }
        let mut out = target.clone();
        if self.white_balance {
            out.white_balance = source.white_balance;
        }
        if self.basic_tone {
            out.exposure_ev = source.exposure_ev;
            out.contrast = source.contrast;
            out.tone = source.tone;
            out.highlight_recovery = source.highlight_recovery;
        }
        if self.presence {
            out.presence = source.presence;
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
        if self.effects {
            out.effects = source.effects;
        }
        if self.profile {
            out.profile = source.profile.clone();
        }
        if self.masks {
            out.masks = source.masks.clone();
        }
        out
    }
}

/// 設定の貼り付け（DEV-30）で、貼り付け先の処理バージョンをどうするか（DATA-09。
/// [`crate::Core::paste_settings_with`]）。
///
/// 処理バージョン 1 と 2 は、版 1 からある項目を同じ意味で使う（genzo-pipeline の
/// `ProcessVersion::shares_settings_with`）ので、版の違う写真の間でも、選んだ項目だけを写せる。版 2 にしかない
/// 項目（レンズ補正・ハイライト復元・ノイズ軽減）を版 1 の写真に写すと、値は保存するが、処理バージョンを更新
/// するまで使われない（[`PasteReport::needs_upgrade`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteVersion {
    /// 貼り付け先の版のまま（既定。新しいアルゴリズムは明示的に更新した写真だけに使う）。
    #[default]
    Keep,
    /// 貼り付け先が貼り付け元より古ければ、貼り付け元の版に更新する（利用者が貼り付けのダイアログ・CLI の
    /// `develop copy --upgrade-process-version` で選んだとき。明示的な更新）。新しい版の写真を古い版に
    /// 下げることはない。
    Upgrade,
    /// 貼り付け元の版にする（CLI の `develop set`: JSON に書いた版をそのまま保存する。古い版に下げることも
    /// ある）。
    Source,
}

/// 設定の貼り付け（DEV-30）の結果（[`crate::Core::paste_settings_with`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PasteReport {
    /// 適用した variant の数。
    pub count: u64,
    /// 処理バージョンを更新した（上げた）variant の数（[`PasteVersion::Upgrade`]・[`PasteVersion::Source`]）。
    pub upgraded: u64,
    /// 貼り付けた後の設定に、その写真の処理バージョンでは使われず、最新の版に更新すると使われる項目の値が
    /// ある variant の数（版 1 の写真に写したレンズ補正など。画面は処理バージョンの更新を案内する）。
    pub needs_upgrade: u64,
    /// 除いた動画の数（現像プリセットの適用。[`crate::Core::apply_develop_preset`]。設定の貼り付けでは常に 0）。
    #[serde(default)]
    pub skipped_videos: u64,
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
        /// 実際に使った外部データ（RAW のカメラ行列・デコーダ、カメラプロファイル・ルックのファイル。04 の 2.5 節。
        /// RAW 以外でプロファイル・ルックも使っていなければ `None`。ほかの結果より大きいので箱に入れる）。
        #[serde(default)]
        render_deps: Option<Box<genzo_model::RenderDeps>>,
    },
    /// 動画を再エンコードせずにコピーした（EXP-05）。
    Copied {
        /// コピーしたファイル（連番を付けた場合は付けた後の名前）。
        #[serde(serialize_with = "lossy_path")]
        path: PathBuf,
        /// 大きさ（バイト）。
        bytes: u64,
        /// 内容の BLAKE3（16 進数の小文字。元のファイルと、コピーを読み直した値が同じことを確かめてある）。
        full_hash: String,
        /// 警告（登録の後に元のファイルが変わっていた・上書きしなかったなど）。
        #[serde(default)]
        warnings: Vec<String>,
    },
    /// 書き出さなかった（衝突時の「スキップ」・書き出さない設定の動画など）。
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
    /// 書き出した数（写真）。
    pub written: u64,
    /// コピーした動画の数（EXP-05）。
    #[serde(default)]
    pub copied: u64,
    /// 書き出さなかった数。
    pub skipped: u64,
    /// 失敗した数。
    pub failed: u64,
    /// 取り消されて途中で終わったか。
    pub cancelled: bool,
}

// ---------------------------------------------------------------------------
// 比較明・比較暗合成（DEV-34。docs/composite_design.md）
// ---------------------------------------------------------------------------

/// 合成する前の確認（[`crate::Core::composite_check`]。docs/composite_design.md の 6.1 節）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CompositeCheck {
    /// 合成する写真（撮影日時の順。分からないものは最後、同じなら variant の ID の順）。
    pub sources: Vec<VariantId>,
    /// 除いた動画の数。
    pub skipped_videos: u64,
    /// 合成できない理由（写真の数が足りない・多すぎる・見つからない写真がある）。空なら合成できる。
    pub problems: Vec<String>,
    /// 見つからない（または読めなかった）写真。
    pub missing: Vec<VariantId>,
    /// 元の写真の寸法・向きが 1 枚目と違う写真（現像した後の寸法が違えば合成できない）。
    pub size_differs: Vec<VariantId>,
    /// 切り抜き・角度・歪曲補正（画素の位置を動かす設定）が 1 枚目と違う写真（画素の位置がずれる。警告だけ）。
    pub geometry_differs: Vec<VariantId>,
    /// それ以外の現像の設定が 1 枚目と違う写真（警告だけ）。
    pub settings_differ: Vec<VariantId>,
    /// 合成の元の写真が入っているスタックの数（元の写真とまとめると、これらは 1 つになる）。
    pub stacks: u64,
    /// 累積のバッファのメモリの見込み（バイト。元の写真の寸法から。切り抜くと小さくなる）。
    pub estimated_buffer_bytes: u64,
    /// 保存先のフォルダ（指定がなければ 1 枚目の写真と同じフォルダ）。
    #[serde(serialize_with = "lossy_path")]
    pub dest_dir: Option<PathBuf>,
    /// 保存するファイルの名前（同じ名前があれば、保存するときに連番を付ける）。
    pub file_name: Option<String>,
}

/// 合成の元の写真の警告（現像の警告など）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositeWarning {
    /// 写真。
    pub variant_id: VariantId,
    /// 説明。
    pub message: String,
}

/// 合成の失敗（どの写真で失敗したか）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositeFailure {
    /// 失敗した写真（写真によらない失敗（書き込みなど）なら `None`）。
    pub variant_id: Option<VariantId>,
    /// エラーの情報。
    pub error: ErrorInfo,
}

/// 合成の結果（[`JobResult::Composite`]）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CompositeReport {
    /// 比べ方。
    pub mode: genzo_model::CompositeMode,
    /// 合成した（する）写真（撮影日時の順）。
    pub sources: Vec<VariantId>,
    /// 除いた動画の数。
    pub skipped_videos: u64,
    /// 重ねた枚数（失敗・取り消しのときは途中までの数）。
    pub composited: u64,
    /// 合成した画像の寸法（幅・高さ）。
    pub size: Option<(u32, u32)>,
    /// 書いたファイル（失敗・取り消しでは `None`。何も書かない）。
    #[serde(serialize_with = "lossy_path")]
    pub path: Option<PathBuf>,
    /// カタログに登録した写真（登録に失敗したら `None`）。
    pub registered: Option<VariantId>,
    /// 登録できなかった理由（ファイルは残す。取り込み直せる）。
    pub registration_error: Option<String>,
    /// 元の写真とスタックにまとめたか。
    pub stacked: bool,
    /// スタックにまとめられなかった理由。
    pub stack_error: Option<String>,
    /// CPU 版で現像した写真の数（GPU が使えない・GPU 版で失敗した）。
    pub cpu_renders: u64,
    /// 現像の警告（写真ごと）。
    pub warnings: Vec<CompositeWarning>,
    /// 失敗（1 枚でも現像に失敗したら全体を失敗にする。決定事項ログ No.178）。
    pub failure: Option<CompositeFailure>,
    /// 取り消されて途中で終わったか（何も書かない）。
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
    /// 動画の再生用のファイル（VID-04）のキャッシュの上限（バイト。既定は
    /// [`crate::DEFAULT_VIDEO_PROXY_CAPACITY_BYTES`]）。
    #[serde(default = "default_video_proxy_bytes")]
    pub video_proxy_cache_bytes: u64,
}

fn default_true() -> bool {
    true
}

fn default_video_proxy_bytes() -> u64 {
    crate::playback::DEFAULT_VIDEO_PROXY_CAPACITY_BYTES
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
    /// 動画の再生用のファイルのキャッシュの上限（バイト。すぐに有効。超えていれば古いものから消す）。
    pub video_proxy_cache_bytes: Option<u64>,
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

// ---------------------------------------------------------------------------
// 撮影日時の一括修正（LIB-16）・タイムライン（ORG-01）
// ---------------------------------------------------------------------------

/// 機材（カメラの表示名・本体のシリアル番号・写真か動画か。LIB-16。genzo-catalog の `DeviceKey`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CaptureDeviceKey {
    /// カメラの表示名（分からなければ `null`）。
    pub camera: Option<String>,
    /// 本体のシリアル番号（記録しないカメラ・版 11 より前に取り込んだものは `null`）。
    pub serial: Option<String>,
    /// 写真か動画か。
    pub kind: AssetKind,
}

impl CaptureDeviceKey {
    pub(crate) fn to_catalog(&self) -> genzo_catalog::DeviceKey {
        genzo_catalog::DeviceKey {
            camera: self.camera.clone(),
            serial: self.serial.clone(),
            kind: self.kind,
        }
    }
}

impl From<genzo_catalog::DeviceKey> for CaptureDeviceKey {
    fn from(k: genzo_catalog::DeviceKey) -> Self {
        Self {
            camera: k.camera,
            serial: k.serial,
            kind: k.kind,
        }
    }
}

/// 機材と数（[`crate::Core::capture_devices`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureDevice {
    /// 機材。
    #[serde(flatten)]
    pub key: CaptureDeviceKey,
    /// 写真・動画の数（asset の数。仮想コピーは数えない）。
    pub assets: u64,
    /// 撮影日時の分かる数。
    pub with_time: u64,
    /// 修正（時計のずれの補正・タイムゾーンの指定）のある数。
    pub adjusted: u64,
    /// 最も早い撮影日時（既定のタイムゾーンの時刻。RFC 3339）。
    pub first: Option<DateTime<FixedOffset>>,
    /// 最も遅い撮影日時（既定のタイムゾーンの時刻）。
    pub last: Option<DateTime<FixedOffset>>,
}

/// 撮影日時の修正の対象（[`crate::Core::edit_capture_times`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureTarget {
    /// 写真（同じ写真の仮想コピーは撮影日時を共有するので 1 枚）。
    Variants {
        /// variant。
        variant_ids: Vec<VariantId>,
    },
    /// 機材のすべての写真・動画。
    Devices {
        /// 機材（[`crate::Core::capture_devices`] の値）。
        devices: Vec<CaptureDeviceKey>,
    },
}

/// 撮影日時の修正（LIB-16。[`crate::Core::edit_capture_times`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureTimeEdit {
    /// 時計のずれを足す（秒。正で後の時刻にする）。
    Shift {
        /// 秒（±[`genzo_catalog::MAX_CAPTURE_SHIFT_SECONDS`] まで）。
        seconds: i64,
    },
    /// 基準の写真の正しい時刻から差を求めて、対象のすべてをその差だけずらす。
    Sync {
        /// 基準の写真（撮影日時の分かるもの）。
        reference: VariantId,
        /// 基準の写真の正しい撮影日時（`"2024-05-01T12:34:56"` の形。オフセットを付ければそれで読む）。
        correct: String,
        /// `correct` を読むオフセット（分。省略すると基準の写真の表示のオフセット）。
        #[serde(default)]
        offset_minutes: Option<i32>,
    },
    /// タイムゾーンを指定する。
    SetZone {
        /// オフセット（分。-18 時間〜+18 時間）。
        offset_minutes: i32,
        /// 読み方（`keep_local`: 記録の時刻をそのタイムゾーンの時刻として読む・`keep_instant`: 撮影した瞬間は
        /// そのままで、そのタイムゾーンで表示する）。
        mode: genzo_model::ZoneMode,
    },
    /// タイムゾーンの指定を取り消す（時計のずれの補正は保つ）。
    ClearZone,
    /// 修正をすべて取り消し、ファイルの値に戻す。
    Reset,
}

/// 対象の撮影日時の出どころごとの数（修正の前。撮影日時の分かるもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CaptureSourceCounts {
    /// ファイルにオフセットが記録されていた。
    pub exif: u64,
    /// UTC の印（`Z`）だけが記録されていた（動画など）。
    pub utc_only: u64,
    /// 記録がなく既定のタイムゾーンで推定した。
    pub user_default: u64,
    /// 利用者が指定した。
    pub user_set: u64,
}

impl From<genzo_catalog::TzSourceCounts> for CaptureSourceCounts {
    fn from(c: genzo_catalog::TzSourceCounts) -> Self {
        Self {
            exif: c.exif,
            utc_only: c.utc_only,
            user_default: c.user_default,
            user_set: c.user_set,
        }
    }
}

/// 1 枚の修正の例。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTimeExample {
    /// asset。
    pub asset_id: AssetId,
    /// 主なファイルの名前。
    pub file_name: String,
    /// 修正の前。
    pub before: CaptureTime,
    /// 修正の後。
    pub after: CaptureTime,
}

/// 撮影日時の修正の見込み・結果（[`crate::Core::plan_capture_time_edit`]・[`crate::Core::edit_capture_times`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CaptureTimePlan {
    /// 対象の写真・動画の数（asset の数）。
    pub assets: u64,
    /// 変わる（変えた）数。
    pub changed: u64,
    /// 変わらない数（撮影日時の分かるもの）。
    pub unchanged: u64,
    /// 撮影日時の分からない数（変えない）。
    pub without_time: u64,
    /// 撮影日時の出どころごとの数。
    pub sources: CaptureSourceCounts,
    /// ずらす秒数（ずらす・基準の写真に合わせるとき。基準の写真から求めた差）。
    pub shift_seconds: Option<i64>,
    /// 変わるものの例（対象の順に最大 [`crate::CAPTURE_TIME_EXAMPLES`] 件）。
    pub examples: Vec<CaptureTimeExample>,
    /// 記録した修正（結果のとき。変わったものがなければ `null`。元に戻すのに使う）。
    pub edit_id: Option<i64>,
}

/// 撮影日時の修正の履歴の 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTimeEditInfo {
    /// ID。
    pub id: i64,
    /// 種類（`shift`・`sync`・`set_zone`・`clear_zone`・`reset`）。
    pub kind: String,
    /// ずらした秒数（`shift`・`sync`）。
    pub shift_seconds: Option<i64>,
    /// 指定したオフセット（`set_zone`。`"+09:00"` の形）。
    pub zone: Option<String>,
    /// 指定の読み方（`set_zone`）。
    pub zone_mode: Option<genzo_model::ZoneMode>,
    /// 変えた写真・動画の数。
    pub asset_count: u64,
    /// 修正した日時（UTC）。
    pub created_at: DateTime<Utc>,
}

impl From<genzo_catalog::CaptureEditRecord> for CaptureTimeEditInfo {
    fn from(r: genzo_catalog::CaptureEditRecord) -> Self {
        Self {
            id: r.id,
            kind: r.kind.as_str().to_owned(),
            shift_seconds: r.shift_s,
            zone: r.zone,
            zone_mode: r.zone_mode,
            asset_count: r.asset_count,
            created_at: r.created_at,
        }
    }
}

/// 撮影日時の修正の取り消しの結果（[`crate::Core::undo_capture_time_edit`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTimeUndo {
    /// 取り消した修正。
    pub edit: CaptureTimeEditInfo,
    /// 戻した写真・動画の数。
    pub restored: u64,
}

/// タイムラインのまとまり（ORG-01。[`crate::Core::timeline`]）。撮影日の中の、撮影の間隔で区切ったイベント。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineGroup {
    /// 検索結果の中の先頭の位置。
    pub start: u64,
    /// 写真・動画の数（仮想コピー・折りたたんだスタックの代表を含む、表示する数）。
    pub count: u64,
    /// 撮影日（写真の表示のオフセットの現地の日付。`"2024-05-01"`。撮影日時の分からないまとまりは `null`）。
    pub day: Option<String>,
    /// その日の最初のまとまりか（日付の見出しを出す）。
    pub new_day: bool,
    /// 最初の写真の現地時刻（`"2024-05-01T09:12:05"`。並びの順で最初。分からなければ `null`）。
    pub first: Option<String>,
    /// 最後の写真の現地時刻。
    pub last: Option<String>,
    /// 写真の数。
    pub photos: u64,
    /// 動画の数。
    pub videos: u64,
}

/// タイムライン（ORG-01。[`crate::Core::timeline`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeline {
    /// 検索結果の世代。
    pub generation: u64,
    /// イベントの区切りに使った間隔（秒。0 ならイベントで分けない）。
    pub gap_seconds: u32,
    /// まとまり（検索結果の順。すべてのまとまりの数を足すと検索結果の件数）。
    pub groups: Vec<TimelineGroup>,
    /// まとまりが多すぎるため、イベントで分けずに日付だけでまとめたか（[`crate::MAX_TIMELINE_GROUPS`]）。
    pub events_merged: bool,
    /// 撮影日の数。
    pub days: u64,
    /// 撮影日時の分からない写真・動画の数。
    pub without_time: u64,
}

// ---------------------------------------------------------------------------
// 位置情報の後付け（LIB-21。ジオタグ付け）
// ---------------------------------------------------------------------------

/// 使う位置の出どころ（[`VariantDetails::gps_source`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpsSource {
    /// カメラ（ファイル）が記録した位置。
    Camera,
    /// 後から付けた位置（カタログの中だけに持つ。元のファイルには書かない）。
    Assigned,
}

impl GpsSource {
    /// 使う位置の出どころ（付けた位置があれば `Assigned`、なければカメラの位置があれば `Camera`）。
    pub fn of(assigned: Option<GpsCoord>, camera: Option<GpsCoord>) -> Option<Self> {
        match (assigned, camera) {
            (Some(_), _) => Some(Self::Assigned),
            (None, Some(_)) => Some(Self::Camera),
            (None, None) => None,
        }
    }
}

/// 位置の変更の見込み・結果（[`crate::Core::plan_location_edit`]・[`crate::Core::edit_locations`]）。数は
/// どれも写真の数（asset の数。同じ写真の仮想コピーは位置を共有するので 1 枚）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LocationEditPlan {
    /// 対象の写真の数（動画・見つからないものを除く）。
    pub photos: u64,
    /// 変わる（変えた）数。
    pub changed: u64,
    /// 変わらない数（同じ位置がすでに付いている。消すときは付けた位置がない）。
    pub unchanged: u64,
    /// 対象のうち、カメラ（ファイル）の位置がある数（付けた位置があれば、そちらを使う）。
    pub with_camera_gps: u64,
    /// 対象のうち、変える前に付けた位置がある数（付けるときは置き換える）。
    pub with_assigned: u64,
    /// 除いた動画の数（asset の数。動画には位置を付けない。書き出しは元のファイルのままコピーするため）。
    pub skipped_videos: u64,
    /// 見つからなかった写真の数（削除されたものなど。variant の数）。
    pub missing: u64,
    /// 記録した変更（結果のとき。変わったものがなければ `null`。見込みでは常に `null`）。
    pub edit_id: Option<i64>,
}

/// 位置の変更の履歴の 1 件（[`crate::Core::location_edits`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocationEditInfo {
    /// ID。
    pub id: i64,
    /// 種類（`set`: 位置を付けた・`clear`: 付けた位置を消した）。
    pub kind: String,
    /// 付けた位置（`set`。`clear` は `null`）。
    pub location: Option<GpsCoord>,
    /// 変えた写真の数。
    pub asset_count: u64,
    /// 変更した日時（UTC）。
    pub created_at: DateTime<Utc>,
}

impl From<genzo_catalog::LocationEditRecord> for LocationEditInfo {
    fn from(r: genzo_catalog::LocationEditRecord) -> Self {
        Self {
            id: r.id,
            kind: r.kind.as_str().to_owned(),
            location: r.location,
            asset_count: r.asset_count,
            created_at: r.created_at,
        }
    }
}

/// 位置の変更の取り消しの結果（[`crate::Core::undo_location_edit`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocationUndo {
    /// 取り消した変更。
    pub edit: LocationEditInfo,
    /// 戻した写真の数（今の位置と同じだったものを除く）。
    pub restored: u64,
    /// 変更の後に削除された写真の数（戻すものがない）。
    pub removed: u64,
}

// ---------------------------------------------------------------------------
// ボリューム（FILE-03）・アプリの中でのファイル操作（FILE-02）・見つからない写真と再リンク（FILE-04）
// ---------------------------------------------------------------------------

/// ボリューム（ドライブ。FILE-03。[`crate::Core::volumes`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    /// ボリューム。
    pub volume_id: VolumeId,
    /// ボリュームの ID（`uuid:`・`winvol:`・`net:` は OS のボリューム ID、`path:` はパスの先頭から作ったもの）。
    pub uuid: String,
    /// ラベル（ボリュームの名前）。
    pub label: Option<String>,
    /// マウント先（つながっていれば今のもの、つながっていなければ最後に確かめたもの）。
    #[serde(serialize_with = "lossy_path")]
    pub mount_path: Option<PathBuf>,
    /// つながっているか。
    pub online: bool,
    /// OS のボリューム ID で識別しているか（`false` はパスの先頭から作った ID。マウント先が変わると追えない）。
    pub identified: bool,
    /// ネットワークのボリュームか（分からなければ `false`）。
    pub network: bool,
    /// ファイルシステムの種類。
    pub fs_type: Option<String>,
    /// つながっていることを最後に確かめた日時。
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// 移動の方法（FILE-02）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveMethod {
    /// 同じボリュームの中の名前の変更。
    Rename,
    /// 別のボリュームへのコピー（照合してから公開し、元をゴミ箱へ移す）。
    Copy,
}

/// 移動・リネームの予定の 1 ファイル。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedFileMove {
    /// ファイル。
    pub file_id: FileId,
    /// 役割（RAW と同時記録の JPEG は一緒に動く）。
    pub role: FileRole,
    /// 今の場所。
    #[serde(serialize_with = "lossy_path")]
    pub from: Option<PathBuf>,
    /// 移動先。
    #[serde(serialize_with = "lossy_path")]
    pub to: Option<PathBuf>,
    /// 方法。
    pub method: MoveMethod,
    /// 大きさ（バイト）。
    pub size: u64,
}

/// 移動・リネームの予定の 1 枚の写真（asset。仮想コピーは同じファイルなので 1 つにまとまる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedPhotoMove {
    /// 写真（マスターの variant）。
    pub variant_id: VariantId,
    /// asset。
    pub asset_id: AssetId,
    /// ファイル（主なファイルが先）。
    pub files: Vec<PlannedFileMove>,
    /// 移せない・名前を変えられない理由（あれば、この写真は飛ばす）。
    pub problem: Option<String>,
}

/// 写真の移動の予定（[`crate::Core::plan_move_files`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMovePlan {
    /// 移動先のフォルダ。
    pub dest_folder: FolderId,
    /// 移動先のフォルダのパス。
    #[serde(serialize_with = "lossy_path")]
    pub dest_path: Option<PathBuf>,
    /// 写真。
    pub photos: Vec<PlannedPhotoMove>,
    /// 移せる写真の数。
    pub movable: u64,
    /// 別のボリュームへコピーするバイト数（移せる写真の分）。
    pub copy_bytes: u64,
}

/// 動かした 1 ファイル。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedFile {
    /// ファイル。
    pub file_id: FileId,
    /// 元の場所。
    #[serde(serialize_with = "lossy_path")]
    pub from: PathBuf,
    /// 新しい場所。
    #[serde(serialize_with = "lossy_path")]
    pub to: PathBuf,
}

/// 写真の移動・リネームの結果（[`crate::Core::move_files`]・[`crate::Core::rename_files`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileMoveReport {
    /// 動かしたファイル。
    pub moved: Vec<MovedFile>,
    /// 動かした写真の数。
    pub photos: u64,
    /// 飛ばした写真（理由。予定の `problem` と、実行の直前に分かったもの）。
    pub skipped: Vec<FileIssue>,
    /// 失敗した写真（元の状態に戻した）。
    pub failed: Vec<FileIssue>,
    /// 別のボリュームへ移した写真の、ゴミ箱へ移せずに元の場所に残ったファイル（カタログは移動先を指す）。
    pub left_sources: Vec<FileIssue>,
    /// コピーしたバイト数。
    pub bytes_copied: u64,
    /// 取り消したか（途中までの結果）。
    pub cancelled: bool,
}

/// 写真の名前の付け方（FILE-02 のリネーム。[`crate::Core::plan_rename_files`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenameSpec {
    /// リネームの規則（IMP-03 のテンプレート。連番は与えた写真の順。重なる名前には `-1` などを付ける）。
    Template {
        /// 規則。
        rule: crate::naming::RenameRule,
    },
    /// 1 枚の写真の名前（拡張子を除く。拡張子は元のまま。使えない文字があればエラー）。
    Name {
        /// 名前。
        name: String,
    },
}

/// 写真の名前の変更の予定（[`crate::Core::plan_rename_files`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRenamePlan {
    /// 写真（与えた順）。
    pub photos: Vec<PlannedPhotoMove>,
    /// 名前を変える写真の数（今と同じ名前・問題のある写真を除く）。
    pub renamable: u64,
}

/// フォルダの場所の指定し直しの予定（FILE-04。[`crate::Core::plan_relocate_folder`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelocatePlan {
    /// フォルダ。
    pub folder_id: FolderId,
    /// 今の（見つからない）場所。
    #[serde(serialize_with = "lossy_path")]
    pub from: Option<PathBuf>,
    /// 新しい場所。
    #[serde(serialize_with = "lossy_path")]
    pub to: PathBuf,
    /// フォルダ（配下を含む）に登録したファイルの数。
    pub files: u64,
    /// そのうち、新しい場所に同じ名前・大きさで見つかった数。
    pub found: u64,
    /// 配下のフォルダの数。
    pub subfolders: u64,
    /// 指定し直せない理由（あれば実行できない）。
    pub problem: Option<String>,
}

/// 見つからない写真の確認の結果（FILE-04。[`crate::Core::check_missing`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MissingCheckReport {
    /// 確かめたファイルの数。
    pub checked: u64,
    /// 新しく見つからない印を付けたファイルの数。
    pub newly_missing: u64,
    /// 見つからない印を外した（元の場所に戻っていた）ファイルの数。
    pub restored: u64,
    /// 確かめた後に見つからないファイルの数（範囲の中）。
    pub missing: u64,
    /// 見つからないフォルダの数。
    pub missing_folders: u64,
    /// つながっていないボリュームのため確かめなかったファイルの数。
    pub skipped_offline: u64,
    /// ネットワークのボリュームのため確かめなかったファイルの数（自動の確認）。
    pub skipped_network: u64,
    /// 取り消したか。
    pub cancelled: bool,
}

/// 再リンクの候補の一致の仕方（FILE-04。確からしい順）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelinkMatch {
    /// 内容（ファイル全体の BLAKE3）が登録のものと同じ。
    Content,
    /// 大きさと先頭・末尾（クイックハッシュ）が同じ（全体のハッシュの記録がないため、全体は比べていない）。
    Quick,
    /// 名前と撮影日時が同じで、内容は変わっている（ほかのソフトで書き換えたなど。結び付けると読み直す）。
    NameAndTime,
}

/// 再リンクの候補の 1 ファイル。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelinkCandidate {
    /// 場所。
    #[serde(serialize_with = "lossy_path")]
    pub path: PathBuf,
    /// 一致の仕方。
    pub matched: RelinkMatch,
    /// 大きさ（バイト）。
    pub size: u64,
    /// 名前が同じか（大文字・小文字と Unicode の正規化を区別しない）。
    pub same_name: bool,
}

/// 再リンクする見つからないファイルと、その候補。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelinkTarget {
    /// ファイル。
    pub file_id: FileId,
    /// 写真（マスターの variant）。
    pub variant_id: VariantId,
    /// 役割。
    pub role: FileRole,
    /// 登録の名前。
    pub name: String,
    /// 登録の場所。
    #[serde(serialize_with = "lossy_path")]
    pub last_path: Option<PathBuf>,
    /// 大きさ（バイト）。
    pub size: u64,
    /// 候補（確からしい順）。
    pub candidates: Vec<RelinkCandidate>,
}

/// 再リンクの候補の探索の結果（FILE-04。[`crate::Core::find_relink_candidates`] のジョブ）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RelinkSearch {
    /// 探したフォルダ。
    #[serde(serialize_with = "lossy_path")]
    pub search_dir: PathBuf,
    /// 見つからないファイルと候補（候補がないものも含む）。
    pub targets: Vec<RelinkTarget>,
    /// 走査した写真・動画のファイルの数。
    pub scanned_files: u64,
    /// 読めなかったフォルダ・ファイル。
    pub unreadable: Vec<FileIssue>,
    /// 取り消したか。
    pub cancelled: bool,
}

/// 再リンクの 1 件（[`crate::Core::relink_files`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelinkRequest {
    /// 見つからないファイル。
    pub file_id: FileId,
    /// 結び付ける場所。
    pub path: PathBuf,
    /// 内容が登録のものと違っても結び付けるか（[`RelinkMatch::NameAndTime`] の候補を利用者が選んだとき。
    /// 結び付けた後に読み直す）。
    #[serde(default)]
    pub accept_changed: bool,
}

/// 再リンクの結果。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RelinkReport {
    /// 結び付けたファイル。
    pub linked: Vec<MovedFile>,
    /// 内容が変わっていたので読み直したファイル。
    #[serde(serialize_with = "lossy_path")]
    pub reread: Vec<PathBuf>,
    /// 結び付けられなかったもの。
    pub failed: Vec<FileIssue>,
}

#[cfg(test)]
mod setting_groups_tests {
    use super::*;

    #[test]
    fn basic_tone_includes_highlight_recovery_and_detail_includes_noise_reduction() {
        let mut source = DevelopSettings {
            exposure_ev: 0.5,
            highlight_recovery: genzo_model::HighlightRecovery {
                method: genzo_model::HighlightMethod::Reconstruct,
                amount: 70.0,
            },
            ..Default::default()
        };
        source.detail.noise_reduction.luminance = 30.0;
        let target = DevelopSettings::default();
        // 基本のトーンだけ: ハイライト復元は写り、ノイズ軽減は写らない。
        let tone_only = SettingGroups {
            white_balance: false,
            basic_tone: true,
            presence: false,
            tone_curve: false,
            color: false,
            geometry: false,
            lens: false,
            detail: false,
            effects: false,
            profile: false,
            masks: false,
        };
        let out = tone_only.merge(&source, &target);
        assert_eq!(out.exposure_ev, 0.5);
        assert_eq!(out.highlight_recovery, source.highlight_recovery);
        assert_eq!(out.detail, target.detail);
        // ディテールだけ: ノイズ軽減が写り、ハイライト復元は写らない。
        let detail_only = SettingGroups {
            basic_tone: false,
            detail: true,
            ..tone_only
        };
        let out = detail_only.merge(&source, &target);
        assert_eq!(out.detail.noise_reduction.luminance, 30.0);
        assert_eq!(out.highlight_recovery, target.highlight_recovery);
        assert_eq!(out.exposure_ev, 0.0);
    }

    #[test]
    fn presence_and_effects_are_their_own_groups() {
        let mut source = DevelopSettings::default();
        source.presence.clarity = 40.0;
        source.effects.grain.amount = 20.0;
        source.detail.sharpening.amount = 50.0;
        let target = DevelopSettings::default();
        let none = SettingGroups {
            white_balance: false,
            basic_tone: false,
            presence: false,
            tone_curve: false,
            color: false,
            geometry: false,
            lens: false,
            detail: false,
            effects: false,
            profile: false,
            masks: false,
        };
        let presence = SettingGroups {
            presence: true,
            ..none
        }
        .merge(&source, &target);
        assert_eq!(presence.presence.clarity, 40.0);
        assert!(presence.effects.is_neutral() && presence.detail.sharpening.amount == 0.0);
        let effects = SettingGroups {
            effects: true,
            ..none
        }
        .merge(&source, &target);
        assert_eq!(effects.effects.grain.amount, 20.0);
        assert!(effects.presence.is_neutral());
        // 古い画面（項目のない JSON）からの指定は、足した項目も写す（既定はすべて）。
        let old: SettingGroups = serde_json::from_str(r#"{"white_balance": false}"#).unwrap();
        assert!(old.presence && old.effects && old.profile && !old.white_balance);
        // カメラプロファイル・ルック（DEV-17）。
        let mut source = source.clone();
        source.profile.camera = Some("Sony 標準".into());
        source.profile.look = Some("film".into());
        source.profile.look_amount = 30.0;
        let profile = SettingGroups {
            profile: true,
            ..none
        }
        .merge(&source, &target);
        assert_eq!(profile.profile, source.profile);
        assert!(profile.presence.is_neutral());
        let others = SettingGroups {
            profile: false,
            ..SettingGroups::ALL
        }
        .merge(&source, &target);
        assert_eq!(others.profile, target.profile);
    }

    /// すべての項目を写しても、一部の項目を写しても、処理バージョンと外部データは貼り付け先のまま（DATA-09。
    /// 検証の指摘: すべての項目では貼り付け元の版に黙って置き換わっていた）。
    #[test]
    fn merge_keeps_the_target_process_version_and_render_deps() {
        let mut source = DevelopSettings {
            exposure_ev: 0.7,
            ..Default::default()
        };
        source.lens.profile_enabled = true;
        source.render_deps.raw_decoder = "libraw-source".to_owned();
        assert_eq!(source.process_version, 2);
        let mut target = DevelopSettings {
            process_version: 1,
            ..Default::default()
        };
        target.render_deps.raw_decoder = "libraw-target".to_owned();
        for groups in [
            SettingGroups::ALL,
            SettingGroups {
                geometry: false,
                ..SettingGroups::ALL
            },
        ] {
            let out = groups.merge(&source, &target);
            assert_eq!(out.process_version, 1, "{groups:?}");
            assert_eq!(out.render_deps, target.render_deps, "{groups:?}");
            assert_eq!(out.exposure_ev, 0.7);
            assert!(out.lens.profile_enabled);
        }
        // 逆向き（版 1 の設定を版 2 の写真へ）も、版 2 のまま。
        let out = SettingGroups::ALL.merge(&target, &source);
        assert_eq!(out.process_version, 2);
        assert_eq!(out.exposure_ev, 0.0);
    }
}
