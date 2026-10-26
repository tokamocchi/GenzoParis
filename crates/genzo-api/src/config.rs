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
/// **配布するアプリは [`WorkerLaunch::Executable`]（既定）を使う**: ワーカーを別の実行ファイル
/// （`genzo-worker`）にし、本体の実行ファイルは LibRaw に依存しない（LibRaw を読み込むのはワーカーの
/// プロセスだけにする。LGPL-2.1 の条件を満たしやすくするため。決定 No.4。ライセンスの扱いは人・専門家の
/// 確認待ち）。[`WorkerLaunch::SelfSubcommand`] は、実行ファイルを 1 つにしたい開発用の CLI（`genzo`）の
/// ためのもの（この場合、機能フラグ `libraw` を有効にした本体の実行ファイルは LibRaw に依存する）。
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
    /// 本体の実行ファイル自身を、隠しサブコマンド付きで起動する（`genzo __worker`）。開発用の CLI で、
    /// ワーカーの実行ファイルを別に用意しなくて済む（配布するアプリでは使わない。決定 No.4）。本体の
    /// `main` は、最初の引数が `subcommand` なら [`genzo_worker::run_worker`] を呼ぶこと。
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
    /// ソフトウェアの GPU は、[`CoreConfig::gpu_allow_software`] で許可したときだけ使う。
    #[default]
    Auto,
    /// 使わない（CPU 版だけで処理する）。
    Off,
}

fn default_true() -> bool {
    true
}

