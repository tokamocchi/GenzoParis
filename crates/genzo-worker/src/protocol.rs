//! 本体とワーカーの間のやり取りの形式（docs/04_architecture.md の 1.2 節「ワーカーとのやり取り」）。
//!
//! - 制御（ジョブの依頼と結果）は、標準入出力で **1 行 1 メッセージの JSON**（UTF-8、行末は `\n`）を
//!   やり取りする。本体 → ワーカーが [`Request`]、ワーカー → 本体が [`Response`]。1 つのワーカーには
//!   同時に 1 件だけ依頼する（応答を受け取るまで次の依頼を送らない）。
//! - 画像のデータ（CFA・リニアの RGB・JPEG）は JSON に入れず、本体が確保した共有メモリ
//!   （[`ShmRef`]。[`crate::shm`]）にワーカーが書き込む。JSON には寸法などの付随情報だけを入れる。
//! - 1 行の長さは [`MAX_LINE_BYTES`] まで。超えた行は不正な応答（本体）・不正な依頼（ワーカー）とする。
//!
//! JSON の例:
//!
//! ```text
//! → {"id":7,"job":{"type":"decode_raw","path":"/photos/a.arw","shm":{"path":"/tmp/genzo-shm-1/3.shm","capacity":75497472}}}
//! ← {"id":7,"result":{"Ok":{"type":"raw","width":7040,"height":4688,...}}}
//! ← {"id":8,"result":{"Err":{"kind":{"code":"shm_too_small","required_bytes":66007104},"message":"..."}}}
//! ```

use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};

use genzo_media::{VideoColorHandling, VideoProbe};
use genzo_model::PhotoMetadata;
use genzo_raw::CfaPattern;
use serde::{Deserialize, Serialize};

/// プロトコルの版。本体は起動直後の [`Job::Ping`] の応答で一致を確かめる。
///
/// メッセージの形を互換性のない形で変えたら上げる。
pub const PROTOCOL_VERSION: u32 = 1;

/// 1 行（1 メッセージ）の長さの上限（バイト。改行を除く）。
///
/// **仮置き**: 1 MiB。メッセージは制御と付随情報（撮影情報・動画の情報）だけで、通常は数 KB 以下。
/// 改行のない出力を読み続けてメモリを使い切らないための上限。
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// 1 行を読んだ結果（[`read_line_limited`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineRead {
    /// 1 行（改行と、その直前の `\r` を除く）。最後の行に改行がなければ、そのまま返す。
    Line(Vec<u8>),
    /// 入力の終わり。
    Eof,
    /// 改行の前に `max` バイトを超えた（行の残りは読んでいない）。
    TooLong,
}

/// 長さの上限付きで 1 行を読む。
pub fn read_line_limited<R: BufRead>(r: &mut R, max: usize) -> io::Result<LineRead> {
    let mut buf = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = r.by_ref().take(limit).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(LineRead::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(LineRead::Line(buf));
    }
    if buf.len() > max {
        return Ok(LineRead::TooLong);
    }
    Ok(LineRead::Line(buf))
}

/// 本体 → ワーカーの依頼。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 依頼の番号（応答で同じ値を返す）。
    pub id: u64,
    /// ジョブ。
    pub job: Job,
}

/// ワーカー → 本体の応答。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// 依頼の番号（[`Request::id`]）。依頼を解釈できなかった場合は 0。
    pub id: u64,
    /// 結果。
    pub result: Result<JobOutput, WorkerError>,
}

