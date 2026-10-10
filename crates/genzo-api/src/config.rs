//! コアの設定（[`CoreConfig`]）と、仮置きの値。
//!
//! [`CoreConfig`] は [`crate::Core::open`] に渡す、起動ごとの設定（ファイルの場所・ワーカーの起動方法・
//! GPU の使用）と、カタログに設定がまだないときの初期値（既定のタイムゾーン・プレビューキャッシュの
//! 上限）を持つ。利用者が設定画面で変えた値（SYS-05）はカタログの設定テーブルに保存し、そちらを優先する
//! （[`crate::CoreSettings`]）。

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use genzo_worker::JobTimeouts;
use serde::{Deserialize, Serialize};

/// 取り込みで、1 つのトランザクションにまとめて登録するファイルの数。
///
/// **仮置き**: 64。PERF-08（1,000 枚の登録を 30 秒以内、途中からサムネイルを順次表示）のため、
/// 1 回の書き込みの待ち（`synchronous = FULL`）を少なくしつつ、グリッドへの反映が遅れすぎない数として
/// 置いた。PoC-6 で書き込みの時間を計測して決める。
pub const IMPORT_BATCH_FILES: usize = 64;

/// 現像設定の自動保存の待ち時間（DATA-03: ドラッグでない変更は最後の操作から 1 秒後に保存する）。
pub const DEVELOP_SAVE_DELAY: Duration = Duration::from_secs(1);

/// 現像設定の自動保存の最大の待ち時間: 最初の未保存の変更からこの時間がたったら、ドラッグでない変更が
/// 続いていても保存する（DATA-03a: 失ってよいのは最後の操作から 1 秒以内の操作。変更が 1 秒未満の間隔で
/// 続くと、[`DEVELOP_SAVE_DELAY`] だけでは保存がいつまでも延びるため）。
///
/// **仮置き**: 1 秒（DATA-03a の文面に合わせた値）。長く変え続けると履歴が 1 秒ごとに分かれる。履歴の
/// まとめ方（DEV-27）との釣り合いで 2〜3 秒にするかは人が決める（implementation_status の表）。
/// 実際の待ち時間は、[`DEVELOP_SAVE_DELAY`]（または設定した待ち時間）より短くはしない。
pub const DEVELOP_SAVE_MAX_DELAY: Duration = Duration::from_secs(1);

/// 自動バックアップの間隔（DATA-04「既定では 1 日 1 回」）。
pub const AUTO_BACKUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 書き出し・プレビューの再生成などの P3 のジョブを実行するスレッドの数の既定値。
///
/// **仮置き**: 2。取り込みや書き出しのような長いジョブが 1 本走っていても、設定の変更に伴う
/// サムネイルの作り直しが進むようにした。各ジョブの中の現像処理は rayon で並列化される。PoC-3 /
/// PERF-13 の計測で見直す。
pub const DEFAULT_BACKGROUND_THREADS: usize = 2;

/// GPU の側の失敗で CPU 版に切り替えた回数がこの数に達したら、そのセッションでは GPU を使わない
/// （6.3 節「GPU を初期化し直す。失敗が続く場合は CPU 版に切り替え、その旨を表示する」）。
///
/// **仮置き**: 2（1 回目は初期化し直して試す）。PoC-3 で実機の失敗の起き方を見て決める。
pub const MAX_GPU_FAILURES: u32 = 2;

/// 取り込みで、名前が `.` で始まるファイル・フォルダを飛ばすか。
///
/// **仮置き**: 飛ばす。macOS が別のファイルシステムに作る `._IMG_0001.JPG`（AppleDouble）や、
/// 書き出しの一時ファイル（genzo-media の `.genzo-export-*.tmp`）を、写真として登録しないため。
pub const SKIP_HIDDEN_FILES: bool = true;