fn default_backup_generations() -> usize {
    genzo_catalog::DEFAULT_BACKUP_GENERATIONS
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
    /// ソフトウェアの GPU（Mesa の llvmpipe・Windows の WARP など。CPU で動く実装）も使うか（既定は
    /// `false`: 使わずに CPU 版で処理する。決定 No.20）。`false` でも、環境変数
    /// `GENZO_GPU_ALLOW_SOFTWARE=1` なら使う（開発・テスト用）。
    #[serde(default)]
    pub gpu_allow_software: bool,
    /// 自動バックアップの置き場。`None` なら `data_dir/backups`。
    #[serde(default)]
    pub backup_dir: Option<PathBuf>,
    /// 現像プリセット・書き出しプリセット（DEV-29・EXP-02）のフォルダ。カタログをまたいで使うので、カタログの
    /// データのフォルダとは別（`None` なら [`crate::default_presets_dir`]: 環境変数 `GENZO_PRESETS_DIR`、なければ
    /// OS のアプリのデータのフォルダの `presets`。アプリと CLI で同じ場所）。
    #[serde(default)]
    pub presets_dir: Option<PathBuf>,
    /// AI マスク（DEV-20）のモデルのフォルダ（モデルのファイルと台帳 `*.genzo-model.json`）。`None` なら
    /// [`crate::default_models_dir`]（環境変数 `GENZO_MODELS_DIR`、なければ OS のアプリのデータのフォルダの `models`。
    /// アプリと CLI で同じ場所）。
    #[serde(default)]
    pub models_dir: Option<PathBuf>,
    /// AI マスクの推論に使う ONNX Runtime の共有ライブラリ（`onnxruntime.dll`・`libonnxruntime.dylib`・
    /// `libonnxruntime.so`）。`None` なら環境変数 `GENZO_ONNXRUNTIME`、モデルのフォルダ、実行ファイルのフォルダの順に
    /// 探す（[`crate::onnxruntime_candidates`]）。
    #[serde(default)]
    pub onnxruntime: Option<PathBuf>,
    /// 自動バックアップの世代数（DATA-04。既定 5。0 なら自動バックアップをしない）。
    #[serde(default = "default_backup_generations")]
    pub backup_generations: usize,
    /// 終了時に、前回のバックアップからの経過時間によらずバックアップを作るか（DATA-04「またはアプリの
    /// 終了時」。既定は `false` で、終了時も 1 日以上たっていれば作る）。
    #[serde(default)]
    pub backup_on_close: bool,
    /// 取り込みの後に、現像結果からサムネイル（L0）と標準プレビュー（L1）を作り直すか（4 章「最初の
    /// 表示」: 取り込み直後は埋め込み JPEG を使う）。
    ///
    /// 既定は `false`（作り直さない。implementation_status の No.24 の (b)）: 取り込みのたびにすべての
    /// 写真を現像する費用を避け、埋め込みのプレビューのまま表示する。現像設定を変えたとき（PRV-02）と、
    /// 表示に L1 が要るとき（[`crate::Core::request_preview`]）に作る。
    #[serde(default)]
    pub render_previews_after_import: bool,
    /// ファイルの内容のハッシュ（全体の BLAKE3。ORG-03）を、バックグラウンドで自動的に計算してよいか
    /// （開いたとき・取り込みの後。カタログの設定 [`crate::CoreSettings::hash_in_background`] が有効なとき
    /// だけ動く）。
    ///
    /// 既定は `false`: 短い間だけ開く CLI・テストでは、すべてのファイルを読む計算を始めない（CLI は
    /// `genzo hash compute` で明示的に計算する）。デスクトップアプリは `true` にする。明示的な計算
    /// （[`crate::Core::compute_content_hashes`]）と、取り込みの重複の確認での計算は、これによらず行う。
    #[serde(default)]
    pub background_hashing: bool,
    /// 見つからない写真のバックグラウンドの確認（FILE-04。カタログを開いたときと、ボリュームがつながったときに、
    /// すべてのファイルの存在を確かめて印を付ける。`relink` の doc）を自動で動かすか。既定は `false`（CLI・
    /// テストは短い間だけ開くので、`genzo missing check` で明示的に確かめる）。デスクトップアプリは `true` にする。
    /// 明示的な確認（[`crate::Core::check_missing`]）は、これによらず動く。
    #[serde(default)]
    pub background_file_check: bool,
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
    /// ワーカーに追加で渡す環境変数（`GENZO_FFMPEG` など）。コアが動画の再生用のファイルを作るとき（VID-04。
    /// `playback` の doc）も、ここの `GENZO_FFMPEG`・`GENZO_FFPROBE` を使う。
    #[serde(skip)]
    pub worker_env: Vec<(OsString, OsString)>,
    /// 動画の再生用のファイルを作る ffmpeg・ffprobe が [`CoreConfig::worker_env`] になければ、このプロセスの
    /// 環境変数（`GENZO_FFMPEG`・`GENZO_FFPROBE`）・PATH の `ffmpeg`・`ffprobe` を使うか（既定は `true`。CLI・テスト）。
    /// デスクトップアプリは `false` にする（アプリが探した場所（決定事項ログ No.67。設定でフォルダを指定したら、
    /// そこだけ）のものだけを使い、見つからなければ作れない理由を返す）。
    #[serde(default = "default_true")]
    pub ffmpeg_path_fallback: bool,
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
            gpu_allow_software: false,
            backup_dir: None,
            presets_dir: None,
            models_dir: None,
            onnxruntime: None,
            backup_generations: default_backup_generations(),
            backup_on_close: false,
            render_previews_after_import: false,
            background_hashing: false,
            background_file_check: false,
            batch_workers: None,
            background_threads: None,
            worker_test_hooks: false,
            worker_shm_dir: None,
            worker_timeouts: None,
            worker_startup_timeout: None,
            worker_env: Vec::new(),
            ffmpeg_path_fallback: true,
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
        assert!(
            !min.gpu_allow_software,
            "ソフトウェアの GPU は既定で使わない（決定 No.20）"
        );
        assert_eq!(min.backup_generations, 5);
        assert!(
            !min.render_previews_after_import,
            "取り込みの後に L0 / L1 を作り直さない（決定 No.24）"
        );
        assert!(
            !CoreConfig::new("/a.db", "/d").render_previews_after_import,
            "決定 No.24"
        );
        assert!(
            !min.background_hashing && !CoreConfig::new("/a.db", "/d").background_hashing,
            "内容のハッシュのバックグラウンドの計算は、アプリが有効にしたときだけ"
        );
        assert_eq!(min.backup_dir(), PathBuf::from("/d").join("backups"));
        let w: WorkerLaunch = serde_json::from_str(
            r#"{"kind":"self_subcommand","executable":null,"subcommand":"__worker"}"#,
        )
        .unwrap();
        assert_eq!(w, WorkerLaunch::current_exe());
    }
}
