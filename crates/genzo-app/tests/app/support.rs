//! 結合テストの共通の道具（一時フォルダ・AppState・イベントの記録・合成の写真・コマンドの実行）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use genzo_api::{
    ErrorInfo, ErrorKind, Event, GpuMode, ImportReport, JobInfo, JobResult, JobState, Response,
    VariantSummary, WorkerLaunch,
};
use genzo_app::commands;
use genzo_app::state::{AppState, EventSink};
use parking_lot::{Condvar, Mutex};
use serde_json::json;

/// ジョブ・描画・イベントを待つ時間の上限（負荷の高い CI でも誤って打ち切らない長さ）。
pub const WAIT_LIMIT: Duration = Duration::from_secs(300);

/// ワーカーの起動方法: このテストの実行ファイル自身を `__worker` 付きで起動する（main.rs の doc）。
pub fn worker() -> WorkerLaunch {
    WorkerLaunch::SelfSubcommand {
        executable: Some(std::env::current_exe().expect("テストの実行ファイルのパス")),
        subcommand: genzo_worker::WORKER_SUBCOMMAND.to_owned(),
    }
}

/// 合図で終わるワーカーの隠しサブコマンドの接頭辞（後ろに合図のファイルのパスを付ける。main.rs）。
pub const EXITING_WORKER_PREFIX: &str = "__worker_exit_when=";