/// ジョブ。
///
/// `Test*` は結合テスト用の口で、ワーカーを環境変数 `GENZO_WORKER_TEST_HOOKS=1` で起動したときだけ
/// 実行する（[`crate::ENV_TEST_HOOKS`]）。それ以外では [`WorkerErrorKind::InvalidRequest`] を返す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Job {
    /// 生存の確認と、プロトコルの版の確認（起動直後の握手にも使う）。
    Ping,
    /// 終了する（応答を返してから終了する）。
    Shutdown,
    /// 写真のメタデータを読む（RAW は LibRaw、JPEG / TIFF / PNG は Exif）。
    ProbePhoto {
        /// 写真のファイル。
        #[serde(with = "wire_path")]
        path: PathBuf,
    },
    /// 動画のメタデータを読む（ffprobe）。
    ProbeVideo {
        /// 動画のファイル。
        #[serde(with = "wire_path")]
        path: PathBuf,
    },
    /// RAW を展開し、CFA（u16）を共有メモリに書く（段階 A0。04 の 2.2 節）。
    DecodeRaw {
        /// RAW のファイル。
        #[serde(with = "wire_path")]
        path: PathBuf,
        /// 書き込む共有メモリ。
        shm: ShmRef,
    },
    /// JPEG / TIFF / PNG をデコードし、リニア BT.2020（B2、f32 の RGB）にして共有メモリに書く。
    DecodeImage {
        /// 画像のファイル。
        #[serde(with = "wire_path")]
        path: PathBuf,
        /// 書き込む共有メモリ。
        shm: ShmRef,
    },
    /// 写真のサムネイル（B5 の JPEG）を作り、共有メモリに書く（PRV-01）。
    Thumbnail {
        /// 写真のファイル。
        #[serde(with = "wire_path")]
        path: PathBuf,
        /// 長辺の画素数（これより小さい画像は拡大しない）。
        max_edge: u32,
        /// JPEG の品質（1〜100）。`None` なら長辺から L0 / L1 の品質を選ぶ
        /// （[`crate::worker::default_quality_for`]）。
        #[serde(default)]
        quality: Option<u8>,
        /// 書き込む共有メモリ。
        shm: ShmRef,
    },
    /// 動画のサムネイル（B5 の JPEG）を作り、共有メモリに書く（VID-02）。
    VideoThumbnail {
        /// 動画のファイル。
        #[serde(with = "wire_path")]
        path: PathBuf,
        /// 長辺の画素数。
        max_edge: u32,
        /// JPEG の品質（1〜100）。`None` なら長辺から選ぶ。
        #[serde(default)]
        quality: Option<u8>,
        /// 書き込む共有メモリ。
        shm: ShmRef,
    },
    /// テスト用: 共有メモリ（あれば）の途中まで書いてから、即座に異常終了する（abort）。
    TestCrash {
        /// 書き込む共有メモリ（任意）。
        #[serde(default)]
        shm: Option<ShmRef>,
        /// 標準出力・標準エラー出力を引き継いだ子プロセスを残してから異常終了するか
        /// （ffmpeg などの孫プロセスが制御のパイプを持ったまま、ワーカーが落ちた場合の確認。
        /// Windows では孫プロセスがワーカーの標準入出力のハンドルを引き継ぐ）。子プロセスは
        /// `shm` のファイルがある間（最長 [`crate::worker::TEST_CHILD_HOLD_MAX`]）待ってから終わる。
        /// `shm` がなければ子プロセスを作らない。
        #[serde(default)]
        leave_child: bool,
    },
    /// テスト用: 応答しない（ハング）。
    TestHang {
        /// 書き込む共有メモリ（任意）。
        #[serde(default)]
        shm: Option<ShmRef>,
    },
    /// テスト用: 不正な行を出力する（その後も動き続ける）。
    TestGarbage {
        /// 不正な行の種類。
        mode: GarbageMode,
    },
    /// テスト用: 共有メモリのヘッダや付随情報を偽った、RAW の展開の結果を返す。
    TestOversize {
        /// 偽り方。
        mode: OversizeMode,
        /// 書き込む共有メモリ。
        shm: ShmRef,
    },
    /// テスト用: `bytes` バイトのメモリを確保して書き込み、解放する（メモリの上限の確認）。
    TestAllocate {
        /// 確保する量（バイト）。
        bytes: u64,
    },
}

impl Job {
    /// ジョブの種類。
    pub fn kind(&self) -> JobKind {
        match self {
            Job::Ping => JobKind::Ping,
            Job::Shutdown => JobKind::Shutdown,
            Job::ProbePhoto { .. } => JobKind::ProbePhoto,
            Job::ProbeVideo { .. } => JobKind::ProbeVideo,
            Job::DecodeRaw { .. } => JobKind::DecodeRaw,
            Job::DecodeImage { .. } => JobKind::DecodeImage,
            Job::Thumbnail { .. } => JobKind::Thumbnail,
            Job::VideoThumbnail { .. } => JobKind::VideoThumbnail,
            Job::TestCrash { .. }
            | Job::TestHang { .. }
            | Job::TestGarbage { .. }
            | Job::TestOversize { .. }
            | Job::TestAllocate { .. } => JobKind::TestHook,
        }
    }

