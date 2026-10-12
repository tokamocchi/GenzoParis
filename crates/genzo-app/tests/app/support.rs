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
        let state = AppState::new(config_dir, sink).with_worker(worker());
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
