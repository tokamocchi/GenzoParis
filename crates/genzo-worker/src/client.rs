//! 本体側の、1 つのワーカープロセスの管理（docs/04_architecture.md の 1.2 節「ワーカーとのやり取り」、
//! 6.3 節「エラー処理」）。
//!
//! - 起動: 実行ファイル（[`locate_worker_executable`]）を起動し、[`Job::Ping`] でプロトコルの版を
//!   確かめる（握手）。
//! - 依頼: 1 つのワーカーには同時に 1 件だけ依頼する（[`WorkerClient`] のメソッドは `&mut self`）。
//!   複数のワーカーを並行して使うときは [`crate::WorkerPool`] を使う。
//! - タイムアウト: ジョブの種類ごと（[`JobTimeouts`]）。超えたらワーカーを強制終了して再起動し、
//!   そのジョブは [`WorkerClientError::Timeout`] にする。
//! - 異常終了: 応答の前に出力が閉じた（EOF）・ワーカーのプロセスが終わった・依頼を送れない場合は、
//!   ワーカーを回収して再起動し、[`WorkerClientError::Crashed`]（終了状態と標準エラー出力の末尾付き）
//!   にする。プロセスの終了も確かめるのは、孫プロセス（ffmpeg など）が制御のパイプを引き継いで
//!   いると、ワーカーが落ちても出力が閉じないため（Windows ではワーカーの標準入出力のハンドルを
//!   引き継ぐ）。
//! - 不正な応答: JSON でない・番号が違う・長すぎる行・種類が違う応答は、ワーカーを再起動して
//!   [`WorkerClientError::Protocol`] にする。
//! - 不正なバッファ: 共有メモリのヘッダ・寸法・長さ・型・上限を検証し（`crate::validate`）、
//!   不正なら破棄し、ワーカーを再起動して [`WorkerClientError::InvalidBuffer`] にする。
//! - 取り消し: [`CancellationToken`] が取り消されたら、実行中のワーカーを強制終了して再起動し、
//!   [`WorkerClientError::Cancelled`] にする（対話用の古い要求を捨てるため。6.1 節）。ワーカーの中の
//!   処理（LibRaw など）は途中で止められないので、プロセスごと終わらせる。
//! - スキップ: 同じファイルで 2 回続けて失敗したら、以後は依頼せずに [`WorkerClientError::Skipped`]
//!   を返す（[`crate::failures`]）。
//! - 共有メモリ: 依頼ごとに [`ShmArena`] に作り、結果を本体のメモリに複製したら（失敗した場合も）
//!   削除する。足りなければワーカーが返す大きさで確保し直して 1 回だけ送り直す。種類ごとに、
//!   直近に必要だった大きさを覚えて次の確保に使う。
//!
//! 再起動はその場で行う（対話用のワーカーを常に待機させておくため。1.2 節）。再起動に失敗した
//! 場合は、次の依頼のときにもう一度起動を試みる。

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use genzo_jobs::CancellationToken;
use genzo_media::{CacheSpec, MAX_IMAGE_PIXELS, VideoProbe};
use genzo_raw::{MAX_PIXELS, RawImage};

use crate::failures::{FailureTracker, MAX_CONSECUTIVE_FAILURES};
use crate::limits::MIN_MEMORY_LIMIT_BYTES;
use crate::protocol::{
    GarbageMode, Job, JobKind, JobOutput, LineRead, LinearImageInfo, MAX_LINE_BYTES, OversizeMode,
    PROTOCOL_VERSION, PhotoProbe, PongInfo, RawFrameInfo, Request, Response, ShmRef, ThumbnailInfo,
    ThumbnailSource, VideoFrameInfo, WorkerError, WorkerErrorKind, read_line_limited,
};
use crate::shm::{BufferError, PayloadKind, SHM_HEADER_LEN, ShmArena, ShmBuffer};
use crate::validate::{self, MAX_THUMBNAIL_EDGE, MAX_THUMBNAIL_JPEG_BYTES};
use crate::{ENV_TEST_HOOKS, ENV_WORKER_PATH, WORKER_EXE_NAME};

/// [`Job::Ping`] のタイムアウトの既定値（**仮置き**: 5 秒）。
pub const DEFAULT_PING_TIMEOUT: Duration = Duration::from_secs(5);
/// [`Job::ProbePhoto`] のタイムアウトの既定値（**仮置き**: 10 秒。メタデータの読み取りは通常
/// 数十 ms。ネットワークドライブや起動直後の HDD でも誤って打ち切らない長さ）。
pub const DEFAULT_PROBE_PHOTO_TIMEOUT: Duration = Duration::from_secs(10);
/// [`Job::ProbeVideo`] のタイムアウトの既定値（**仮置き**: 30 秒。ワーカーの中の ffprobe の
/// タイムアウト（`genzo_media::video::DEFAULT_PROBE_TIMEOUT`、20 秒）より長くし、ffprobe の
/// ハングはワーカーが通常のエラーとして返せるようにする）。
pub const DEFAULT_PROBE_VIDEO_TIMEOUT: Duration = Duration::from_secs(30);
/// [`Job::DecodeRaw`] のタイムアウトの既定値（**仮置き**: 30 秒。α7 IV の展開は 1 秒程度の
/// 見込みで、十分な余裕を見た値。PoC-2 で計測して見直す）。
pub const DEFAULT_DECODE_RAW_TIMEOUT: Duration = Duration::from_secs(30);
/// [`Job::DecodeImage`] のタイムアウトの既定値（**仮置き**: 30 秒。PoC-2 で見直す）。
pub const DEFAULT_DECODE_IMAGE_TIMEOUT: Duration = Duration::from_secs(30);
/// [`Job::Thumbnail`] のタイムアウトの既定値（**仮置き**: 20 秒。RAW 以外の大きな画像は全体を
/// デコードしてから縮小するため、展開より短くはしない）。
pub const DEFAULT_THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(20);
/// [`Job::VideoThumbnail`] のタイムアウトの既定値（**仮置き**: 90 秒。ワーカーの中の ffprobe
/// （20 秒）と、代表フレーム・先頭のフレームの 2 回の ffmpeg（各 30 秒）の合計より長くする。
/// PERF-11（1 本あたり 2 秒）とは別の、ハングへの備え。PoC-7 で見直す）。
pub const DEFAULT_VIDEO_THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(90);
/// テスト用のジョブのタイムアウトの既定値（**仮置き**: 10 秒）。
pub const DEFAULT_TEST_HOOK_TIMEOUT: Duration = Duration::from_secs(10);
/// [`Job::Shutdown`] の応答と終了を待つ時間の既定値（**仮置き**: 5 秒）。
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// 起動と握手（[`Job::Ping`]）のタイムアウトの既定値（**仮置き**: 10 秒。LibRaw などの読み込みと
/// ウイルス対策ソフトの検査を見込む）。
pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

const _: () = {
    assert!(
        DEFAULT_PROBE_VIDEO_TIMEOUT.as_secs() > genzo_media::video::DEFAULT_PROBE_TIMEOUT.as_secs()
    );
    assert!(
        DEFAULT_VIDEO_THUMBNAIL_TIMEOUT.as_secs()
            > genzo_media::video::DEFAULT_PROBE_TIMEOUT.as_secs()
                + 2 * genzo_media::video::DEFAULT_FRAME_TIMEOUT.as_secs()
    );
};