    /// 対象のファイル（あれば）。
    pub fn path(&self) -> Option<&Path> {
        match self {
            Job::ProbePhoto { path }
            | Job::ProbeVideo { path }
            | Job::DecodeRaw { path, .. }
            | Job::DecodeImage { path, .. }
            | Job::Thumbnail { path, .. }
            | Job::VideoThumbnail { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// ジョブの種類（タイムアウトの既定値を決めるのに使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// [`Job::Ping`]。
    Ping,
    /// [`Job::Shutdown`]。
    Shutdown,
    /// [`Job::ProbePhoto`]。
    ProbePhoto,
    /// [`Job::ProbeVideo`]。
    ProbeVideo,
    /// [`Job::DecodeRaw`]。
    DecodeRaw,
    /// [`Job::DecodeImage`]。
    DecodeImage,
    /// [`Job::Thumbnail`]。
    Thumbnail,
    /// [`Job::VideoThumbnail`]。
    VideoThumbnail,
    /// テスト用のジョブ（`Test*`）。
    TestHook,
}

/// 共有メモリ（本体が確保したファイル）の場所と大きさ（[`crate::shm`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShmRef {
    /// ファイルのパス。
    #[serde(with = "wire_path")]
    pub path: PathBuf,
    /// 大きさ（バイト。ヘッダを含む）。ワーカーはファイルの大きさが一致することを確かめる。
    pub capacity: u64,
    /// データのチェックサムをヘッダに書くか（[`crate::shm::checksum`]）。
    #[serde(default)]
    pub checksum: bool,
}

/// 不正な行の種類（[`Job::TestGarbage`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GarbageMode {
    /// JSON ではない行。
    NotJson,
    /// 依頼と違う番号の応答。
    WrongId,
    /// [`MAX_LINE_BYTES`] を超える長い行。
    TooLong,
}

/// 偽り方（[`Job::TestOversize`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OversizeMode {
    /// ヘッダと付随情報の寸法が、画素数の上限（`genzo_raw::MAX_PIXELS`）を超える。
    TooManyPixels,
    /// ヘッダのデータの長さが寸法と合わない。
    LengthMismatch,
    /// ヘッダのデータの長さが共有メモリの大きさを超える。
    BeyondCapacity,
    /// ヘッダの寸法と付随情報（JSON）の寸法が違う。
    DimensionMismatch,
    /// ヘッダの先頭の印（マジック）が違う。
    BadMagic,
    /// ヘッダは正しいが、付随情報の白レベルが不正（`RawImage::validate` で検出される）。
    InvalidLevels,
    /// チェックサムが合わない。
    BadChecksum,
}

/// ジョブの結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobOutput {
    /// [`Job::Ping`] の結果。
    Pong(PongInfo),
    /// [`Job::Shutdown`] の結果（この後ワーカーは終了する）。
    ShuttingDown,
    /// [`Job::ProbePhoto`] の結果。
    Photo(PhotoProbe),
    /// [`Job::ProbeVideo`] の結果。
    Video(VideoProbe),
    /// [`Job::DecodeRaw`] の結果（CFA は共有メモリ）。
    Raw(RawFrameInfo),
    /// [`Job::DecodeImage`] の結果（画素は共有メモリ）。
    Image(LinearImageInfo),
    /// [`Job::Thumbnail`]・[`Job::VideoThumbnail`] の結果（JPEG は共有メモリ）。
    Thumbnail(ThumbnailInfo),
    /// [`Job::TestAllocate`] の結果。
    Allocated {
        /// 確保できた量（バイト）。
        bytes: u64,
    },
}

/// [`Job::Ping`] の結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PongInfo {
    /// ワーカーのプロトコルの版（[`PROTOCOL_VERSION`]）。
    pub protocol_version: u32,
    /// ワーカーの crate の版。
    pub worker_version: String,
    /// ワーカーのプロセス ID。
    pub pid: u32,
    /// RAW のデコーダの識別子（LibRaw が無効な build では `None`）。
    pub raw_decoder: Option<String>,
    /// テスト用の口が有効か。
    pub test_hooks: bool,
    /// メモリの上限の状態。
    pub memory_limit: MemoryLimitStatus,
}

