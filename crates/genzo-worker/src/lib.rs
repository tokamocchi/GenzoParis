//! ワーカープロセスと、本体側のワーカー管理。
//!
//! 信頼できない入力（写真・動画）の解析とデコードを、本体とは別のプロセスで行う
//! （docs/04_architecture.md の 1.1 節・1.2 節、02 の SEC-05。対話的な処理も例外にしない）。
//! 壊れたファイルでデコーダが異常終了・ハングしても、本体は落ちずにワーカーを再起動し、そのファイル
//! だけをエラーにする（6.3 節）。
//!
//! | モジュール | 内容 | 設計書 |
//! |---|---|---|
//! | [`protocol`] | 制御のメッセージ（標準入出力、1 行 1 メッセージの JSON） | 1.2 節「ワーカーとのやり取り」 |
//! | [`shm`] | 共有メモリ（本体が確保して所有し、ワーカーは書き込むだけ。ファイルを使う方式） | 1.2 節、AR-10、PoC-2 |
//! | [`worker`] | ワーカーの処理（バイナリ `genzo-worker`。本体の実行ファイルの隠しサブコマンドからは [`run_worker`]） | 1.2 節、2.6 節「RAW 以外の入力」、4 章 |
//! | [`client`] | 本体側: 1 つのワーカーの起動・タイムアウト・再起動・取り消し・バッファの検証 | 1.2 節、6.1 節、6.3 節 |
//! | [`pool`] | 本体側: 対話用 1 個とバッチ用 1〜2 個のワーカー | 1.2 節のプロセス構成 |
//! | [`failures`] | 同じファイルで 2 回続けて失敗したらスキップ | 1.2 節、6.3 節 |
//! | [`limits`] | ワーカーのメモリの上限（Linux だけ） | 1.2 節、AR-10 |
//!
//! # ジョブ
//!
//! | ジョブ | ワーカーで使う処理 | 結果（JSON） | 共有メモリ |
//! |---|---|---|---|
//! | `ProbePhoto` | RAW: `genzo_raw::read_metadata`、JPEG / TIFF / PNG: `genzo_media::probe_image_file` | [`PhotoProbe`] | なし |
//! | `ProbeVideo` | ffprobe（`genzo_media::FfmpegTools::probe`） | `genzo_media::VideoProbe` | なし |
//! | `DecodeRaw` | `genzo_raw::decode_file` | [`RawFrameInfo`] | CFA（u16） |
//! | `DecodeImage` | `genzo_media::decode_image_file` ＋ lcms2 でリニア BT.2020 へ | [`LinearImageInfo`] | RGB（f32） |
//! | `Thumbnail` | RAW: 埋め込みの JPEG（`genzo_raw::extract_thumbnail`）、それ以外: 画像そのもの → B5 | [`ThumbnailInfo`] | JPEG |
//! | `VideoThumbnail` | ffmpeg（`genzo_media::FfmpegTools::thumbnail`）→ B5 | [`ThumbnailInfo`] | JPEG |
//!
//! 機能フラグ `libraw` が無効な build では、RAW のジョブは [`WorkerErrorKind::Unsupported`] になる。
//!
//! # 例
//!
//! ```no_run
//! use genzo_jobs::CancellationToken;
//! use genzo_media::CacheSpec;
//! use genzo_worker::{Lane, PoolConfig, WorkerPool};
//!
//! let pool = WorkerPool::new(PoolConfig::default())?;
//! let cancel = CancellationToken::new();
//! let path = std::path::Path::new("/photos/DSC00001.ARW");
//! let probe = pool.probe_photo(Lane::Batch, path, &cancel)?;
//! let thumb = pool.thumbnail(Lane::Batch, path, CacheSpec::L0_THUMBNAIL, &cancel)?;
//! let raw = pool.decode_raw(Lane::Interactive, path, &cancel)?; // 検証済みの RawImage
//! # let _ = (probe, thumb, raw);
//! # Ok::<(), genzo_worker::WorkerClientError>(())
//! ```
//!
//! # 設計からの補足
//!
//! - 共有メモリは、本体の一時ディレクトリのファイルを使う方式（[`shm`]）。ワーカーはマップして書き、
//!   本体はマップせずに読み取りで自分のメモリに複製する（乗っ取られたワーカーがファイルを切り詰めても、
//!   本体が SIGBUS で落ちないため）。OS ごとの名前付き共有メモリとの比較は PoC-2 で行う。
//! - 本体は受け取ったデータを自分のメモリに複製してから検証して使う（その後でワーカーが書き換えても
//!   影響しない）。複製の費用は PoC-2 で計測する。
//! - ワーカーのメモリの上限は Linux でだけ設定する（Windows・macOS は PoC-2 で確認）。
//! - ワーカーが ffmpeg を実行中に強制終了された場合、ffmpeg は出力先が閉じた時点で終わる
//!   （孫プロセスをまとめて終了させる仕組み（ジョブオブジェクト・プロセスグループ）は PoC-2 で検討。
//!   ワーカーは 2026-10-10 から別のプロセスグループで起動し（端末の Ctrl+C を受け取らないため。K3）、
//!   Unix では孫プロセスも同じグループに入るが、グループごとの終了はまだ使っていない）。
//!   孫プロセスがパイプを持ったままでも、本体はワーカーのプロセスの終了で異常終了に気づく
//!   （[`client`]）。

