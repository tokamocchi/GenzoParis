//! カタログを使うコマンドの共通の処理: コア（genzo-api の [`Core`]）を開く、ジョブを待つ（進捗を
//! 標準エラーへ）、警告を出す、閉じる。
//!
//! - ワーカーは、本体の実行ファイル自身を隠しサブコマンド `__worker` 付きで起動する
//!   （[`WorkerLaunch::current_exe`]。04 の 1.2 節。ワーカーの実行ファイルを別に配布しない）。
//! - カタログのパスは `--catalog`、なければ環境変数 `GENZO_CATALOG`（どちらもなければ使い方の誤り）。
//! - `catalog init` 以外は、カタログのファイルがなければエラーにする（打ち間違えたパスに新しい空の
//!   カタログを作らないため）。
//! - 閉じる前に、バックグラウンドのジョブ（現像設定の変更・取り込みの後のサムネイルの作り直しなど）が
//!   終わるのを待つ（[`Core::wait_idle`]）。待たずに閉じると、それらのジョブは取り消される。
//! - Ctrl+C（K3。`interrupt` の doc）: 開いている間は 1 回目の Ctrl+C ですぐには終了しない。ジョブを
//!   待っていれば取り消して終わるのを待ち（取り込み・書き出しは途中までの結果を返す）、閉じるときは
//!   バックグラウンドのジョブを待たずに閉じる（[`Core::close`] が取り消す）。開いている途中で押された
//!   場合は、開き終えたらすぐに閉じて「取り消し」のエラーにする。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::Receiver;
use genzo_api::{
    ApiError, Core, CoreConfig, Event, JobInfo, JobKind, JobResult, JobState, WorkerLaunch,
};

use crate::args::GlobalArgs;
use crate::error::{CliError, CliResult};
use crate::interrupt::{self, Graceful};
use crate::output::Output;

/// カタログのパスを指定する環境変数。
pub const ENV_CATALOG: &str = "GENZO_CATALOG";
/// データのフォルダを指定する環境変数。
pub const ENV_DATA_DIR: &str = "GENZO_DATA_DIR";

/// ジョブの進捗を確かめる間隔（イベントを待つ時間の上限。結果は状態で決め、この時間では決めない）。
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// カタログのパス（`--catalog`、なければ環境変数 `GENZO_CATALOG`）。
pub fn catalog_path(g: &GlobalArgs) -> CliResult<PathBuf> {
    if let Some(p) = &g.catalog {
        return Ok(p.clone());
    }
    match std::env::var_os(ENV_CATALOG).filter(|v| !v.is_empty()) {
        Some(v) => Ok(PathBuf::from(v)),
        None => Err(CliError::Usage(format!(
            "カタログを --catalog <PATH> か環境変数 {ENV_CATALOG} で指定してください"
        ))),
    }
}

/// データのフォルダ（`--data-dir`、環境変数 `GENZO_DATA_DIR`、なければカタログの隣の
/// `<カタログ名>-data`）。
pub fn data_dir(g: &GlobalArgs, catalog: &Path) -> PathBuf {
    if let Some(d) = &g.data_dir {
        return d.clone();
    }
    if let Some(v) = std::env::var_os(ENV_DATA_DIR).filter(|v| !v.is_empty()) {
        return PathBuf::from(v);
    }
    default_data_dir(catalog)
}

/// カタログの隣の `<カタログ名>-data`。
pub fn default_data_dir(catalog: &Path) -> PathBuf {
    let stem = catalog
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "catalog".to_owned());
    let dir = catalog
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    dir.join(format!("{stem}-data"))
}

/// コアを開くときの指定。
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// カタログがなければ作る（`catalog init`）。
    pub create: bool,
    /// 取り込みの後に現像結果からサムネイル・プレビューを作り直すか（`None` なら genzo-api の既定）。
    pub render_previews_after_import: Option<bool>,
    /// バックアップの置き場（`catalog backup --to`）。
    pub backup_dir: Option<PathBuf>,
}