/// ワーカーのメモリの上限の状態（04 の 1.2 節、AR-10）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MemoryLimitStatus {
    /// 上限を指定していない。
    NotRequested,
    /// 上限を設定した（Linux の `RLIMIT_DATA`）。
    Applied {
        /// 上限（バイト）。
        bytes: u64,
    },
    /// 上限を指定したが、設定できなかった（この OS では未対応、または失敗）。
    NotApplied {
        /// 指定した上限（バイト）。
        bytes: u64,
        /// 理由。
        reason: String,
    },
}

/// 写真の形式（ワーカーがファイルの先頭のバイト列と拡張子から判定する。
/// [`crate::worker::detect_photo_format`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotoFormat {
    /// RAW（LibRaw で扱う）。
    Raw,
    /// JPEG。
    Jpeg,
    /// PNG。
    Png,
    /// TIFF（RAW 以外）。
    Tiff,
}

/// 色を解釈したプロファイルの要約（RAW 以外。[`genzo_media::SourceProfile`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSummary {
    /// 埋め込みの ICC プロファイルを使ったか（`false` なら sRGB とみなした）。
    pub embedded: bool,
    /// 埋め込みのプロファイルの説明（'desc' タグ）。
    pub description: Option<String>,
    /// sRGB とみなした理由（埋め込みを使った場合は `None`）。
    pub assumed_srgb_reason: Option<String>,
}

/// [`Job::ProbePhoto`] の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoProbe {
    /// 形式。
    pub format: PhotoFormat,
    /// 撮影情報（幅・高さは向きを反映する前の寸法）。
    pub metadata: PhotoMetadata,
    /// 色のプロファイル（RAW では `None`）。
    pub profile: Option<ProfileSummary>,
}

/// [`Job::DecodeRaw`] の付随情報（CFA の値そのものは共有メモリ）。
///
/// 本体は、共有メモリの CFA と合わせて `genzo_raw::RawImage` を組み立て、
/// `RawImage::validate` で検証してから使う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawFrameInfo {
    /// 幅（有効画素）。
    pub width: u32,
    /// 高さ（有効画素）。
    pub height: u32,
    /// CFA の配列。
    pub cfa: CfaPattern,
    /// 黒レベル（R, G1, B, G2 の順）。
    pub black_level: [f32; 4],
    /// 白レベル。
    pub white_level: f32,
    /// 撮影時の WB の係数（G を 1 に正規化したもの）。
    pub as_shot_wb: [f32; 4],
    /// XYZ（D65）→ カメラ RGB の行列。
    pub cam_xyz: Option<[[f32; 3]; 3]>,
    /// 撮影情報。
    pub metadata: PhotoMetadata,
    /// デコーダの識別子（04 の 2.5 節の `render_deps.raw_decoder`。例: `"libraw-0.21.2"`）。
    pub decoder_id: Option<String>,
}

/// [`Job::DecodeImage`] の付随情報（画素は共有メモリ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinearImageInfo {
    /// 元の形式。
    pub format: PhotoFormat,
    /// 幅（向きを反映する前）。
    pub width: u32,
    /// 高さ（向きを反映する前）。
    pub height: u32,
    /// 元の画像の 1 チャンネルのビット数（8 または 16）。
    pub source_bits: u8,
    /// 撮影情報（向きは Exif の値。画素には反映していない）。
    pub metadata: PhotoMetadata,
    /// 色を解釈したプロファイル。
    pub profile: ProfileSummary,
    /// アルファチャンネルを捨てたか。
    pub alpha_dropped: bool,
}

/// サムネイルの元。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailSource {
    /// RAW に埋め込まれた JPEG。
    EmbeddedJpeg,
    /// RAW に埋め込まれたビットマップ（8bit の RGB / グレー）。
    EmbeddedBitmap,
    /// RAW 以外の画像そのもの。
    Image,
    /// 動画の代表フレーム。
    VideoFrame,
}

/// 動画のサムネイルの付随情報。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFrameInfo {
    /// 取り出したフレームの位置（ミリ秒）。
    pub position_ms: u64,
    /// 色の扱い（IQ-09）。
    pub color: VideoColorHandling,
}