/// [`Job::DecodeRaw`] の共有メモリの最初の大きさ（**仮置き**: 72 MiB。α7 IV の有効画素
/// （約 3,300 万画素 × 2 バイト ≒ 66 MB）が収まる大きさ。PoC-2 で実際のバッファの大きさに
/// 合わせる）。足りなければ確保し直す。ファイルは確保しただけでは物理メモリを使わない。
pub const INITIAL_RAW_SHM_BYTES: u64 = 72 * 1024 * 1024;
/// [`Job::DecodeImage`] の共有メモリの最初の大きさ（**仮置き**: 64 MiB。約 560 万画素の
/// f32 の RGB。大きな画像は 1 回目で必要な大きさを受け取って確保し直す）。
pub const INITIAL_IMAGE_SHM_BYTES: u64 = 64 * 1024 * 1024;
/// サムネイルの共有メモリの最初の大きさ（**仮置き**: 4 MiB。L1（長辺 2560px、品質 85）の
/// JPEG は 1〜2 MB 程度の見込み）。
pub const INITIAL_THUMBNAIL_SHM_BYTES: u64 = 4 * 1024 * 1024;
/// テスト用のジョブの共有メモリの大きさ。
const TEST_HOOK_SHM_BYTES: u64 = 64 * 1024;

/// 共有メモリの最初の大きさ（ヘッダを含む。種類ごと）。
///
/// 足りなければワーカーが必要な大きさを返し、確保し直す（[`WorkerClient`] は種類ごとに直近の
/// 大きさを覚えて、次から使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitialShmBytes {
    /// [`Job::DecodeRaw`]。
    pub raw: u64,
    /// [`Job::DecodeImage`]。
    pub image: u64,
    /// [`Job::Thumbnail`]・[`Job::VideoThumbnail`]。
    pub thumbnail: u64,
}

impl Default for InitialShmBytes {
    fn default() -> Self {
        Self {
            raw: INITIAL_RAW_SHM_BYTES,
            image: INITIAL_IMAGE_SHM_BYTES,
            thumbnail: INITIAL_THUMBNAIL_SHM_BYTES,
        }
    }
}

/// 標準エラー出力のうち残す末尾のバイト数（異常終了の説明に付ける）。
pub const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// 異常終了したワーカーの終了を待つ時間（出力が閉じた後）。
const CRASH_REAP_GRACE: Duration = Duration::from_secs(2);
/// 強制終了したワーカーの終了を待つ時間。
///
/// **仮置き**: 2 秒。応答のないネットワークドライブの入出力の途中などでは、強制終了してもすぐには
/// 終わらないことがある（Linux の D 状態など）。その場合は別のスレッドで回収を待ち、本体は先に
/// 進む（本体が止まらないことを優先する）。
const KILL_REAP_GRACE: Duration = Duration::from_secs(2);
/// 応答を読むスレッドと本体の間のチャネルの大きさ（通常は 1 件。ワーカーが応答を大量に出しても、
/// 本体のメモリを使い切らないように上限を設ける）。
const EVENT_CHANNEL_CAPACITY: usize = 4;
/// 異常終了したワーカーの標準エラー出力を読み終えるのを待つ時間。
const STDERR_DRAIN_GRACE: Duration = Duration::from_secs(1);
/// 終了を確かめる間隔。
const REAP_POLL_INTERVAL: Duration = Duration::from_millis(5);
/// 応答を待つ間に、ワーカーのプロセスが終わっていないかを確かめる間隔。
///
/// **仮置き**: 100 ms。出力が閉じない（孫プロセスがパイプを引き継いでいる）まま、ワーカーが
/// 落ちた場合に、異常終了に気づくまでの遅れ（＋[`EXIT_DRAIN_GRACE`]）。
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// ワーカーのプロセスが終わったのを見つけた後、終わる前に書かれた応答（[`Job::Shutdown`] の応答
/// など）が届くのを待つ時間（**仮置き**: 1 秒）。
const EXIT_DRAIN_GRACE: Duration = Duration::from_secs(1);

/// ジョブの種類ごとのタイムアウト。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobTimeouts {
    /// [`Job::Ping`]。
    pub ping: Duration,
    /// [`Job::ProbePhoto`]。
    pub probe_photo: Duration,
    /// [`Job::ProbeVideo`]。
    pub probe_video: Duration,
    /// [`Job::DecodeRaw`]。
    pub decode_raw: Duration,
    /// [`Job::DecodeImage`]。
    pub decode_image: Duration,
    /// [`Job::Thumbnail`]。
    pub thumbnail: Duration,
    /// [`Job::VideoThumbnail`]。
    pub video_thumbnail: Duration,
    /// テスト用のジョブ。
    pub test_hook: Duration,
    /// [`Job::Shutdown`]（応答と終了を待つ時間）。
    pub shutdown: Duration,
}

impl Default for JobTimeouts {
    fn default() -> Self {
        Self {
            ping: DEFAULT_PING_TIMEOUT,
            probe_photo: DEFAULT_PROBE_PHOTO_TIMEOUT,
            probe_video: DEFAULT_PROBE_VIDEO_TIMEOUT,
            decode_raw: DEFAULT_DECODE_RAW_TIMEOUT,
            decode_image: DEFAULT_DECODE_IMAGE_TIMEOUT,
            thumbnail: DEFAULT_THUMBNAIL_TIMEOUT,
            video_thumbnail: DEFAULT_VIDEO_THUMBNAIL_TIMEOUT,
            test_hook: DEFAULT_TEST_HOOK_TIMEOUT,
            shutdown: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

impl JobTimeouts {
    /// すべての種類で同じタイムアウト。
    pub fn uniform(timeout: Duration) -> Self {
        Self {
            ping: timeout,
            probe_photo: timeout,
            probe_video: timeout,
            decode_raw: timeout,
            decode_image: timeout,
            thumbnail: timeout,
            video_thumbnail: timeout,
            test_hook: timeout,
            shutdown: timeout,
        }
    }

    /// 種類 `kind` のタイムアウト。
    pub fn for_kind(&self, kind: JobKind) -> Duration {
        match kind {
            JobKind::Ping => self.ping,
            JobKind::Shutdown => self.shutdown,
            JobKind::ProbePhoto => self.probe_photo,
            JobKind::ProbeVideo => self.probe_video,
            JobKind::DecodeRaw => self.decode_raw,
            JobKind::DecodeImage => self.decode_image,
            JobKind::Thumbnail => self.thumbnail,
            JobKind::VideoThumbnail => self.video_thumbnail,
            JobKind::TestHook => self.test_hook,
        }
    }

    fn all(&self) -> [Duration; 9] {
        [
            self.ping,
            self.probe_photo,
            self.probe_video,
            self.decode_raw,
            self.decode_image,
            self.thumbnail,
            self.video_thumbnail,
            self.test_hook,
            self.shutdown,
        ]
    }
}

/// ワーカーの起動と管理の設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    /// ワーカーの実行ファイル。`None` なら [`locate_worker_executable`] で探す。
    pub executable: Option<PathBuf>,
    /// 実行ファイルの直後に付ける引数（既定は空）。
    ///
    /// 本体の実行ファイル自身を隠しサブコマンド付きでワーカーとして起動する構成（`genzo __worker`。
    /// [`crate::WORKER_SUBCOMMAND`]・[`crate::run_worker`]）では `["__worker"]` を渡す。ワーカーへの
    /// 引数（`--memory-limit-bytes` など）は、この後に付ける。
    pub args: Vec<OsString>,
    /// ジョブの種類ごとのタイムアウト。
    pub timeouts: JobTimeouts,
    /// 起動と握手のタイムアウト。
    pub startup_timeout: Duration,
    /// ワーカーのメモリの上限（バイト。04 の 1.2 節、AR-10）。`None` なら設定しない。
    ///
    /// Linux だけで有効（`RLIMIT_DATA`。[`crate::limits`]）。他の OS では設定されない
    /// （[`crate::protocol::MemoryLimitStatus::NotApplied`]。PoC-2 で確認する）。既定値は `None`
    /// （**仮置き**。ワーカー 1 つが使う量を PoC-2 で計測して決める）。
    pub memory_limit_bytes: Option<u64>,
    /// テスト用の口を有効にするか（ワーカーに環境変数 `GENZO_WORKER_TEST_HOOKS=1` を渡す）。
    /// `false` なら、本体の環境にその変数があってもワーカーには渡さない。
    pub test_hooks: bool,
    /// 共有メモリのチェックサムを求めて確かめるか（[`crate::shm::checksum`]）。
    ///
    /// 既定値は `false`（**仮置き**。データ全体を 1 回読む費用がかかる。PoC-2 で受け渡しの時間を
    /// 計測して決める）。
    pub verify_checksum: bool,
    /// 共有メモリの一時ディレクトリを置く場所。`None` なら OS の一時ディレクトリ。
    pub shm_root: Option<PathBuf>,
    /// 共有メモリの最初の大きさ。
    pub initial_shm: InitialShmBytes,
    /// ワーカーに追加で渡す環境変数（`GENZO_FFMPEG` など）。
    pub env: Vec<(OsString, OsString)>,
    /// 同じファイルで何回続けて失敗したらスキップするか（04 の 1.2 節の「2 回」）。
    pub max_consecutive_failures: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            executable: None,
            args: Vec::new(),
            timeouts: JobTimeouts::default(),
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            memory_limit_bytes: None,
            test_hooks: false,
            verify_checksum: false,
            shm_root: None,
            initial_shm: InitialShmBytes::default(),
            env: Vec::new(),
            max_consecutive_failures: MAX_CONSECUTIVE_FAILURES,
        }
    }
}

