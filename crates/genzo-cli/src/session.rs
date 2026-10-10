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

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::Receiver;
use genzo_api::{ApiError, Core, CoreConfig, Event, JobInfo, JobKind, JobState, WorkerLaunch};

use crate::args::GlobalArgs;
use crate::error::{CliError, CliResult};
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
        let core = Core::open(config)?;
        let events = core.subscribe();
        Ok(Self { core, events, out })
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

    /// ジョブが終わるまで待つ。進捗と警告を標準エラーに出す。
    pub fn wait_job(&self, job_id: u64, what: &str) -> CliResult<JobInfo> {
        let mut last: Option<(u64, u64)> = None;
        loop {
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
    pub fn wait_job_ok(&self, job_id: u64, what: &str) -> CliResult<JobInfo> {
        let info = self.wait_job(job_id, what)?;
        match info.state {
            JobState::Succeeded => Ok(info),
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
    /// 警告を標準エラーに出す（何も出さずに止まって見えないように）。
    pub fn close(self) -> CliResult<()> {
        if self.core.running_jobs() > 0 {
            self.out
                .progress("バックグラウンドの処理（サムネイルの作り直しなど）を待っています…");
        }
        let idle = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                self.core.wait_idle();
                idle.store(true, std::sync::atomic::Ordering::Release);
            });
            let mut last: std::collections::HashMap<u64, (u64, u64)> = Default::default();
            while !idle.load(std::sync::atomic::Ordering::Acquire) {
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
        let r = self.core.close();
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
