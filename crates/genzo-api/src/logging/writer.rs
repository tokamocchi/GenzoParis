//! ログのファイル（大きさの上限とローテーション）と、直前のログ（クラッシュレポートに付ける）。

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tracing_subscriber::fmt::MakeWriter;

/// パニックのフックがロックを待つ時間（ログを書いている途中のスレッドがパニックした場合などに、フックが
/// 止まらないようにする）。
pub(crate) const HOOK_LOCK_TIMEOUT: Duration = Duration::from_millis(200);

/// ログ（クラッシュレポートのフォルダを含む）のフォルダを作る（親も。既にあれば何もしない）。
///
/// Unix（Linux・macOS）では、作るフォルダの権限を利用者だけ（0700）にする（ログには写真のファイル名・
/// フォルダの場所が入るため。XDG Base Directory の決まりの、データのフォルダを作るときの権限と同じ）。
/// 既にあるフォルダの権限は変えない（`GENZO_LOG_DIR` で利用者が選んだフォルダなど）。Windows では、
/// フォルダは親（`%LOCALAPPDATA%` は利用者ごと）の権限を受け継ぐ。
pub fn create_log_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// ログ・クラッシュレポートのファイルを開く設定の始まり（Unix では、作るファイルの権限を利用者だけ
/// （0600）にする。[`create_log_dir`] と同じ理由。既にあるファイルの権限は変えない）。
pub(crate) fn private_file_options() -> OpenOptions {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// 大きさの上限とローテーションのあるログのファイル（`<名前>.log`、古いものは `<名前>.1.log` 〜
/// `<名前>.<keep>.log`）。
///
/// - 書き込みで上限（`max_bytes`）を超えるときは、先に名前を 1 つずつずらし（一番古いものは消す）、
///   新しい `<名前>.log` に書く。1 回の書き込み（1 件のログ）は分けない。
/// - 名前を変えられない（ほかのプロセスが開いている・権限など）ときは、そのファイルを空にして書き続ける
///   （ディスクを使い続けないことを優先する）。それもできなければ、書き込みをやめる（アプリは止めない）。
/// - ほかのプロセス（同時に動く CLI など）が同じファイルに書く場合のローテーションの競合は防がない
///   （1 件のログが前後のファイルに分かれることがある）。
pub(crate) struct RotatingFile {
    dir: PathBuf,
    stem: String,
    max_bytes: u64,
    keep: usize,
    file: Option<File>,
    size: u64,
}

impl RotatingFile {
    /// `dir/<stem>.log` を追記で開く（フォルダがなければ作る）。
    pub(crate) fn open(dir: &Path, stem: &str, max_bytes: u64, keep: usize) -> io::Result<Self> {
        create_log_dir(dir)?;
        let mut f = Self {
            dir: dir.to_path_buf(),
            stem: stem.to_owned(),
            max_bytes,
            keep,
            file: None,
            size: 0,
        };
        f.reopen()?;
        Ok(f)
    }

    /// 今のファイルのパス。
    pub(crate) fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.log", self.stem))
    }

    /// `n` 番目に古いファイル（1 から）のパス。
    fn rotated_path(&self, n: usize) -> PathBuf {
        self.dir.join(format!("{}.{n}.log", self.stem))
    }

    fn reopen(&mut self) -> io::Result<()> {
        self.file = None;
        let file = private_file_options()
            .create(true)
            .append(true)
            .open(self.path())?;
        self.size = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    /// 1 件のログを書く。失敗は返さない（ログのためにアプリを止めない）。
    pub(crate) fn write(&mut self, buf: &[u8]) {
        let len = buf.len() as u64;
        if self.size > 0 && self.size.saturating_add(len) > self.max_bytes {
            self.rotate();
        }
        if let Some(f) = self.file.as_mut()
            && f.write_all(buf).is_ok()
        {
            self.size = self.size.saturating_add(len);
        }
    }

    /// 名前を 1 つずつずらし、新しいファイルを開く。
    fn rotate(&mut self) {
        // 開いたまま名前を変えると、Windows では失敗しうる（ほかのプロセスが開いている場合も）。
        self.file = None;
        let _ = fs::remove_file(self.rotated_path(self.keep));
        for n in (1..self.keep).rev() {
            let from = self.rotated_path(n);
            if from.exists() {
                let _ = fs::rename(&from, self.rotated_path(n + 1));
            }
        }
        let renamed = self.keep > 0 && fs::rename(self.path(), self.rotated_path(1)).is_ok();
        if !renamed {
            let _ = self.truncate();
        }
        if self.reopen().is_err() {
            self.file = None;
        } else if self.size >= self.max_bytes {
            // 名前を変えられなかったのに空にもできていない（ほかのプロセスが書き足したなど）。
            let _ = self.truncate();
        }
    }

    /// 今のファイルを空にする（開き直す。Windows の追記だけのハンドルでは `set_len` できないため）。
    pub(crate) fn truncate(&mut self) -> io::Result<()> {
        self.file = None;
        private_file_options()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.path())?;
        self.reopen()
    }
}