impl WorkerConfig {
    /// 値を確かめる。
    pub fn validate(&self) -> Result<(), WorkerClientError> {
        let bad = |m: String| Err(WorkerClientError::InvalidArgument(m));
        if self.timeouts.all().contains(&Duration::ZERO) || self.startup_timeout.is_zero() {
            return bad("タイムアウトは 0 より長くする".to_owned());
        }
        if let Some(limit) = self.memory_limit_bytes
            && limit < MIN_MEMORY_LIMIT_BYTES
        {
            return bad(format!(
                "メモリの上限 {limit} は {MIN_MEMORY_LIMIT_BYTES} バイト以上にする"
            ));
        }
        if self.max_consecutive_failures == 0 {
            return bad("スキップするまでの失敗の回数は 1 以上にする".to_owned());
        }
        let s = self.initial_shm;
        if [s.raw, s.image, s.thumbnail]
            .iter()
            .any(|&b| b < SHM_HEADER_LEN as u64)
        {
            return bad(format!(
                "共有メモリの最初の大きさは {SHM_HEADER_LEN} バイト以上にする（{s:?}）"
            ));
        }
        Ok(())
    }

    /// 共有メモリの一時ディレクトリを作る。
    pub fn create_arena(&self) -> Result<ShmArena, WorkerClientError> {
        let arena = match &self.shm_root {
            Some(root) => ShmArena::new_in(root),
            None => ShmArena::new(),
        };
        arena.map_err(|e| WorkerClientError::Shm(format!("一時ディレクトリを作れない: {e}")))
    }
}

/// ワーカーの実行ファイルを探す。
///
/// 1. `explicit`（[`WorkerConfig::executable`]）
/// 2. 環境変数 `GENZO_WORKER_PATH`（空でなければ）
/// 3. 本体の実行ファイルと同じフォルダの `genzo-worker`（Windows では `genzo-worker.exe`）
/// 4. 本体の実行ファイルが `deps` フォルダにある場合（cargo のテスト）は、その 1 つ上のフォルダ
///
/// 結合テストでは `env!("CARGO_BIN_EXE_genzo-worker")` を `explicit` に渡す。
pub fn locate_worker_executable(explicit: Option<&Path>) -> Result<PathBuf, WorkerClientError> {
    locate_with(
        explicit,
        std::env::var_os(ENV_WORKER_PATH),
        std::env::current_exe().ok(),
    )
}

fn locate_with(
    explicit: Option<&Path>,
    env_value: Option<OsString>,
    current_exe: Option<PathBuf>,
) -> Result<PathBuf, WorkerClientError> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Some(v) = env_value.filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(v));
    }
    let name = format!("{WORKER_EXE_NAME}{}", std::env::consts::EXE_SUFFIX);
    let mut tried = Vec::new();
    if let Some(dir) = current_exe.as_deref().and_then(Path::parent) {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        tried.push(candidate);
        if dir.file_name().is_some_and(|n| n == "deps")
            && let Some(parent) = dir.parent()
        {
            let candidate = parent.join(&name);
            if candidate.is_file() {
                return Ok(candidate);
            }
            tried.push(candidate);
        }
    }
    Err(WorkerClientError::Spawn(format!(
        "ワーカーの実行ファイルが見つからない（環境変数 {ENV_WORKER_PATH} で指定できる。探した場所: {tried:?}）"
    )))
}

/// 本体側で起きた、ワーカーのジョブのエラー。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkerClientError {
    /// ワーカーが通常のエラーとして返した（壊れたファイル・対応していない形式など。ワーカーは無事）。
    #[error("ワーカーでの処理が失敗しました: {0}")]
    Job(WorkerError),
    /// 時間内に応答がなかった（ワーカーを強制終了して再起動した）。
    #[error(
        "ワーカーが {timeout:?} 以内に応答しなかったため、強制終了して再起動しました（{kind:?}）"
    )]
    Timeout {
        /// ジョブの種類。
        kind: JobKind,
        /// タイムアウト。
        timeout: Duration,
    },
    /// ワーカーが異常終了した（再起動した）。
    #[error("ワーカーが異常終了しました（{detail}）")]
    Crashed {
        /// 終了状態など。
        detail: String,
        /// ワーカーの標準エラー出力の末尾。
        stderr_tail: String,
    },
    /// ワーカーの応答が不正（再起動した）。
    #[error("ワーカーの応答が不正です（{0}）")]
    Protocol(String),
    /// 受け取ったバッファが不正（破棄し、ワーカーを再起動した）。
    #[error("ワーカーから受け取ったバッファが不正です: {0}")]
    InvalidBuffer(#[from] BufferError),
    /// 同じファイルで続けて失敗したため、依頼せずにスキップした（04 の 6.3 節）。
    #[error("{} は {failures} 回続けてワーカーの処理に失敗したため、スキップしました", path.display())]
    Skipped {
        /// ファイル。
        path: PathBuf,
        /// 続けて失敗した回数。
        failures: u32,
    },
    /// 取り消された（実行中だった場合はワーカーを再起動した）。
    #[error("処理は取り消されました")]
    Cancelled,
    /// ワーカーを起動できない（実行ファイルがない、握手に失敗したなど）。
    #[error("ワーカーを起動できません: {0}")]
    Spawn(String),
    /// 共有メモリを用意できない。
    #[error("共有メモリを用意できません: {0}")]
    Shm(String),
    /// 引数・設定が不正。
    #[error("引数が不正です: {0}")]
    InvalidArgument(String),
}

impl WorkerClientError {
    /// 「同じファイルでの失敗」として数えるか（異常終了・タイムアウト・不正な応答・不正な
    /// バッファ）。[`crate::failures`] の説明。
    pub fn counts_toward_skip(&self) -> bool {
        matches!(
            self,
            Self::Timeout { .. }
                | Self::Crashed { .. }
                | Self::Protocol(_)
                | Self::InvalidBuffer(_)
        )
    }

    /// ワーカーが返したエラーの種類（[`Self::Job`] のときだけ）。
    pub fn worker_error_kind(&self) -> Option<&WorkerErrorKind> {
        match self {
            Self::Job(e) => Some(&e.kind),
            _ => None,
        }
    }
}