/// ワーカーの起動方法（04 の 1.2 節）。
///
/// JSON では `{"kind": "executable", "path": null}`、
/// `{"kind": "self_subcommand", "executable": null, "subcommand": "__worker"}` のように表す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerLaunch {
    /// 別の実行ファイル（`genzo-worker`）を起動する。`path` が `None` なら
    /// [`genzo_worker::locate_worker_executable`] で探す（環境変数 `GENZO_WORKER_PATH`、本体と同じフォルダ）。
    Executable {
        /// ワーカーの実行ファイル。
        path: Option<PathBuf>,
    },
    /// 本体の実行ファイル自身を、隠しサブコマンド付きで起動する（`genzo __worker`）。CLI や将来の
    /// アプリで、ワーカーの実行ファイルを別に配布しなくて済む。本体の `main` は、最初の引数が
    /// `subcommand` なら [`genzo_worker::run_worker`] を呼ぶこと。
    SelfSubcommand {
        /// 実行ファイル。`None` なら本体の実行ファイル（`std::env::current_exe`）。
        executable: Option<PathBuf>,
        /// 隠しサブコマンド（既定は [`genzo_worker::WORKER_SUBCOMMAND`]）。
        subcommand: String,
    },
}

impl Default for WorkerLaunch {
    fn default() -> Self {
        Self::Executable { path: None }
    }
}

impl WorkerLaunch {
    /// 本体の実行ファイル自身を `__worker` 付きで起動する設定。
    pub fn current_exe() -> Self {
        Self::SelfSubcommand {
            executable: None,
            subcommand: genzo_worker::WORKER_SUBCOMMAND.to_owned(),
        }
    }
}

/// GPU の使用（2.4 節・6.3 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuMode {
    /// 使えれば使う（環境変数 `GENZO_GPU=0` などでも止められる。genzo-gpu の `GpuContextOptions::from_env`）。
    #[default]
    Auto,
    /// 使わない（CPU 版だけで処理する）。
    Off,
}

fn default_backup_generations() -> usize {
    genzo_catalog::DEFAULT_BACKUP_GENERATIONS
}

fn default_true() -> bool {
    true
}

/// [`crate::Core::open`] の設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoreConfig {
    /// カタログ（catalog.db）のパス。なければ作る。
    pub catalog_path: PathBuf,
    /// データのフォルダ（thumbs.db、previews/、既定のバックアップの置き場 backups/）。なければ作る。
    pub data_dir: PathBuf,
    /// ワーカーの起動方法。
    #[serde(default)]
    pub worker: WorkerLaunch,
    /// 既定のタイムゾーンのオフセット（分。UTC より東が正）の初期値。カタログに設定がなければ使い、
    /// 最初に開いたときにカタログに保存する（以後はカタログの値を使う。
    /// [`crate::CoreSettings::default_utc_offset_minutes`]）。`None` なら最初に開いた時点の OS の
    /// オフセット（**仮置き**。夏時間のある地域の扱いは genzo-model の `CaptureTime` の doc）で、保存した
    /// 値と開いた時点の OS のオフセットが違えば警告する（自動では変えない）。
    #[serde(default)]
    pub default_utc_offset_minutes: Option<i32>,
    /// L1 プレビューのキャッシュの上限（バイト）の初期値。カタログに設定がなければ使う。`None` なら
    /// genzo-catalog の [`genzo_catalog::DEFAULT_PREVIEW_CAPACITY_BYTES`]（20GB。SCL-04）。
    #[serde(default)]
    pub preview_cache_bytes: Option<u64>,
    /// GPU の使用。
    #[serde(default)]
    pub gpu: GpuMode,
    /// 自動バックアップの置き場。`None` なら `data_dir/backups`。
    #[serde(default)]
    pub backup_dir: Option<PathBuf>,
    /// 自動バックアップの世代数（DATA-04。既定 5。0 なら自動バックアップをしない）。
    #[serde(default = "default_backup_generations")]
    pub backup_generations: usize,
    /// 終了時に、前回のバックアップからの経過時間によらずバックアップを作るか（DATA-04「またはアプリの
    /// 終了時」。既定は `false` で、終了時も 1 日以上たっていれば作る）。
    #[serde(default)]
    pub backup_on_close: bool,
    /// 取り込みの後に、現像結果からサムネイル（L0）と標準プレビュー（L1）を作り直すか（4 章「最初の
    /// 表示」: 取り込み直後は埋め込み JPEG を使い、バックグラウンドで現像結果に置き換える）。
    #[serde(default = "default_true")]
    pub render_previews_after_import: bool,
    /// バッチ用のワーカーの数（1〜2）。`None` なら genzo-worker の既定値。
    #[serde(default)]
    pub batch_workers: Option<usize>,
    /// P3 のジョブのスレッドの数。`None` なら [`DEFAULT_BACKGROUND_THREADS`]。
    #[serde(default)]
    pub background_threads: Option<usize>,
    /// ワーカーのテスト用の口を有効にする（テスト専用。[`crate::Core::run_worker_test_hook`]）。
    #[serde(default)]
    pub worker_test_hooks: bool,
    /// ワーカーの共有メモリの一時ディレクトリを置く場所（`None` なら OS の一時ディレクトリ）。
    #[serde(default)]
    pub worker_shm_dir: Option<PathBuf>,
    /// ワーカーのジョブのタイムアウト（`None` なら genzo-worker の既定値。テスト用）。
    #[serde(skip)]
    pub worker_timeouts: Option<JobTimeouts>,
    /// ワーカーの起動と握手のタイムアウト（`None` なら genzo-worker の既定値。テスト用）。
    #[serde(skip)]
    pub worker_startup_timeout: Option<Duration>,
    /// ワーカーに追加で渡す環境変数（`GENZO_FFMPEG` など）。
    #[serde(skip)]
    pub worker_env: Vec<(OsString, OsString)>,
    /// 現像設定の自動保存の待ち時間（`None` なら [`DEVELOP_SAVE_DELAY`]。テスト用: 長くすると、
    /// 時間によらずに「まとめて 1 件」を確かめられる）。
    #[serde(skip)]
    pub develop_save_delay: Option<Duration>,
    /// 現像設定の自動保存の最大の待ち時間（`None` なら [`DEVELOP_SAVE_MAX_DELAY`]。待ち時間より短くは
    /// しない。テスト用）。
    #[serde(skip)]
    pub develop_save_max_delay: Option<Duration>,
}