/// 開いたコアと、イベントの受け取り側。
pub struct Session {
    /// コア。
    pub core: Core,
    events: Receiver<Event>,
    /// 出力。
    pub out: Output,
    /// 開いている間の印（1 回目の Ctrl+C ではすぐに終了しない）。フィールドはこの順に drop するので、
    /// コアを閉じた後に外れる。
    _graceful: Graceful,
}

impl Session {
    /// コアを開く。
    pub fn open(g: &GlobalArgs, out: Output, opts: &OpenOptions) -> CliResult<Self> {
        let catalog = catalog_path(g)?;
        if !opts.create && !catalog.is_file() {
            return Err(CliError::with_hint(
                ApiError::NotFound(format!("カタログ {}", catalog.display())),
                "パスを確かめてください。新しいカタログは genzo catalog init で作れます",
            ));
        }
        let mut config = CoreConfig::new(&catalog, data_dir(g, &catalog));
        config.worker = WorkerLaunch::current_exe();
        config.gpu = g.gpu.into();
        if let Some(r) = opts.render_previews_after_import {
            config.render_previews_after_import = r;
        }
        if let Some(d) = &opts.backup_dir {
            config.backup_dir = Some(d.clone());
        }
        // 開いている途中（終わっていないファイル操作の確定・ワーカーの起動など）の Ctrl+C でも、
        // 開き終えてから正常に閉じる。
        let graceful = Graceful::begin();
        let core = Core::open(config)?;
        let events = core.subscribe();
        let s = Self {
            core,
            events,
            out,
            _graceful: graceful,
        };
        if interrupt::requested() {
            s.close()?;
            return Err(CliError::Api(ApiError::Cancelled));
        }
        Ok(s)
    }

    /// 溜まっているイベントのうち、警告を標準エラーに出す。
    pub fn drain_warnings(&self) {
        for e in self.events.try_iter() {
            self.print_warning(&e);
        }
    }

    fn print_warning(&self, e: &Event) {
        if let Event::Warning {
            message,
            path,
            variant_id,
            ..
        } = e
        {
            let mut text = message.clone();
            if let Some(v) = variant_id {
                text.push_str(&format!("（variant {v}）"));
            }
            if let Some(p) = path {
                text.push_str(&format!("（{}）", p.display()));
            }
            self.out.warn(text);
        }
    }