/// [`Job::Thumbnail`]・[`Job::VideoThumbnail`] の付随情報（JPEG は共有メモリ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbnailInfo {
    /// 幅（向きを反映した後）。
    pub width: u32,
    /// 高さ（向きを反映した後）。
    pub height: u32,
    /// JPEG のバイト数。
    pub byte_len: u64,
    /// 元。
    pub source: ThumbnailSource,
    /// 動画の場合の付随情報。
    pub video: Option<VideoFrameInfo>,
}

/// ワーカーで起きたエラー（ワーカーは動き続ける）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}（{kind:?}）")]
pub struct WorkerError {
    /// 種類。
    pub kind: WorkerErrorKind,
    /// 説明（利用者に見せる理由）。
    pub message: String,
}

impl WorkerError {
    /// エラーを作る。
    pub fn new(kind: WorkerErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// [`WorkerError`] の種類。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum WorkerErrorKind {
    /// ファイルが見つからない。
    NotFound,
    /// ファイルの入出力のエラー（権限など）。
    Io,
    /// 対応していない形式（LibRaw が無効な build での RAW、埋め込みサムネイルがないなど）。
    Unsupported,
    /// 壊れたファイル（デコードできない）。
    Decode,
    /// 画素数・ファイルの大きさが上限を超える。
    TooLarge,
    /// 外部プログラム（ffprobe / ffmpeg）のエラー。
    Tool,
    /// 依頼が不正（解釈できない、引数が範囲外、テスト用の口が無効など）。
    InvalidRequest,
    /// 共有メモリが足りない。本体は `required_bytes` 以上を確保し直して、同じ依頼をもう一度送る。
    ShmTooSmall {
        /// 必要な大きさ（バイト。ヘッダを含む）。
        required_bytes: u64,
    },
    /// 共有メモリを開けない・大きさが合わない。
    Shm,
    /// ワーカーの内部のエラー。
    Internal,
}

/// パスを JSON で表す（UTF-8 でないパスも失わない）。
///
/// UTF-8 のパスは文字列、そうでないパスは Unix ではバイト列（`{"unix_bytes":[..]}`）、Windows では
/// UTF-16 の列（`{"windows_wide":[..]}`）で表す。別の OS の形式は読めない（本体とワーカーは同じ
/// OS で動く）。
pub mod wire_path {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Utf8(String),
        UnixBytes { unix_bytes: Vec<u8> },
        WindowsWide { windows_wide: Vec<u16> },
    }