impl CoreConfig {
    /// カタログとデータのフォルダを指定した、既定の設定。
    pub fn new(catalog_path: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            catalog_path: catalog_path.into(),
            data_dir: data_dir.into(),
            worker: WorkerLaunch::default(),
            default_utc_offset_minutes: None,
            preview_cache_bytes: None,
            gpu: GpuMode::Auto,
            backup_dir: None,
            backup_generations: default_backup_generations(),
            backup_on_close: false,
            render_previews_after_import: true,
            batch_workers: None,
            background_threads: None,
            worker_test_hooks: false,
            worker_shm_dir: None,
            worker_timeouts: None,
            worker_startup_timeout: None,
            worker_env: Vec::new(),
            develop_save_delay: None,
            develop_save_max_delay: None,
        }
    }

    /// 自動バックアップの置き場。
    pub fn backup_dir(&self) -> PathBuf {
        self.backup_dir
            .clone()
            .unwrap_or_else(|| self.data_dir.join("backups"))
    }

    /// thumbs.db のパス。
    pub fn thumbs_path(&self) -> PathBuf {
        self.data_dir.join("thumbs.db")
    }

    /// 既定の L1 プレビューのフォルダ（`data_dir/previews`）。
    pub fn default_preview_dir(&self) -> PathBuf {
        self.data_dir.join("previews")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_and_defaults() {
        let c = CoreConfig::new("/c/catalog.db", "/c/data");
        let json = serde_json::to_string(&c).unwrap();
        let back: CoreConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        // 必須はカタログとデータのフォルダだけ。
        let min: CoreConfig =
            serde_json::from_str(r#"{"catalog_path":"/a.db","data_dir":"/d"}"#).unwrap();
        assert_eq!(min.worker, WorkerLaunch::Executable { path: None });
        assert_eq!(min.gpu, GpuMode::Auto);
        assert_eq!(min.backup_generations, 5);
        assert!(min.render_previews_after_import);
        assert_eq!(min.backup_dir(), PathBuf::from("/d").join("backups"));
        let w: WorkerLaunch = serde_json::from_str(
            r#"{"kind":"self_subcommand","executable":null,"subcommand":"__worker"}"#,
        )
        .unwrap();
        assert_eq!(w, WorkerLaunch::current_exe());
    }
}