/// 1 件のログの大きさの上限（バイト。**仮置き**: 16 KiB）。超えた分は捨て、捨てたことを書く
/// （[`clip_record`]。長い引数・外部のプログラムの出力などで、1 行が数 MB にならないように）。
pub(crate) const MAX_RECORD_BYTES: usize = 16 * 1024;

/// 1 件のログを [`MAX_RECORD_BYTES`] までにする。超えるときは UTF-8 の文字の途中で切らないように前を残し、
/// 末尾に省略した大きさを書く（改行で終える）。
pub(crate) fn clip_record(buf: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if buf.len() <= MAX_RECORD_BYTES {
        return std::borrow::Cow::Borrowed(buf);
    }
    let mut end = MAX_RECORD_BYTES;
    // UTF-8 の続きのバイト（0b10xx_xxxx）の前で切らない。
    while end > 0 && (buf[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    let mut out = buf[..end].to_vec();
    out.extend_from_slice(
        format!(
            "…（ログの 1 件が長すぎるため、残りの {} バイトを省略しました）\n",
            buf.len() - end
        )
        .as_bytes(),
    );
    std::borrow::Cow::Owned(out)
}

/// 直前のログ（新しいものが最後）。合計の大きさ（`max_bytes`）を超えたら古いものから捨てる。
pub(crate) struct RecentLines {
    lines: VecDeque<String>,
    bytes: usize,
    max_bytes: usize,
}

impl RecentLines {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: 0,
            max_bytes,
        }
    }

    pub(crate) fn push(&mut self, text: String) {
        self.bytes += text.len();
        self.lines.push_back(text);
        while self.bytes > self.max_bytes && self.lines.len() > 1 {
            if let Some(old) = self.lines.pop_front() {
                self.bytes -= old.len();
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.lines.clear();
        self.bytes = 0;
    }

    /// 全体をつないだ文字列。
    pub(crate) fn joined(&self) -> String {
        let mut s = String::with_capacity(self.bytes);
        for l in &self.lines {
            s.push_str(l);
        }
        s
    }
}

/// ログの書き込み先（ファイル（あれば）と直前のログ）。tracing の fmt の層から使う。
pub(crate) struct Sink {
    pub(crate) file: Mutex<Option<RotatingFile>>,
    pub(crate) recent: Mutex<RecentLines>,
}

impl Sink {
    pub(crate) fn new(file: Option<RotatingFile>, recent_bytes: usize) -> Self {
        Self {
            file: Mutex::new(file),
            recent: Mutex::new(RecentLines::new(recent_bytes)),
        }
    }

    /// 1 件のログを書く（[`MAX_RECORD_BYTES`] を超える分は捨てる）。
    pub(crate) fn write_event(&self, buf: &[u8]) {
        let buf = &*clip_record(buf);
        self.recent
            .lock()
            .push(String::from_utf8_lossy(buf).into_owned());
        if let Some(f) = self.file.lock().as_mut() {
            f.write(buf);
        }
    }

    /// パニックのフックから 1 件のログを書く（ロックを待ちすぎない。取れなければ書かない）。
    pub(crate) fn write_event_from_hook(&self, buf: &[u8]) {
        let buf = &*clip_record(buf);
        if let Some(mut recent) = self.recent.try_lock_for(HOOK_LOCK_TIMEOUT) {
            recent.push(String::from_utf8_lossy(buf).into_owned());
        }
        if let Some(mut file) = self.file.try_lock_for(HOOK_LOCK_TIMEOUT)
            && let Some(f) = file.as_mut()
        {
            f.write(buf);
        }
    }

    /// 直前のログ（パニックのフックから。ロックを取れなければ `None`）。
    pub(crate) fn recent_from_hook(&self) -> Option<String> {
        self.recent
            .try_lock_for(HOOK_LOCK_TIMEOUT)
            .map(|r| r.joined())
    }

    /// 今のファイルのパス（ファイルに書いていなければ `None`）。
    pub(crate) fn file_path(&self) -> Option<PathBuf> {
        self.file.lock().as_ref().map(RotatingFile::path)
    }
}

/// [`Sink`] に書く tracing の書き手。
#[derive(Clone)]
pub(crate) struct SinkMakeWriter(pub(crate) Arc<Sink>);

/// 1 件のログの書き手（fmt の層は、1 件を書式にしてから 1 回で書く）。
pub(crate) struct SinkWriter(Arc<Sink>);

impl Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_event(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SinkMakeWriter {
    type Writer = SinkWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SinkWriter(Arc::clone(&self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_by_size_and_keeps_a_fixed_number_of_old_files() {
        let d = tempfile::tempdir().unwrap();
        let mut f = RotatingFile::open(d.path(), "app", 100, 2).unwrap();
        let line = [b'a'; 40];
        for _ in 0..2 {
            f.write(&line);
        }
        assert_eq!(fs::metadata(f.path()).unwrap().len(), 80);
        // 上限を超える書き込みの前に名前をずらす（1 件は分けない）。
        f.write(&[b'b'; 40]);
        assert_eq!(fs::read(d.path().join("app.1.log")).unwrap(), [b'a'; 80]);
        assert_eq!(fs::read(f.path()).unwrap(), [b'b'; 40]);
        for c in *b"cdef" {
            f.write(&[c; 70]);
        }
        // 古いものは 2 つまで（app.1.log・app.2.log）。
        assert_eq!(fs::read(f.path()).unwrap(), [b'f'; 70]);
        assert_eq!(fs::read(d.path().join("app.1.log")).unwrap(), [b'e'; 70]);
        assert_eq!(fs::read(d.path().join("app.2.log")).unwrap(), [b'd'; 70]);
        assert!(!d.path().join("app.3.log").exists());
    }

    #[test]
    fn a_line_larger_than_the_limit_is_written_whole() {
        let d = tempfile::tempdir().unwrap();
        let mut f = RotatingFile::open(d.path(), "cli", 10, 1).unwrap();
        f.write(&[b'x'; 25]);
        assert_eq!(fs::metadata(f.path()).unwrap().len(), 25);
        f.write(b"y");
        assert_eq!(fs::read(f.path()).unwrap(), b"y");
        assert_eq!(fs::metadata(d.path().join("cli.1.log")).unwrap().len(), 25);
    }

    #[test]
    fn reopening_appends_and_counts_the_existing_size() {
        let d = tempfile::tempdir().unwrap();
        {
            let mut f = RotatingFile::open(d.path(), "app", 50, 1).unwrap();
            f.write(&[b'a'; 30]);
        }
        let mut f = RotatingFile::open(d.path(), "app", 50, 1).unwrap();
        f.write(&[b'b'; 30]);
        // 開き直しても前の大きさを数えるので、上限を超える前にずらす。
        assert_eq!(fs::read(d.path().join("app.1.log")).unwrap(), [b'a'; 30]);
        assert_eq!(fs::read(f.path()).unwrap(), [b'b'; 30]);
    }

    #[test]
    fn truncate_empties_the_current_file_and_keeps_writing() {
        let d = tempfile::tempdir().unwrap();
        let mut f = RotatingFile::open(d.path(), "app", 1000, 1).unwrap();
        f.write(b"secret path /home/u/photo.ARW\n");
        f.truncate().unwrap();
        assert_eq!(fs::metadata(f.path()).unwrap().len(), 0);
        f.write(b"next\n");
        assert_eq!(fs::read(f.path()).unwrap(), b"next\n");
    }

    /// Unix では、作るログのフォルダ・ファイルを利用者だけが読めるようにする（ログに写真のファイル名・
    /// フォルダの場所が入るため）。既にあるフォルダの権限は変えない。
    #[cfg(unix)]
    #[test]
    fn new_log_folders_and_files_are_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let d = tempfile::tempdir().unwrap();
        let parent = d.path().join("data");
        let dir = parent.join("logs");
        let mut f = RotatingFile::open(&dir, "app", 100, 1).unwrap();
        f.write(&[b'a'; 60]);
        assert_eq!(mode(&parent), 0o700);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&f.path()), 0o600);
        // ずらした後の新しいファイルも同じ。
        f.write(&[b'b'; 60]);
        assert_eq!(mode(&f.path()), 0o600);
        assert_eq!(mode(&dir.join("app.1.log")), 0o600);
        // 既にあるフォルダの権限は変えない（GENZO_LOG_DIR で利用者が選んだフォルダなど）。
        let shared = d.path().join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
        create_log_dir(&shared).unwrap();
        assert_eq!(mode(&shared), 0o755);
    }

    #[test]
    fn long_records_are_clipped_at_a_character_boundary() {
        // 検証の指摘: 1 件を分けずに書くので、長い引数などで 1 行が数 MB になりえた。
        let short = b"short\n";
        assert_eq!(&*clip_record(short), short);
        let exact = vec![b'a'; MAX_RECORD_BYTES];
        assert_eq!(clip_record(&exact).len(), MAX_RECORD_BYTES);
        // 「あ」は UTF-8 で 3 バイト。上限が文字の途中に来るようにする。
        let long = format!("x{}", "あ".repeat(MAX_RECORD_BYTES));
        let clipped = clip_record(long.as_bytes());
        let text = std::str::from_utf8(&clipped).expect("文字の途中で切らない");
        assert!(text.len() < MAX_RECORD_BYTES + 200, "{}", text.len());
        assert!(text.starts_with("xあ"));
        assert!(
            text.ends_with("バイトを省略しました）\n"),
            "{}",
            &text[text.len() - 120..]
        );
        let kept = text.find('…').unwrap();
        assert!(kept <= MAX_RECORD_BYTES);
        assert!(text.contains(&format!("残りの {} バイト", long.len() - kept)));
        // Sink もファイル・直前のログの両方で切る。
        let d = tempfile::tempdir().unwrap();
        let file = RotatingFile::open(d.path(), "cli", 1 << 30, 1).unwrap();
        let sink = Sink::new(Some(file), 1 << 30);
        sink.write_event(long.as_bytes());
        sink.write_event_from_hook(long.as_bytes());
        let written = fs::read_to_string(d.path().join("cli.log")).unwrap();
        assert_eq!(written, format!("{text}{text}"));
        assert_eq!(sink.recent_from_hook().unwrap(), written);
    }

    #[test]
    fn recent_lines_keep_the_newest_within_the_budget() {
        let mut r = RecentLines::new(10);
        r.push("aaaa\n".to_owned());
        r.push("bbbb\n".to_owned());
        r.push("cccc\n".to_owned());
        assert_eq!(r.joined(), "bbbb\ncccc\n");
        // 1 件だけなら、予算を超えても残す。
        r.push("x".repeat(30));
        assert_eq!(r.joined(), "x".repeat(30));
        r.clear();
        assert_eq!(r.joined(), "");
    }

    #[test]
    fn sink_writes_to_file_and_recent_lines() {
        let d = tempfile::tempdir().unwrap();
        let file = RotatingFile::open(d.path(), "app", 1000, 1).unwrap();
        let sink = Arc::new(Sink::new(Some(file), 1000));
        let mw = SinkMakeWriter(Arc::clone(&sink));
        mw.make_writer().write_all(b"one\n").unwrap();
        sink.write_event_from_hook(b"two\n");
        assert_eq!(
            fs::read_to_string(d.path().join("app.log")).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(sink.recent_from_hook().unwrap(), "one\ntwo\n");
        assert_eq!(sink.file_path(), Some(d.path().join("app.log")));
        // ファイルなし（ログのファイルを使わない設定）でも、直前のログは残す。
        let sink = Sink::new(None, 1000);
        sink.write_event(b"three\n");
        assert_eq!(sink.recent_from_hook().unwrap(), "three\n");
        assert_eq!(sink.file_path(), None);
    }
}