/// [`WorkerClient::decode_raw`] の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct RawFrame {
    /// 展開した RAW（`RawImage::validate` 済み）。
    pub image: RawImage,
    /// デコーダの識別子。
    pub decoder_id: Option<String>,
}

/// [`WorkerClient::decode_image`] の結果（リニア BT.2020。04 の 2.6 節の B2）。
#[derive(Debug, Clone, PartialEq)]
pub struct LinearImage {
    /// 画素（行優先。向きは反映していない）。
    pub pixels: Vec<[f32; 3]>,
    /// 付随情報（寸法・形式・撮影情報・プロファイル）。
    pub info: LinearImageInfo,
    /// NaN・無限大を 0 に置き換えた値の数（04 の 2.6 節）。
    pub non_finite_replaced: u64,
}

impl LinearImage {
    /// 幅。
    pub fn width(&self) -> u32 {
        self.info.width
    }

    /// 高さ。
    pub fn height(&self) -> u32 {
        self.info.height
    }
}

/// [`WorkerClient::thumbnail`]・[`WorkerClient::video_thumbnail`] の結果（B5 の JPEG）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// JPEG のバイト列（Display P3 の ICC プロファイル付き、向きを反映済み）。
    pub jpeg: Vec<u8>,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
    /// 元。
    pub source: ThumbnailSource,
    /// 動画の場合の付随情報。
    pub video: Option<VideoFrameInfo>,
}

/// テスト用の口（[`WorkerClient::run_test_hook`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestHook {
    /// 異常終了する。`with_shm` なら共有メモリを渡し、途中まで書かせる。
    Crash {
        /// 共有メモリを渡すか。
        with_shm: bool,
    },
    /// 標準出力・標準エラー出力を引き継いだ子プロセスを残して異常終了する（共有メモリを渡す。
    /// 子プロセスは共有メモリのファイルが削除されると終わる）。
    CrashLeavingChild,
    /// 応答しない。
    Hang {
        /// 共有メモリを渡すか。
        with_shm: bool,
    },
    /// 不正な行を出力する。
    Garbage(GarbageMode),
    /// 偽ったヘッダ・付随情報で RAW の展開の結果を返す（本体は RAW として検証する）。
    Oversize(OversizeMode),
    /// メモリを確保する。
    Allocate(u64),
}

/// テスト用の口の結果。
#[derive(Debug, Clone, PartialEq)]
pub enum TestHookOutput {
    /// [`TestHook::Oversize`] が検証を通った（通常は起きない）。
    Raw(Box<RawFrame>),
    /// [`TestHook::Allocate`] が確保できた量。
    Allocated(u64),
}

/// ワーカーの統計（診断・テスト用）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerStats {
    /// 起動した回数（最初の起動を含む）。
    pub spawns: u64,
    /// 再起動した回数（強制終了・異常終了の後）。
    pub restarts: u64,
    /// 受け取った応答の数（握手を除く）。
    pub responses: u64,
    /// タイムアウトの数。
    pub timeouts: u64,
    /// 異常終了の数。
    pub crashes: u64,
    /// 実行中に取り消した数。
    pub cancellations: u64,
    /// 不正な応答の数。
    pub protocol_errors: u64,
    /// 不正なバッファの数。
    pub invalid_buffers: u64,
}

/// 標準出力を読むスレッドから届くもの。
enum ReaderEvent {
    Line(Vec<u8>),
    Eof,
    TooLong,
    Error(String),
}

/// 起動中のワーカープロセス。drop すると強制終了して回収する。
struct WorkerProcess {
    /// 回収を別のスレッドに任せた後は `None`。
    child: Option<Child>,
    pid: u32,
    stdin: Option<ChildStdin>,
    events: Receiver<ReaderEvent>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    stderr_done: Receiver<()>,
    pong: Option<PongInfo>,
    exited: Option<ExitStatus>,
}

impl WorkerProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    /// 終わっているか（終わっていたら回収して終了状態を記録する）。回収を別のスレッドに任せた後は
    /// `false`。
    fn has_exited(&mut self) -> bool {
        if self.exited.is_some() {
            return true;
        }
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.exited = Some(status);
                true
            }
            _ => false,
        }
    }

    /// `grace` の間、終わるのを待つ（終わったら回収して終了状態を記録する）。
    fn wait_for_exit(&mut self, grace: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + grace;
        while self.exited.is_none() {
            let child = self.child.as_mut()?;
            match child.try_wait() {
                Ok(Some(status)) => self.exited = Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(REAP_POLL_INTERVAL),
                _ => return None,
            }
        }
        self.exited
    }

    /// 強制終了して回収する。[`KILL_REAP_GRACE`] の間に終わらなければ、回収を別のスレッドに
    /// 任せて `None` を返す（本体は待たない）。
    fn kill(&mut self) -> Option<ExitStatus> {
        if self.exited.is_some() {
            return self.exited;
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
        if let Some(status) = self.wait_for_exit(KILL_REAP_GRACE) {
            return Some(status);
        }
        if let Some(mut child) = self.child.take() {
            // スレッドを作れない場合は、ゾンビのまま残る（本体を止めないことを優先する）。
            let _ = thread::Builder::new()
                .name("genzo-worker-reaper".to_owned())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
        None
    }

    /// `grace` の間、自分で終わるのを待ち、終わらなければ強制終了する。
    fn reap(&mut self, grace: Duration) -> Option<ExitStatus> {
        match self.wait_for_exit(grace) {
            Some(status) => Some(status),
            None => self.kill(),
        }
    }

    /// 標準エラー出力の末尾（読み終えるのを少し待つ）。
    fn stderr_tail(&self) -> String {
        let _ = self.stderr_done.recv_timeout(STDERR_DRAIN_GRACE);
        let tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&tail).trim().to_owned()
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        drop(self.stdin.take());
        self.kill();
    }
}

/// 1 回のやり取りの結果。
enum Exchange {
    Response(Box<Response>),
    Timeout,
    Cancelled,
    Broken(String),
    Protocol(String),
}

/// 依頼を送り、応答を待つ。
fn exchange(
    proc: &mut WorkerProcess,
    request: &Request,
    timeout: Duration,
    cancel: Option<&Receiver<()>>,
) -> Exchange {
    let mut line = match serde_json::to_vec(request) {
        Ok(v) => v,
        Err(e) => return Exchange::Protocol(format!("依頼を JSON にできない: {e}")),
    };
    line.push(b'\n');
    let Some(stdin) = proc.stdin.as_mut() else {
        return Exchange::Broken("ワーカーの入力は閉じている".to_owned());
    };
    if let Err(e) = stdin.write_all(&line).and_then(|()| stdin.flush()) {
        return Exchange::Broken(format!("依頼を送れない: {e}"));
    }
    let never = crossbeam_channel::never();
    let cancel = cancel.unwrap_or(&never);
    // 応答・取り消し・タイムアウトを待ちながら、ワーカーのプロセスが終わっていないかも確かめる。
    // 出力が閉じたこと（EOF）だけで異常終了を判断すると、孫プロセス（ffmpeg など）が制御の
    // パイプを引き継いでいる場合（Windows ではワーカーの標準入出力のハンドルを引き継ぐ）に、
    // タイムアウトまで気づかない。
    let deadline = Instant::now().checked_add(timeout);
    let event = loop {
        let wait = match deadline {
            Some(d) => match d.checked_duration_since(Instant::now()) {
                Some(rest) if !rest.is_zero() => rest.min(EXIT_POLL_INTERVAL),
                _ => return Exchange::Timeout,
            },
            None => EXIT_POLL_INTERVAL,
        };
        crossbeam_channel::select! {
            recv(proc.events) -> ev => break ev,
            recv(cancel) -> _ => return Exchange::Cancelled,
            default(wait) => {
                if proc.has_exited() {
                    // 終わる前に書かれた応答があれば使う。
                    match proc.events.recv_timeout(EXIT_DRAIN_GRACE) {
                        Ok(ev) => break Ok(ev),
                        Err(_) => {
                            return Exchange::Broken(
                                "応答の前にワーカーのプロセスが終了した".to_owned(),
                            );
                        }
                    }
                }
            }
        }
    };
    match event {
        Ok(ReaderEvent::Line(bytes)) => match serde_json::from_slice::<Response>(&bytes) {
            Ok(r) if r.id == request.id => Exchange::Response(Box::new(r)),
            Ok(r) => Exchange::Protocol(format!(
                "応答の番号 {} が依頼の番号 {} と違う",
                r.id, request.id
            )),
            Err(e) => {
                let head: String = String::from_utf8_lossy(&bytes).chars().take(80).collect();
                Exchange::Protocol(format!("応答を解釈できない（{e}）: {head:?}"))
            }
        },
        Ok(ReaderEvent::TooLong) => {
            Exchange::Protocol(format!("応答の行が {MAX_LINE_BYTES} バイトを超える"))
        }
        Ok(ReaderEvent::Error(e)) => Exchange::Broken(format!("出力を読めない: {e}")),
        Ok(ReaderEvent::Eof) | Err(_) => Exchange::Broken("応答の前に出力が閉じた".to_owned()),
    }
}