pub mod client;
pub mod failures;
pub mod limits;
pub mod pool;
pub mod protocol;
pub mod shm;
mod validate;
pub mod worker;

pub use client::{
    InitialShmBytes, JobTimeouts, LinearImage, RawFrame, TestHook, TestHookOutput, Thumbnail,
    WorkerClient, WorkerClientError, WorkerConfig, WorkerStats, locate_worker_executable,
};
pub use failures::{FailureTracker, MAX_CONSECUTIVE_FAILURES};
pub use pool::{Lane, MAX_BATCH_WORKERS, PoolConfig, WorkerLease, WorkerPool};
pub use protocol::{
    GarbageMode, JobKind, LinearImageInfo, MemoryLimitStatus, OversizeMode, PROTOCOL_VERSION,
    PhotoFormat, PhotoProbe, PongInfo, ProfileSummary, RawFrameInfo, ThumbnailInfo,
    ThumbnailSource, VideoFrameInfo, WorkerError, WorkerErrorKind,
};
pub use shm::{BufferError, PayloadKind, ShmArena};
pub use validate::{MAX_THUMBNAIL_EDGE, MAX_THUMBNAIL_JPEG_BYTES};
pub use worker::{main_entry, run_worker};

/// ワーカーの実行ファイルを指定する環境変数（[`locate_worker_executable`]）。
pub const ENV_WORKER_PATH: &str = "GENZO_WORKER_PATH";

/// ワーカーのテスト用の口を有効にする環境変数（値が `1` のとき）。本番では設定しない。
pub const ENV_TEST_HOOKS: &str = "GENZO_WORKER_TEST_HOOKS";

/// ワーカーの実行ファイルの名前（拡張子を除く）。
pub const WORKER_EXE_NAME: &str = "genzo-worker";

/// 本体の実行ファイル自身をワーカーとして起動するときの隠しサブコマンドの名前（`genzo __worker`）。
///
/// 本体の `main` は、最初の引数がこの名前なら [`run_worker`] を呼ぶ（`relaunch_prefix` には
/// `[WORKER_SUBCOMMAND]`、`args` には残りの引数を渡す）。本体の側は [`WorkerConfig::args`] に
/// `[WORKER_SUBCOMMAND]`、[`WorkerConfig::executable`] に本体の実行ファイルを設定して起動する。
pub const WORKER_SUBCOMMAND: &str = "__worker";