    /// パスを JSON の値にする。
    pub fn serialize<S: Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
        to_repr(path).serialize(s)
    }

    /// JSON の値からパスを作る。
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        from_repr(Repr::deserialize(d)?).map_err(serde::de::Error::custom)
    }

    fn to_repr(path: &Path) -> Repr {
        if let Some(s) = path.to_str() {
            return Repr::Utf8(s.to_owned());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Repr::UnixBytes {
                unix_bytes: path.as_os_str().as_bytes().to_vec(),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Repr::WindowsWide {
                windows_wide: path.as_os_str().encode_wide().collect(),
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            Repr::Utf8(path.to_string_lossy().into_owned())
        }
    }

    fn from_repr(repr: Repr) -> Result<PathBuf, &'static str> {
        match repr {
            Repr::Utf8(s) => Ok(PathBuf::from(s)),
            Repr::UnixBytes { unix_bytes } => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    Ok(PathBuf::from(OsString::from_vec(unix_bytes)))
                }
                #[cfg(not(unix))]
                {
                    let _ = unix_bytes;
                    Err("Unix のバイト列のパスは、この OS では使えない")
                }
            }
            Repr::WindowsWide { windows_wide } => {
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStringExt;
                    Ok(PathBuf::from(OsString::from_wide(&windows_wide)))
                }
                #[cfg(not(windows))]
                {
                    let _ = windows_wide;
                    Err("Windows の UTF-16 のパスは、この OS では使えない")
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct P(#[serde(with = "super")] PathBuf);

        #[test]
        fn utf8_paths_are_plain_strings() {
            let p = P(PathBuf::from("写真/2024 05/a.ARW"));
            let json = serde_json::to_string(&p).unwrap();
            assert_eq!(json, r#""写真/2024 05/a.ARW""#);
            assert_eq!(serde_json::from_str::<P>(&json).unwrap(), p);
        }

        #[cfg(unix)]
        #[test]
        fn non_utf8_unix_paths_round_trip() {
            use std::os::unix::ffi::OsStringExt;
            let p = P(PathBuf::from(OsString::from_vec(vec![
                b'a', 0xFF, b'/', 0xC3,
            ])));
            let json = serde_json::to_string(&p).unwrap();
            assert!(json.contains("unix_bytes"), "{json}");
            assert_eq!(serde_json::from_str::<P>(&json).unwrap(), p);
            // Windows の形式は Unix では読めない。
            assert!(serde_json::from_str::<P>(r#"{"windows_wide":[97]}"#).is_err());
        }

        #[cfg(windows)]
        #[test]
        fn non_utf16_windows_paths_round_trip() {
            use std::os::windows::ffi::OsStringExt;
            // 対になっていないサロゲート（UTF-8 にできない）。
            let p = P(PathBuf::from(OsString::from_wide(&[0x61, 0xD800, 0x62])));
            let json = serde_json::to_string(&p).unwrap();
            assert!(json.contains("windows_wide"), "{json}");
            assert_eq!(serde_json::from_str::<P>(&json).unwrap(), p);
            assert!(serde_json::from_str::<P>(r#"{"unix_bytes":[97]}"#).is_err());
        }

        #[test]
        fn malformed_values_are_rejected() {
            assert!(serde_json::from_str::<P>("42").is_err());
            assert!(serde_json::from_str::<P>(r#"{"other":1}"#).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shm() -> ShmRef {
        ShmRef {
            path: PathBuf::from("/tmp/genzo-shm-x/1.shm"),
            capacity: 4096,
            checksum: true,
        }
    }

    #[test]
    fn requests_round_trip() {
        let jobs = vec![
            Job::Ping,
            Job::Shutdown,
            Job::ProbePhoto {
                path: "a.jpg".into(),
            },
            Job::ProbeVideo {
                path: "b.mp4".into(),
            },
            Job::DecodeRaw {
                path: "c.arw".into(),
                shm: shm(),
            },
            Job::DecodeImage {
                path: "d.tif".into(),
                shm: shm(),
            },
            Job::Thumbnail {
                path: "e.dng".into(),
                max_edge: 320,
                quality: Some(80),
                shm: shm(),
            },
            Job::VideoThumbnail {
                path: "f.mov".into(),
                max_edge: 2560,
                quality: None,
                shm: shm(),
            },
            Job::TestCrash {
                shm: Some(shm()),
                leave_child: true,
            },
            Job::TestHang { shm: None },
            Job::TestGarbage {
                mode: GarbageMode::WrongId,
            },
            Job::TestOversize {
                mode: OversizeMode::BadMagic,
                shm: shm(),
            },
            Job::TestAllocate { bytes: 1 << 20 },
        ];
        for (i, job) in jobs.into_iter().enumerate() {
            let req = Request { id: i as u64, job };
            let line = serde_json::to_string(&req).unwrap();
            assert!(!line.contains('\n'), "{line}");
            assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), req);
        }
    }

    #[test]
    fn wire_names_are_stable() {
        let line = serde_json::to_string(&Request {
            id: 3,
            job: Job::DecodeRaw {
                path: "a.arw".into(),
                shm: shm(),
            },
        })
        .unwrap();
        assert_eq!(
            line,
            r#"{"id":3,"job":{"type":"decode_raw","path":"a.arw","shm":{"path":"/tmp/genzo-shm-x/1.shm","capacity":4096,"checksum":true}}}"#
        );
        let err = Response {
            id: 3,
            result: Err(WorkerError::new(
                WorkerErrorKind::ShmTooSmall {
                    required_bytes: 100,
                },
                "足りない",
            )),
        };
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"id":3,"result":{"Err":{"kind":{"code":"shm_too_small","required_bytes":100},"message":"足りない"}}}"#
        );
        // 省略できる項目（品質・チェックサム）は無くてもよい。
        let req: Request = serde_json::from_str(
            r#"{"id":1,"job":{"type":"thumbnail","path":"x","max_edge":320,"shm":{"path":"s","capacity":64}}}"#,
        )
        .unwrap();
        assert_eq!(
            req.job,
            Job::Thumbnail {
                path: "x".into(),
                max_edge: 320,
                quality: None,
                shm: ShmRef {
                    path: "s".into(),
                    capacity: 64,
                    checksum: false
                }
            }
        );
    }

    #[test]
    fn responses_round_trip() {
        let outputs = vec![
            JobOutput::Pong(PongInfo {
                protocol_version: PROTOCOL_VERSION,
                worker_version: "0.1.0".into(),
                pid: 42,
                raw_decoder: Some("libraw-0.21.2".into()),
                test_hooks: false,
                memory_limit: MemoryLimitStatus::NotApplied {
                    bytes: 1 << 30,
                    reason: "未対応".into(),
                },
            }),
            JobOutput::ShuttingDown,
            JobOutput::Photo(PhotoProbe {
                format: PhotoFormat::Jpeg,
                metadata: PhotoMetadata {
                    iso: Some(100),
                    ..Default::default()
                },
                profile: Some(ProfileSummary {
                    embedded: false,
                    description: None,
                    assumed_srgb_reason: Some("NoProfile".into()),
                }),
            }),
            JobOutput::Video(VideoProbe::default()),
            JobOutput::Raw(RawFrameInfo {
                width: 64,
                height: 48,
                cfa: CfaPattern::GBRG,
                black_level: [510.0, 512.0, 514.0, 516.0],
                white_level: 16383.0,
                as_shot_wb: [2.25, 1.0, 1.625, 1.0],
                cam_xyz: Some([
                    [0.7424, -0.2329, -0.0466],
                    [-0.4598, 1.2471, 0.2347],
                    [-0.0715, 0.1505, 0.6066],
                ]),
                metadata: PhotoMetadata::default(),
                decoder_id: None,
            }),
            JobOutput::Image(LinearImageInfo {
                format: PhotoFormat::Png,
                width: 3,
                height: 2,
                source_bits: 16,
                metadata: PhotoMetadata::default(),
                profile: ProfileSummary {
                    embedded: true,
                    description: Some("Display P3".into()),
                    assumed_srgb_reason: None,
                },
                alpha_dropped: true,
            }),
            JobOutput::Thumbnail(ThumbnailInfo {
                width: 320,
                height: 213,
                byte_len: 12345,
                source: ThumbnailSource::VideoFrame,
                video: Some(VideoFrameInfo {
                    position_ms: 300,
                    color: VideoColorHandling::HdrNotToneMapped,
                }),
            }),
            JobOutput::Allocated { bytes: 7 },
        ];
        for (i, out) in outputs.into_iter().enumerate() {
            let resp = Response {
                id: i as u64,
                result: Ok(out),
            };
            let line = serde_json::to_string(&resp).unwrap();
            assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), resp);
        }
    }

    #[test]
    fn job_kind_and_path() {
        let j = Job::Thumbnail {
            path: "x.jpg".into(),
            max_edge: 1,
            quality: None,
            shm: shm(),
        };
        assert_eq!(j.kind(), JobKind::Thumbnail);
        assert_eq!(j.path(), Some(Path::new("x.jpg")));
        assert_eq!(Job::Ping.path(), None);
        assert_eq!(Job::TestAllocate { bytes: 1 }.kind(), JobKind::TestHook);
    }

    #[test]
    fn lines_are_read_with_a_limit() {
        let mut r = io::Cursor::new(b"abc\r\n\nxyz".to_vec());
        assert_eq!(
            read_line_limited(&mut r, 8).unwrap(),
            LineRead::Line(b"abc".to_vec())
        );
        assert_eq!(
            read_line_limited(&mut r, 8).unwrap(),
            LineRead::Line(vec![])
        );
        assert_eq!(
            read_line_limited(&mut r, 8).unwrap(),
            LineRead::Line(b"xyz".to_vec()),
            "改行のない最後の行"
        );
        assert_eq!(read_line_limited(&mut r, 8).unwrap(), LineRead::Eof);
        // ちょうど上限の長さは読める。超えたら TooLong。
        let mut r = io::Cursor::new(b"12345678\n123456789\n".to_vec());
        assert_eq!(
            read_line_limited(&mut r, 8).unwrap(),
            LineRead::Line(b"12345678".to_vec())
        );
        assert_eq!(read_line_limited(&mut r, 8).unwrap(), LineRead::TooLong);
    }

    #[test]
    fn unknown_jobs_fail_to_parse() {
        assert!(
            serde_json::from_str::<Request>(r#"{"id":1,"job":{"type":"format_disk"}}"#).is_err()
        );
        assert!(serde_json::from_str::<Request>(r#"{"id":1}"#).is_err());
    }
}