/// 標準出力を 1 行ずつ読んで送る。
fn read_stdout(stdout: impl Read, tx: Sender<ReaderEvent>) {
    let mut reader = std::io::BufReader::new(stdout);
    loop {
        let event = match read_line_limited(&mut reader, MAX_LINE_BYTES) {
            Ok(LineRead::Line(l)) => ReaderEvent::Line(l),
            Ok(LineRead::Eof) => ReaderEvent::Eof,
            Ok(LineRead::TooLong) => ReaderEvent::TooLong,
            Err(e) => ReaderEvent::Error(e.to_string()),
        };
        let last = !matches!(event, ReaderEvent::Line(_));
        if tx.send(event).is_err() || last {
            // 長すぎる行の後も読まない（ワーカーは再起動される）。
            return;
        }
    }
}

/// 標準エラー出力を最後まで読み、末尾だけ残す。
fn read_stderr(mut stderr: impl Read, tail: Arc<Mutex<Vec<u8>>>, done: Sender<()>) {
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut t = tail
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                t.extend_from_slice(&buf[..n]);
                if t.len() > STDERR_TAIL_BYTES {
                    let cut = t.len() - STDERR_TAIL_BYTES;
                    t.drain(..cut);
                }
            }
        }
    }
    let _ = done.send(());
}

/// 1 つのワーカープロセスとのやり取りを管理する。
///
/// drop するとワーカーを強制終了する（[`WorkerClient::shutdown`] で穏やかに終わらせることもできる）。
pub struct WorkerClient {
    config: Arc<WorkerConfig>,
    executable: PathBuf,
    arena: Arc<ShmArena>,
    failures: Arc<FailureTracker>,
    process: Option<WorkerProcess>,
    next_id: u64,
    stats: WorkerStats,
    /// 種類ごとの、直近に必要だった共有メモリの大きさ（CFA, RGB, JPEG の順）。
    last_required: [u64; 3],
    label: String,
}