    /// ジョブが終わるまで待つ。進捗と警告を標準エラーに出す。Ctrl+C で中断を求められたら、ジョブを
    /// 取り消して、終わるのを待つ（途中までの結果を受け取るため）。
    pub fn wait_job(&self, job_id: u64, what: &str) -> CliResult<JobInfo> {
        let mut last: Option<(u64, u64)> = None;
        let mut cancelled = false;
        loop {
            if !cancelled && interrupt::requested() {
                cancelled = true;
                // 既に終わって記録から外れていれば、取り消すものはない（下の状態の確認で分かる）。
                if let Err(e) = self.core.cancel_job(job_id) {
                    tracing::debug!(error = %e, job_id, "ジョブを取り消せない");
                }
            }
            match self.events.recv_timeout(POLL_INTERVAL) {
                Ok(Event::JobProgress {
                    job_id: id,
                    done,
                    total,
                    ..
                }) if id == job_id => {
                    if last != Some((done, total)) {
                        last = Some((done, total));
                        if total > 0 {
                            self.out.progress(format!("{what}: {done}/{total}"));
                        } else {
                            self.out.progress(format!("{what}: {done}"));
                        }
                    }
                }
                Ok(e) => self.print_warning(&e),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                // 送り側がない（コアの終了の後）。状態の確認だけを続ける。
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(POLL_INTERVAL);
                }
            }
            let info = self.core.job(job_id)?;
            if info.state.is_finished() {
                // 終わった後のイベント（警告）も出す。
                self.drain_warnings();
                return Ok(info);
            }
        }
    }

    /// ジョブを待ち、成功しなければエラーにする。
    ///
    /// 取り消された取り込み・書き出しは、途中までの結果を返す（結果に `cancelled: true` の印がある。
    /// 呼び出し側は結果を出し、一部が終わらなかったものとして扱う）。
    pub fn wait_job_ok(&self, job_id: u64, what: &str) -> CliResult<JobInfo> {
        let info = self.wait_job(job_id, what)?;
        match info.state {
            JobState::Succeeded => Ok(info),
            JobState::Cancelled
                if matches!(
                    info.result,
                    Some(JobResult::Import(_) | JobResult::Export(_))
                ) =>
            {
                Ok(info)
            }
            JobState::Cancelled => Err(CliError::Api(ApiError::Cancelled)),
            JobState::Failed | JobState::Running => match info.error {
                Some(error) => Err(CliError::Job {
                    what: what.to_owned(),
                    error,
                }),
                None => Err(CliError::other(format!(
                    "{what}に失敗しました（理由は不明です）"
                ))),
            },
        }
    }

    /// バックグラウンドのジョブを待ってから閉じる（閉じるときのエラーも返す）。
    ///
    /// 待っている間も、ジョブの進捗（取り込みの後のサムネイルの作り直しなど。枚数が多いと長くかかる）と
    /// 警告を標準エラーに出す（何も出さずに止まって見えないように）。Ctrl+C で中断を求められたら、待つのを
    /// やめて閉じる（[`Core::close`] が実行中・待機中のジョブを取り消す。作り直せなかったサムネイルは、
    /// 次に開いたときに作り直す）。
    pub fn close(self) -> CliResult<()> {
        if self.core.running_jobs() > 0 && !interrupt::requested() {
            self.out
                .progress("バックグラウンドの処理（サムネイルの作り直しなど）を待っています…");
        }
        let idle = std::sync::atomic::AtomicBool::new(false);
        let mut closed: Option<Result<(), ApiError>> = None;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                self.core.wait_idle();
                idle.store(true, std::sync::atomic::Ordering::Release);
            });
            let mut last: std::collections::HashMap<u64, (u64, u64)> = Default::default();
            while !idle.load(std::sync::atomic::Ordering::Acquire) {
                if closed.is_none() && interrupt::requested() {
                    self.out.progress(
                        "カタログを閉じています（バックグラウンドの処理は取り消します）…",
                    );
                    // 閉じるとスケジューラが止まり、ジョブが取り消されるので、待っているスレッド
                    // （wait_idle）も戻る。
                    closed = Some(self.core.close());
                }
                match self.events.recv_timeout(POLL_INTERVAL) {
                    Ok(Event::JobProgress {
                        job_id,
                        kind,
                        done,
                        total,
                    }) => {
                        if last.insert(job_id, (done, total)) != Some((done, total)) && total > 0 {
                            self.out
                                .progress(format!("{}: {done}/{total}", job_kind_text(kind)));
                        }
                    }
                    Ok(e) => self.print_warning(&e),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(POLL_INTERVAL);
                    }
                }
            }
        });
        self.drain_warnings();
        let r = closed.unwrap_or_else(|| self.core.close());
        self.drain_warnings();
        r.map_err(CliError::from)
    }
}

/// ジョブの種類の表示名（進捗の表示）。
pub fn job_kind_text(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Import => "取り込み",
        JobKind::Export => "書き出し",
        JobKind::RegeneratePreviews => "サムネイルの作り直し",
        JobKind::IntegrityCheck => "詳細チェック",
        JobKind::ReresolveCaptureTimes => "撮影日時の推定し直し",
        JobKind::CollectGarbage => "キャッシュの回収",
    }
}

impl Drop for Session {
    /// エラーで途中で終わった場合も、溜まっている警告（前回の異常終了など）を出す（コアは drop で閉じる）。
    fn drop(&mut self) {
        self.drain_warnings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_data_dir_is_next_to_the_catalog() {
        assert_eq!(
            default_data_dir(Path::new("/p/cat/main.db")),
            PathBuf::from("/p/cat/main-data")
        );
        assert_eq!(
            default_data_dir(Path::new("catalog.db")),
            PathBuf::from(".").join("catalog-data")
        );
    }
}