/// 合図で終わるワーカー（[`ExitSwitch`]）の中で: 生きている印のファイルを作り、合図のファイルができたら、
/// 印を消してすぐに終わる（応答を返さずに。ワーカーの異常終了と同じ）。
pub fn exit_when_signalled(marker: PathBuf) {
    let alive = ExitSwitch::alive_path(&marker, std::process::id());
    if std::fs::write(&alive, b"").is_err() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("test-exit-switch".to_owned())
        .spawn(move || {
            loop {
                if marker.exists() {
                    let _ = std::fs::remove_file(&alive);
                    std::process::exit(3);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
}

/// ワーカーのプロセスを合図で終わらせる仕掛け（閉じる処理の「閉じた後の段階のエラー」を起こすため。
/// コアは、終了の処理でワーカーに終了を頼み、ワーカーがもういなければエラーにする）。
pub struct ExitSwitch {
    marker: PathBuf,
}

impl ExitSwitch {
    /// `dir` に合図のファイルを置く仕掛け。
    pub fn new(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        Self {
            marker: dir.join("exit-now"),
        }
    }

    fn alive_path(marker: &Path, pid: u32) -> PathBuf {
        let mut s = marker.as_os_str().to_owned();
        s.push(format!(".alive-{pid}"));
        PathBuf::from(s)
    }

    /// 生きているワーカーの数（印のファイルの数）。
    pub fn alive(&self) -> usize {
        let prefix = format!(
            "{}.alive-",
            self.marker.file_name().unwrap().to_string_lossy()
        );
        std::fs::read_dir(self.marker.parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .is_ok_and(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            })
            .count()
    }

    /// このワーカーの起動方法（テストの実行ファイル自身を、合図のファイルのパス付きの隠しサブコマンドで起動する）。
    pub fn launch(&self) -> WorkerLaunch {
        WorkerLaunch::SelfSubcommand {
            executable: Some(std::env::current_exe().expect("テストの実行ファイルのパス")),
            subcommand: format!("{EXITING_WORKER_PREFIX}{}", self.marker.display()),
        }
    }

    /// 今動いているワーカーをすべて終わらせ、終わるのを待つ。合図は外すので、後から起動するワーカーは動く。
    pub fn exit_all(&self) {
        assert!(self.alive() > 0, "ワーカーが動いていない");
        std::fs::write(&self.marker, b"").unwrap();
        wait_until("ワーカーが終わる", || self.alive() == 0);
        std::fs::remove_file(&self.marker).unwrap();
    }
}

/// EventSink で受けたイベントの記録。
#[derive(Default)]
pub struct Recorder {
    events: Mutex<Vec<Event>>,
    changed: Condvar,
}

impl Recorder {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// AppState に渡す送り先。
    pub fn sink(self: &Arc<Self>) -> EventSink {
        let me = Arc::clone(self);
        Arc::new(move |e: &Event| me.push(e.clone()))
    }

    pub fn push(&self, e: Event) {
        self.events.lock().push(e);
        self.changed.notify_all();
    }

    /// 今までに受けた数（[`Recorder::since`] の印）。
    pub fn mark(&self) -> usize {
        self.events.lock().len()
    }

    /// 印より後に受けたイベント。
    pub fn since(&self, mark: usize) -> Vec<Event> {
        self.events.lock()[mark..].to_vec()
    }

    /// 印より後に、条件に合うイベントが届くまで待つ。
    pub fn wait_for(&self, mark: usize, what: &str, pred: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + WAIT_LIMIT;
        let mut events = self.events.lock();
        loop {
            if let Some(e) = events[mark..].iter().find(|e| pred(e)) {
                return e.clone();
            }
            if self.changed.wait_until(&mut events, deadline).timed_out() {
                panic!("{what} のイベントが届かない: {:?}", &events[mark..]);
            }
        }
    }
}

/// テストの環境（一時フォルダとイベントの記録）。
pub struct Env {
    pub dir: tempfile::TempDir,
    pub events: Arc<Recorder>,
}

impl Env {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("一時フォルダ"),
            events: Recorder::new(),
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// アプリの設定のフォルダ（settings.json）。
    pub fn config_dir(&self) -> PathBuf {
        self.root().join("config")
    }

    /// カタログのパス（名前に空白と日本語を含むフォルダの下）。
    pub fn catalog(&self, name: &str) -> PathBuf {
        self.root().join("カタログ 置き場").join(name)
    }

    /// 写真のフォルダ（名前に空白と日本語を含む）。
    pub fn photos(&self, name: &str) -> PathBuf {
        self.root().join("写真 フォルダ").join(name)
    }

    /// イベントを記録する AppState（GPU は使わない）。
    pub fn state(&self) -> AppState {
        self.state_with(self.config_dir(), self.events.sink())
    }

    /// 設定のフォルダとイベントの送り先を指定した AppState（GPU は使わない）。
    pub fn state_with(&self, config_dir: PathBuf, sink: EventSink) -> AppState {
        self.state_with_worker(config_dir, sink, worker())
    }

    /// ワーカーの起動方法も指定した AppState（GPU は使わない）。
    pub fn state_with_worker(
        &self,
        config_dir: PathBuf,
        sink: EventSink,
        launch: WorkerLaunch,
    ) -> AppState {
        let state = AppState::new(config_dir, sink).with_worker(launch);
        state
            .update_settings(|s| s.gpu = GpuMode::Off)
            .expect("設定を保存できる");
        state
    }
}

/// コアのコマンド（JSON）を実行する（画面の `core_execute` と同じ処理）。
pub fn exec(state: &AppState, command: serde_json::Value) -> Result<Response, ErrorInfo> {
    commands::execute_command(state, command)
}

/// コアのコマンドを実行して、成功を確かめる。
pub fn ok(state: &AppState, command: serde_json::Value) -> Response {
    let text = command.to_string();
    exec(state, command).unwrap_or_else(|e| panic!("{text} が失敗した: {e:?}"))
}

/// エラーの分類を確かめる。
#[track_caller]
pub fn assert_kind<T: std::fmt::Debug>(r: Result<T, ErrorInfo>, kind: ErrorKind) -> ErrorInfo {
    match r {
        Ok(v) => panic!("{kind:?} のエラーになるはずが成功した: {v:?}"),
        Err(e) => {
            assert_eq!(e.kind, kind, "{e:?}");
            assert_eq!(e.user_actionable, kind.user_actionable(), "{e:?}");
            assert_eq!(e.retryable, kind.retryable(), "{e:?}");
            e
        }
    }
}

/// ジョブが終わるまで待つ（WaitJob のコマンド）。
pub fn wait_job(state: &AppState, job_id: u64) -> JobInfo {
    let timeout_ms = WAIT_LIMIT.as_millis() as u64;
    match ok(
        state,
        json!({"type": "wait_job", "job_id": job_id, "timeout_ms": timeout_ms}),
    ) {
        Response::Job(info) => {
            assert!(info.state.is_finished(), "{info:?}");
            info
        }
        other => panic!("{other:?}"),
    }
}

/// フォルダを取り込む（ImportFolder → WaitJob）。
pub fn import(state: &AppState, dir: &Path) -> ImportReport {
    let job_id = match ok(
        state,
        json!({"type": "import_folder", "path": dir, "recursive": true}),
    ) {
        Response::JobStarted { job_id } => job_id,
        other => panic!("{other:?}"),
    };
    let info = wait_job(state, job_id);
    assert_eq!(info.state, JobState::Succeeded, "{info:?}");
    match info.result {
        Some(JobResult::Import(r)) => r,
        other => panic!("{other:?}"),
    }
}

/// 既定の条件で検索して、すべての写真を返す（Search → Range）。
pub fn all_items(state: &AppState) -> Vec<VariantSummary> {
    let (generation, count) = match ok(state, json!({"type": "search"})) {
        Response::Search(r) => (r.generation, r.count),
        other => panic!("{other:?}"),
    };
    let mut items = Vec::new();
    let mut start = 0;
    while start < count {
        match ok(
            state,
            json!({"type": "range", "generation": generation, "start": start, "len": 100}),
        ) {
            Response::Range(r) => items.extend(r.items),
            other => panic!("{other:?}"),
        }
        start += 100;
    }
    assert_eq!(items.len() as u64, count);
    items
}

/// 名前の写真。
pub fn by_name<'a>(items: &'a [VariantSummary], name: &str) -> &'a VariantSummary {
    items
        .iter()
        .find(|s| s.file_name == name && s.is_master)
        .unwrap_or_else(|| {
            panic!(
                "{name} がない: {:?}",
                items.iter().map(|s| &s.file_name).collect::<Vec<_>>()
            )
        })
}

/// 合成の画素（グラデーション。彩度の高い色を含む）。
fn gradient(w: u32, h: u32, seed: u8) -> image::RgbImage {
    image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            (x * 255 / w.max(1)) as u8,
            (y * 255 / h.max(1)) as u8,
            seed.wrapping_mul(37).wrapping_add(40),
        ])
    })
}

/// 合成の写真を書く（拡張子で JPEG / PNG。image crate）。
pub fn write_image(path: &Path, (w, h): (u32, u32), seed: u8) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    gradient(w, h, seed).save(path).unwrap();
}

/// 条件が成り立つまで待つ。
pub fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT_LIMIT;
    while !f() {
        assert!(Instant::now() < deadline, "{what} にならない");
        std::thread::sleep(Duration::from_millis(20));
    }
}