impl std::fmt::Debug for WorkerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerClient")
            .field("label", &self.label)
            .field("pid", &self.pid())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl WorkerClient {
    /// ワーカーを起動する（共有メモリの一時ディレクトリと失敗の記録は、このワーカー専用）。
    pub fn spawn(config: WorkerConfig) -> Result<Self, WorkerClientError> {
        config.validate()?;
        let arena = Arc::new(config.create_arena()?);
        let failures = Arc::new(FailureTracker::new(config.max_consecutive_failures));
        Self::spawn_shared(Arc::new(config), arena, failures, "worker".to_owned())
    }

    /// 共有メモリの一時ディレクトリと失敗の記録を共有して、ワーカーを起動する（[`crate::WorkerPool`]）。
    pub fn spawn_shared(
        config: Arc<WorkerConfig>,
        arena: Arc<ShmArena>,
        failures: Arc<FailureTracker>,
        label: String,
    ) -> Result<Self, WorkerClientError> {
        config.validate()?;
        let executable = locate_worker_executable(config.executable.as_deref())?;
        let mut client = Self {
            config,
            executable,
            arena,
            failures,
            process: None,
            next_id: 1,
            stats: WorkerStats::default(),
            last_required: [0; 3],
            label,
        };
        client.process = Some(client.start()?);
        Ok(client)
    }

    /// 名前（診断用）。
    pub fn label(&self) -> &str {
        &self.label
    }

    /// 設定。
    pub fn config(&self) -> &WorkerConfig {
        &self.config
    }

    /// 実行ファイル。
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// 起動中のワーカーのプロセス ID。
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().map(WorkerProcess::pid)
    }

    /// 起動中のワーカーの握手の応答（版・デコーダ・メモリの上限）。
    pub fn worker_info(&self) -> Option<&PongInfo> {
        self.process.as_ref().and_then(|p| p.pong.as_ref())
    }

    /// 統計。
    pub fn stats(&self) -> WorkerStats {
        self.stats
    }

    /// 失敗の記録。
    pub fn failures(&self) -> &Arc<FailureTracker> {
        &self.failures
    }

    /// 共有メモリの一時ディレクトリ。
    pub fn arena(&self) -> &Arc<ShmArena> {
        &self.arena
    }

    /// ワーカーを起動して握手する。
    fn start(&mut self) -> Result<WorkerProcess, WorkerClientError> {
        let spawn_err = |m: String| WorkerClientError::Spawn(m);
        let mut cmd = Command::new(&self.executable);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.args(&self.config.args);
        if let Some(limit) = self.config.memory_limit_bytes {
            cmd.arg("--memory-limit-bytes").arg(limit.to_string());
        }
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }
        // テスト用の口は設定でだけ有効にする（本体の環境や `env` に変数があっても渡さない）。
        if self.config.test_hooks {
            cmd.env(ENV_TEST_HOOKS, "1");
        } else {
            cmd.env_remove(ENV_TEST_HOOKS);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            /// CREATE_NO_WINDOW（Win32 のプロセス作成フラグ）。コンソールのウィンドウを開かない。
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| spawn_err(format!("{}: {e}", self.executable.display())))?;
        self.stats.spawns += 1;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(spawn_err("標準入出力のパイプを作れない".to_owned()));
        };
        let (tx, events) = crossbeam_channel::bounded(EVENT_CHANNEL_CAPACITY);
        let tail = Arc::new(Mutex::new(Vec::new()));
        let (done_tx, stderr_done) = crossbeam_channel::bounded(1);
        let mut proc = WorkerProcess {
            pid: child.id(),
            child: Some(child),
            stdin: Some(stdin),
            events,
            stderr_tail: Arc::clone(&tail),
            stderr_done,
            pong: None,
            exited: None,
        };
        // スレッドを作れなければ、proc の drop でワーカーを終了させる。
        thread::Builder::new()
            .name(format!("genzo-worker-{}-stdout", self.label))
            .spawn(move || read_stdout(stdout, tx))
            .map_err(|e| spawn_err(format!("読み取りのスレッドを作れない: {e}")))?;
        thread::Builder::new()
            .name(format!("genzo-worker-{}-stderr", self.label))
            .spawn(move || read_stderr(stderr, tail, done_tx))
            .map_err(|e| spawn_err(format!("読み取りのスレッドを作れない: {e}")))?;

        let request = Request {
            id: 0,
            job: Job::Ping,
        };
        let exchanged = exchange(&mut proc, &request, self.config.startup_timeout, None);
        match exchanged {
            Exchange::Response(r) if matches!(r.result, Ok(JobOutput::Pong(_))) => {
                let Ok(JobOutput::Pong(pong)) = r.result else {
                    unreachable!("直前で確かめた")
                };
                if pong.protocol_version != PROTOCOL_VERSION {
                    return Err(spawn_err(format!(
                        "ワーカーのプロトコルの版 {} が本体の {PROTOCOL_VERSION} と違う（{}）",
                        pong.protocol_version,
                        self.executable.display()
                    )));
                }
                proc.pong = Some(pong);
                Ok(proc)
            }
            Exchange::Response(r) => Err(spawn_err(format!("握手の応答が不正: {:?}", r.result))),
            Exchange::Timeout => Err(spawn_err(format!(
                "{:?} 以内に握手の応答がない",
                self.config.startup_timeout
            ))),
            Exchange::Cancelled => unreachable!("握手は取り消さない"),
            Exchange::Broken(d) | Exchange::Protocol(d) => {
                let status = proc.reap(CRASH_REAP_GRACE);
                Err(spawn_err(format!(
                    "握手に失敗: {d}（終了状態 {status:?}、標準エラー出力: {}）",
                    proc.stderr_tail()
                )))
            }
        }
    }

    /// 起動していなければ起動する。
    fn ensure_running(&mut self) -> Result<(), WorkerClientError> {
        if self.process.is_none() {
            self.process = Some(self.start()?);
        }
        Ok(())
    }

    /// 今のワーカーを強制終了して、起動し直す。起動に失敗した場合は、次の依頼のときに試みる。
    fn recycle(&mut self) {
        if let Some(mut p) = self.process.take() {
            p.kill();
        }
        self.stats.restarts += 1;
        self.process = self.start().ok();
    }

    /// ワーカーを再起動する（強制終了してから起動する）。
    pub fn restart(&mut self) -> Result<(), WorkerClientError> {
        if let Some(mut p) = self.process.take() {
            p.kill();
        }
        self.stats.restarts += 1;
        self.process = Some(self.start()?);
        Ok(())
    }

    /// 1 件の依頼を送り、応答を受け取る。異常時はワーカーを再起動してエラーを返す。
    fn call(
        &mut self,
        job: Job,
        cancel: &CancellationToken,
    ) -> Result<Result<JobOutput, WorkerError>, WorkerClientError> {
        if cancel.is_cancelled() {
            return Err(WorkerClientError::Cancelled);
        }
        self.ensure_running()?;
        let kind = job.kind();
        let timeout = self.config.timeouts.for_kind(kind);
        let id = self.next_id;
        self.next_id += 1;
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(1);
        let _registration = cancel.on_cancel(move || {
            let _ = cancel_tx.try_send(());
        });
        let proc = self
            .process
            .as_mut()
            .expect("ensure_running の後は起動している");
        match exchange(proc, &Request { id, job }, timeout, Some(&cancel_rx)) {
            Exchange::Response(r) => {
                self.stats.responses += 1;
                Ok(r.result)
            }
            Exchange::Timeout => {
                self.stats.timeouts += 1;
                self.recycle();
                Err(WorkerClientError::Timeout { kind, timeout })
            }
            Exchange::Cancelled => {
                self.stats.cancellations += 1;
                self.recycle();
                Err(WorkerClientError::Cancelled)
            }
            Exchange::Broken(detail) => {
                self.stats.crashes += 1;
                let mut p = self.process.take().expect("起動している");
                let status = p.reap(CRASH_REAP_GRACE);
                let stderr_tail = p.stderr_tail();
                drop(p);
                self.recycle();
                let status = status.map_or_else(|| "不明".to_owned(), |s| s.to_string());
                Err(WorkerClientError::Crashed {
                    detail: format!("{detail}。終了状態: {status}"),
                    stderr_tail,
                })
            }
            Exchange::Protocol(msg) => self.protocol_error(msg),
        }
    }

    fn protocol_error<T>(&mut self, msg: String) -> Result<T, WorkerClientError> {
        self.stats.protocol_errors += 1;
        self.recycle();
        Err(WorkerClientError::Protocol(msg))
    }

    /// 予期しない種類の応答。
    fn unexpected<T>(&mut self, out: &JobOutput) -> Result<T, WorkerClientError> {
        let name = serde_json::to_value(out)
            .ok()
            .and_then(|v| v.get("type")?.as_str().map(str::to_owned))
            .unwrap_or_default();
        self.protocol_error(format!("予期しない種類の応答（{name}）"))
    }

    /// ファイルを対象とする種類 `kind` のジョブを、スキップの確認と失敗の記録をしながら実行する
    /// （[`crate::failures`]）。
    fn file_job<T>(
        &mut self,
        path: &Path,
        kind: JobKind,
        f: impl FnOnce(&mut Self) -> Result<T, WorkerClientError>,
    ) -> Result<T, WorkerClientError> {
        let failures = self.failures.consecutive_failures(path);
        if failures >= self.failures.threshold() {
            return Err(WorkerClientError::Skipped {
                path: path.to_path_buf(),
                failures,
            });
        }
        let result = f(self);
        match &result {
            Ok(_) => self.failures.record_success(path, kind),
            Err(e) if e.counts_toward_skip() => {
                self.failures.record_failure(path, kind);
            }
            Err(_) => {}
        }
        result
    }

    fn kind_index(kind: PayloadKind) -> usize {
        match kind {
            PayloadKind::CfaU16 => 0,
            PayloadKind::RgbF32 => 1,
            PayloadKind::Jpeg => 2,
        }
    }

    fn initial_capacity(&self, kind: PayloadKind) -> u64 {
        let initial = self.config.initial_shm;
        let base = match kind {
            PayloadKind::CfaU16 => initial.raw,
            PayloadKind::RgbF32 => initial.image,
            PayloadKind::Jpeg => initial.thumbnail,
        };
        base.max(self.last_required[Self::kind_index(kind)])
    }

    /// 種類ごとの共有メモリの大きさの上限（ヘッダを含む）。
    pub fn max_shm_bytes(kind: PayloadKind) -> u64 {
        let data = match kind {
            PayloadKind::CfaU16 => MAX_PIXELS * 2,
            PayloadKind::RgbF32 => MAX_IMAGE_PIXELS * 12,
            PayloadKind::Jpeg => MAX_THUMBNAIL_JPEG_BYTES,
        };
        SHM_HEADER_LEN as u64 + data
    }

    fn allocate(&self, capacity: u64) -> Result<ShmBuffer, WorkerClientError> {
        self.arena
            .allocate(capacity)
            .map_err(|e| WorkerClientError::Shm(e.to_string()))
    }

    /// 共有メモリを使うジョブ。足りなければ 1 回だけ確保し直して送り直す。
    fn shm_job(
        &mut self,
        kind: PayloadKind,
        cancel: &CancellationToken,
        make: impl Fn(ShmRef) -> Job,
    ) -> Result<(JobOutput, ShmBuffer), WorkerClientError> {
        let mut capacity = self.initial_capacity(kind);
        for attempt in 0..2 {
            let buffer = self.allocate(capacity)?;
            let job = make(buffer.shm_ref(self.config.verify_checksum));
            match self.call(job, cancel)? {
                Ok(out) => return Ok((out, buffer)),
                Err(WorkerError {
                    kind: WorkerErrorKind::ShmTooSmall { required_bytes },
                    ..
                }) if attempt == 0 => {
                    if required_bytes <= capacity || required_bytes > Self::max_shm_bytes(kind) {
                        return self.protocol_error(format!(
                            "ワーカーが求めた共有メモリの大きさ {required_bytes} が不正（確保したのは {capacity}）"
                        ));
                    }
                    self.last_required[Self::kind_index(kind)] = required_bytes;
                    capacity = required_bytes;
                }
                Err(WorkerError {
                    kind: WorkerErrorKind::ShmTooSmall { .. },
                    ..
                }) => {
                    return self.protocol_error(
                        "求められた大きさで確保し直しても、共有メモリが足りないと言われた"
                            .to_owned(),
                    );
                }
                Err(e) => return Err(WorkerClientError::Job(e)),
            }
        }
        unreachable!("2 回目は必ず return する")
    }

    /// 検証に失敗したバッファ: 破棄し、ワーカーを再起動する。
    fn invalid_buffer<T>(&mut self, e: BufferError) -> Result<T, WorkerClientError> {
        self.stats.invalid_buffers += 1;
        self.recycle();
        Err(WorkerClientError::InvalidBuffer(e))
    }

    fn finish_raw(
        &mut self,
        buffer: &ShmBuffer,
        info: RawFrameInfo,
    ) -> Result<RawFrame, WorkerClientError> {
        let decoder_id = info.decoder_id.clone();
        let image = buffer
            .open_payload(PayloadKind::CfaU16, self.config.verify_checksum)
            .and_then(|p| validate::raw_image(&p, info));
        match image {
            Ok(image) => Ok(RawFrame { image, decoder_id }),
            Err(e) => self.invalid_buffer(e),
        }
    }

    /// 生存の確認（往復の確認）。
    pub fn ping(&mut self) -> Result<PongInfo, WorkerClientError> {
        match self.call(Job::Ping, &CancellationToken::new())? {
            Ok(JobOutput::Pong(p)) => Ok(p),
            Ok(other) => self.unexpected(&other),
            Err(e) => Err(WorkerClientError::Job(e)),
        }
    }

    /// 写真のメタデータを読む（RAW は LibRaw、JPEG / TIFF / PNG は Exif）。
    pub fn probe_photo(
        &mut self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<PhotoProbe, WorkerClientError> {
        self.file_job(path, JobKind::ProbePhoto, |c| {
            let job = Job::ProbePhoto {
                path: path.to_path_buf(),
            };
            match c.call(job, cancel)? {
                Ok(JobOutput::Photo(mut p)) => {
                    validate::sanitize_metadata(&mut p.metadata);
                    Ok(p)
                }
                Ok(other) => c.unexpected(&other),
                Err(e) => Err(WorkerClientError::Job(e)),
            }
        })
    }

    /// 動画のメタデータを読む（ffprobe）。
    pub fn probe_video(
        &mut self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<VideoProbe, WorkerClientError> {
        self.file_job(path, JobKind::ProbeVideo, |c| {
            let job = Job::ProbeVideo {
                path: path.to_path_buf(),
            };
            match c.call(job, cancel)? {
                Ok(JobOutput::Video(v)) => Ok(v),
                Ok(other) => c.unexpected(&other),
                Err(e) => Err(WorkerClientError::Job(e)),
            }
        })
    }

    /// RAW を展開する（段階 A0）。CFA は検証済みの [`RawImage`] で返す。
    pub fn decode_raw(
        &mut self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<RawFrame, WorkerClientError> {
        self.file_job(path, JobKind::DecodeRaw, |c| {
            let (out, buffer) = c.shm_job(PayloadKind::CfaU16, cancel, |shm| Job::DecodeRaw {
                path: path.to_path_buf(),
                shm,
            })?;
            match out {
                JobOutput::Raw(info) => c.finish_raw(&buffer, info),
                other => c.unexpected(&other),
            }
        })
    }

    /// JPEG / TIFF / PNG をデコードし、リニア BT.2020（B2）で返す（向きは反映しない）。
    pub fn decode_image(
        &mut self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<LinearImage, WorkerClientError> {
        self.file_job(path, JobKind::DecodeImage, |c| {
            let (out, buffer) = c.shm_job(PayloadKind::RgbF32, cancel, |shm| Job::DecodeImage {
                path: path.to_path_buf(),
                shm,
            })?;
            let JobOutput::Image(mut info) = out else {
                return c.unexpected(&out);
            };
            let pixels = buffer
                .open_payload(PayloadKind::RgbF32, c.config.verify_checksum)
                .and_then(|p| validate::linear_pixels(&p, &mut info));
            match pixels {
                Ok((pixels, non_finite_replaced)) => Ok(LinearImage {
                    pixels,
                    info,
                    non_finite_replaced,
                }),
                Err(e) => c.invalid_buffer(e),
            }
        })
    }

    fn check_spec(spec: CacheSpec) -> Result<(), WorkerClientError> {
        if !(1..=MAX_THUMBNAIL_EDGE).contains(&spec.long_edge) || !(1..=100).contains(&spec.quality)
        {
            return Err(WorkerClientError::InvalidArgument(format!(
                "サムネイルの長辺は 1〜{MAX_THUMBNAIL_EDGE}、品質は 1〜100（{spec:?}）"
            )));
        }
        Ok(())
    }

    fn thumbnail_job(
        &mut self,
        video: bool,
        path: &Path,
        spec: CacheSpec,
        cancel: &CancellationToken,
    ) -> Result<Thumbnail, WorkerClientError> {
        Self::check_spec(spec)?;
        let kind = if video {
            JobKind::VideoThumbnail
        } else {
            JobKind::Thumbnail
        };
        self.file_job(path, kind, |c| {
            let (out, buffer) = c.shm_job(PayloadKind::Jpeg, cancel, |shm| {
                let path = path.to_path_buf();
                if video {
                    Job::VideoThumbnail {
                        path,
                        max_edge: spec.long_edge,
                        quality: Some(spec.quality),
                        shm,
                    }
                } else {
                    Job::Thumbnail {
                        path,
                        max_edge: spec.long_edge,
                        quality: Some(spec.quality),
                        shm,
                    }
                }
            })?;
            let info: ThumbnailInfo = match out {
                JobOutput::Thumbnail(info)
                    if video == (info.source == ThumbnailSource::VideoFrame)
                        && video == info.video.is_some() =>
                {
                    info
                }
                other => return c.unexpected(&other),
            };
            let jpeg = buffer
                .open_payload(PayloadKind::Jpeg, c.config.verify_checksum)
                .and_then(|p| validate::thumbnail_jpeg(&p, &info, spec.long_edge));
            match jpeg {
                Ok(jpeg) => Ok(Thumbnail {
                    jpeg,
                    width: info.width,
                    height: info.height,
                    source: info.source,
                    video: info.video,
                }),
                Err(e) => c.invalid_buffer(e),
            }
        })
    }

    /// 写真のサムネイル（B5 の JPEG。RAW は埋め込みの JPEG から）を作る（PRV-01）。
    pub fn thumbnail(
        &mut self,
        path: &Path,
        spec: CacheSpec,
        cancel: &CancellationToken,
    ) -> Result<Thumbnail, WorkerClientError> {
        self.thumbnail_job(false, path, spec, cancel)
    }

    /// 動画のサムネイル（B5 の JPEG）を作る（VID-02）。
    pub fn video_thumbnail(
        &mut self,
        path: &Path,
        spec: CacheSpec,
        cancel: &CancellationToken,
    ) -> Result<Thumbnail, WorkerClientError> {
        self.thumbnail_job(true, path, spec, cancel)
    }

    /// テスト用の口を実行する（ワーカーを `test_hooks` 付きで起動した場合だけ動く）。
    ///
    /// `track_path` を渡すと、そのファイルのジョブとして失敗を記録する（スキップの確認用）。
    pub fn run_test_hook(
        &mut self,
        hook: TestHook,
        track_path: Option<&Path>,
        cancel: &CancellationToken,
    ) -> Result<TestHookOutput, WorkerClientError> {
        let run = |c: &mut Self| -> Result<TestHookOutput, WorkerClientError> {
            let checksum = c.config.verify_checksum;
            let mut buffer = None;
            let job = match hook {
                TestHook::Crash { with_shm } | TestHook::Hang { with_shm } => {
                    if with_shm {
                        buffer = Some(c.allocate(TEST_HOOK_SHM_BYTES)?);
                    }
                    let shm = buffer.as_ref().map(|b| b.shm_ref(checksum));
                    if matches!(hook, TestHook::Crash { .. }) {
                        Job::TestCrash {
                            shm,
                            leave_child: false,
                        }
                    } else {
                        Job::TestHang { shm }
                    }
                }
                TestHook::CrashLeavingChild => {
                    let b = c.allocate(TEST_HOOK_SHM_BYTES)?;
                    let shm = b.shm_ref(checksum);
                    buffer = Some(b);
                    Job::TestCrash {
                        shm: Some(shm),
                        leave_child: true,
                    }
                }
                TestHook::Garbage(mode) => Job::TestGarbage { mode },
                TestHook::Oversize(mode) => {
                    let b = c.allocate(TEST_HOOK_SHM_BYTES)?;
                    let shm = b.shm_ref(checksum);
                    buffer = Some(b);
                    Job::TestOversize { mode, shm }
                }
                TestHook::Allocate(bytes) => Job::TestAllocate { bytes },
            };
            match c.call(job, cancel)? {
                Ok(JobOutput::Raw(info)) if matches!(hook, TestHook::Oversize(_)) => {
                    let buffer = buffer.expect("Oversize は共有メモリを渡す");
                    c.finish_raw(&buffer, info)
                        .map(|r| TestHookOutput::Raw(Box::new(r)))
                }
                Ok(JobOutput::Allocated { bytes }) => Ok(TestHookOutput::Allocated(bytes)),
                Ok(other) => c.unexpected(&other),
                Err(e) => Err(WorkerClientError::Job(e)),
            }
        };
        match track_path {
            Some(p) => self.file_job(p, JobKind::TestHook, run),
            None => run(self),
        }
    }

    /// ワーカーに終了を依頼して、終了を待つ（時間内に終わらなければ強制終了する）。
    pub fn shutdown(mut self) -> Result<(), WorkerClientError> {
        let Some(mut proc) = self.process.take() else {
            return Ok(());
        };
        let timeout = self.config.timeouts.shutdown;
        let request = Request {
            id: self.next_id,
            job: Job::Shutdown,
        };
        let result = match exchange(&mut proc, &request, timeout, None) {
            Exchange::Response(r) if r.result == Ok(JobOutput::ShuttingDown) => Ok(()),
            Exchange::Response(r) => Err(WorkerClientError::Protocol(format!(
                "終了の応答が不正: {:?}",
                r.result
            ))),
            Exchange::Timeout => Err(WorkerClientError::Timeout {
                kind: JobKind::Shutdown,
                timeout,
            }),
            Exchange::Cancelled => Err(WorkerClientError::Cancelled),
            Exchange::Broken(d) => Err(WorkerClientError::Crashed {
                detail: d,
                stderr_tail: String::new(),
            }),
            Exchange::Protocol(m) => Err(WorkerClientError::Protocol(m)),
        };
        drop(proc.stdin.take());
        proc.reap(timeout);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_by_kind() {
        let t = JobTimeouts::default();
        assert_eq!(t.for_kind(JobKind::DecodeRaw), DEFAULT_DECODE_RAW_TIMEOUT);
        assert_eq!(
            t.for_kind(JobKind::VideoThumbnail),
            DEFAULT_VIDEO_THUMBNAIL_TIMEOUT
        );
        let u = JobTimeouts::uniform(Duration::from_millis(7));
        for kind in [
            JobKind::Ping,
            JobKind::Shutdown,
            JobKind::ProbePhoto,
            JobKind::ProbeVideo,
            JobKind::DecodeRaw,
            JobKind::DecodeImage,
            JobKind::Thumbnail,
            JobKind::VideoThumbnail,
            JobKind::TestHook,
        ] {
            assert_eq!(u.for_kind(kind), Duration::from_millis(7));
        }
    }

    #[test]
    fn config_validation() {
        WorkerConfig::default().validate().unwrap();
        let mut c = WorkerConfig::default();
        c.timeouts.decode_raw = Duration::ZERO;
        assert!(matches!(
            c.validate(),
            Err(WorkerClientError::InvalidArgument(_))
        ));
        let c = WorkerConfig {
            memory_limit_bytes: Some(1024),
            ..Default::default()
        };
        assert!(c.validate().is_err());
        let c = WorkerConfig {
            max_consecutive_failures: 0,
            ..Default::default()
        };
        assert!(c.validate().is_err());
        let c = WorkerConfig {
            startup_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(c.validate().is_err());
        let mut c = WorkerConfig::default();
        c.initial_shm.thumbnail = SHM_HEADER_LEN as u64 - 1;
        assert!(c.validate().is_err());
        c.initial_shm.thumbnail = SHM_HEADER_LEN as u64;
        c.validate().unwrap();
    }

    #[test]
    fn executable_lookup_order() {
        let dir = tempfile::tempdir().unwrap();
        let name = format!("{WORKER_EXE_NAME}{}", std::env::consts::EXE_SUFFIX);
        // 明示 > 環境変数 > 同じフォルダ > deps の上。
        let explicit = dir.path().join("x");
        assert_eq!(
            locate_with(Some(&explicit), Some("env".into()), None).unwrap(),
            explicit
        );
        assert_eq!(
            locate_with(None, Some("env-worker".into()), None).unwrap(),
            PathBuf::from("env-worker")
        );
        // 空の環境変数は使わない。
        let deps = dir.path().join("deps");
        std::fs::create_dir(&deps).unwrap();
        let host = deps.join("host");
        assert!(matches!(
            locate_with(None, Some(OsString::new()), Some(host.clone())),
            Err(WorkerClientError::Spawn(_))
        ));
        std::fs::write(dir.path().join(&name), b"").unwrap();
        assert_eq!(
            locate_with(None, None, Some(host.clone())).unwrap(),
            dir.path().join(&name)
        );
        std::fs::write(deps.join(&name), b"").unwrap();
        assert_eq!(
            locate_with(None, None, Some(host)).unwrap(),
            deps.join(&name)
        );
        assert!(locate_with(None, None, None).is_err());
    }

    #[test]
    fn skip_counting_categories() {
        assert!(
            WorkerClientError::Timeout {
                kind: JobKind::DecodeRaw,
                timeout: Duration::from_secs(1)
            }
            .counts_toward_skip()
        );
        assert!(
            WorkerClientError::Crashed {
                detail: String::new(),
                stderr_tail: String::new()
            }
            .counts_toward_skip()
        );
        assert!(WorkerClientError::Protocol(String::new()).counts_toward_skip());
        assert!(WorkerClientError::InvalidBuffer(BufferError::BadMagic).counts_toward_skip());
        assert!(!WorkerClientError::Cancelled.counts_toward_skip());
        let job = WorkerClientError::Job(WorkerError::new(WorkerErrorKind::Decode, "x"));
        assert!(!job.counts_toward_skip());
        assert_eq!(job.worker_error_kind(), Some(&WorkerErrorKind::Decode));
        assert!(!WorkerClientError::Spawn(String::new()).counts_toward_skip());
    }

    #[test]
    fn shm_limits_cover_the_largest_inputs() {
        assert_eq!(
            WorkerClient::max_shm_bytes(PayloadKind::CfaU16),
            64 + 400_000_000
        );
        assert_eq!(
            WorkerClient::max_shm_bytes(PayloadKind::RgbF32),
            64 + 2_400_000_000
        );
        const { assert!(INITIAL_RAW_SHM_BYTES >= 33_000_000 * 2 + 64) };
    }
}
