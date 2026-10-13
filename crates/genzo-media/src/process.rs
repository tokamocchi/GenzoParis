//! 子プロセス（ffprobe / ffmpeg）をタイムアウト付きで実行する（04 の 1.2 節・6.3 節）。
//!
//! - 標準入力は閉じる（ffmpeg が入力を待って止まらないように）。
//! - 標準出力は上限（`max_stdout`）まで読む。超えたら強制終了してエラーにする（巨大な出力で
//!   メモリを使い切らないように）。
//! - 標準エラー出力は末尾の [`STDERR_TAIL_BYTES`] バイトだけ残す（エラーの理由の表示用）。
//! - 時間（`timeout`）を超えたら強制終了（kill）して、終了を待ってからエラーにする（ハングへの対策）。
//! - シェルは使わない（引数はそのまま渡すので、ファイル名による注入は起きない）。
//!
//! 長く動く変換（動画の再生用のファイル。VID-04）は [`run_streaming`] で実行する: 標準出力を 1 行ずつ
//! 渡し（ffmpeg の `-progress`）、取り消しを確かめ、出力が途絶えた時間（止まったとみなす時間）で打ち切る。

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{MediaError, Result};

/// 標準エラー出力のうち残す末尾のバイト数。
pub const STDERR_TAIL_BYTES: usize = 4096;

/// 子プロセスの終了を確かめる間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// 子プロセスが終わった後、パイプに残った出力を読み終えるのを待つ最短の時間。
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// [`run_streaming`] が標準出力の 1 行として渡す長さの上限（バイト。超えた分は捨てる。改行のない出力で
/// メモリを使い切らないため）。
pub const MAX_STREAM_LINE_BYTES: usize = 4096;

/// [`run_streaming`] が取り消し・終了を確かめる間隔。
const STREAM_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 実行の結果。
#[derive(Debug)]
pub struct ProcessOutput {
    /// 終了状態。
    pub status: ExitStatus,
    /// 標準出力。
    pub stdout: Vec<u8>,
    /// 標準エラー出力の末尾（UTF-8 でない部分は置き換える）。
    pub stderr_tail: String,
}

/// 標準出力を上限まで読む。上限を超えたら `overflow` を立てて読むのをやめる（パイプを閉じる）。
fn read_capped(mut r: impl Read, max: usize, overflow: &AtomicBool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.len() + n > max {
                    overflow.store(true, Ordering::SeqCst);
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
        }
    }
    out
}

/// 標準エラー出力を最後まで読み、末尾だけ残す。
fn read_tail(mut r: impl Read) -> Vec<u8> {
    let mut tail = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > STDERR_TAIL_BYTES {
                    let cut = tail.len() - STDERR_TAIL_BYTES;
                    tail.drain(..cut);
                }
            }
        }
    }
    tail
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Windows では、子プロセスのためにコンソールのウィンドウを開かない。
fn configure(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// CREATE_NO_WINDOW（Win32 のプロセス作成フラグ）。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

/// `program` を引数 `args` で実行し、終了を待つ（タイムアウトと出力の上限付き）。
///
/// 終了コードが 0 でなくてもエラーにはしない（呼び出し側で [`ProcessOutput::status`] を見る）。
pub fn run_with_timeout<S: AsRef<OsStr>>(
    program: &OsStr,
    args: &[S],
    timeout: Duration,
    max_stdout: usize,
) -> Result<ProcessOutput> {
    let name = program.to_string_lossy().into_owned();
    let mut cmd = Command::new(program);
    cmd.args(args);
    configure(&mut cmd);
    let mut child = cmd.spawn().map_err(|source| MediaError::ToolSpawn {
        program: name.clone(),
        source,
    })?;

    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().expect("標準出力はパイプにしている");
    let stderr = child
        .stderr
        .take()
        .expect("標準エラー出力はパイプにしている");
    // 読み取りのスレッドの結果はチャネルで受け取る（孫プロセスがパイプを開いたままにしても、
    // 待ち続けないように。その場合スレッドはパイプが閉じるまで残る）。
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    {
        let overflow = Arc::clone(&overflow);
        thread::spawn(move || {
            let _ = out_tx.send(read_capped(stdout, max_stdout, &overflow));
        });
    }
    thread::spawn(move || {
        let _ = err_tx.send(read_tail(stderr));
    });

    let deadline = Instant::now() + timeout;
    let timeout_err = || MediaError::ToolTimeout {
        program: name.clone(),
        timeout,
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_and_reap(&mut child);
                return Err(MediaError::io_no_path(e));
            }
        }
        if overflow.load(Ordering::SeqCst) {
            kill_and_reap(&mut child);
            return Err(MediaError::ToolOutputTooLarge {
                program: name,
                max: max_stdout,
            });
        }
        if Instant::now() >= deadline {
            kill_and_reap(&mut child);
            return Err(timeout_err());
        }
        thread::sleep(POLL_INTERVAL);
    };
    // 子プロセスが終われば、パイプに残った出力を読み終えたところで読み取りのスレッドも終わる。
    let wait = deadline
        .saturating_duration_since(Instant::now())
        .max(DRAIN_GRACE);
    let stdout = out_rx.recv_timeout(wait).map_err(|_| timeout_err())?;
    let wait = deadline
        .saturating_duration_since(Instant::now())
        .max(DRAIN_GRACE);
    let stderr = err_rx.recv_timeout(wait).map_err(|_| timeout_err())?;
    if overflow.load(Ordering::SeqCst) {
        return Err(MediaError::ToolOutputTooLarge {
            program: name,
            max: max_stdout,
        });
    }
    Ok(ProcessOutput {
        status,
        stdout,
        stderr_tail: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// [`run_streaming`] の結果。
#[derive(Debug)]
pub struct StreamedOutput {
    /// 終了状態。
    pub status: ExitStatus,
    /// 標準エラー出力の末尾（UTF-8 でない部分は置き換える）。
    pub stderr_tail: String,
}

/// 標準出力を 1 行ずつ（改行と、その直前の `\r` を除き、[`MAX_STREAM_LINE_BYTES`] までに切って）`tx` に送る。
fn send_lines(r: impl Read, tx: mpsc::Sender<String>) {
    let mut r = BufReader::new(r);
    let mut line: Vec<u8> = Vec::new();
    let send = |line: &mut Vec<u8>| {
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let ok = tx.send(String::from_utf8_lossy(line).into_owned()).is_ok();
        line.clear();
        ok
    };
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if buf.is_empty() {
            if !line.is_empty() {
                send(&mut line);
            }
            break;
        }
        let (chunk, found, used) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (&buf[..i], true, i + 1),
            None => (buf, false, buf.len()),
        };
        let room = MAX_STREAM_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        r.consume(used);
        if found && !send(&mut line) {
            break;
        }
    }
}

/// `program` を引数 `args` で実行し、標準出力の行を 1 行ずつ `on_line` に渡しながら終了を待つ（ffmpeg の
/// 変換の進捗など、長く動くもの）。
///
/// - 標準出力に `idle_timeout` の間なにも出なければ、止まったとみなして強制終了し、
///   [`MediaError::ToolTimeout`]（タイムアウトは `idle_timeout`）を返す。全体の時間には上限を置かない
///   （長い動画の変換は数十分かかりうる。取り消しで止める）。
/// - `cancelled` が真を返したら、強制終了して [`MediaError::Cancelled`] を返す（50 ミリ秒
///   ごとに確かめる）。
/// - 終了コードが 0 でなくてもエラーにはしない（呼び出し側で [`StreamedOutput::status`] を見る）。
pub fn run_streaming<S: AsRef<OsStr>>(
    program: &OsStr,
    args: &[S],
    idle_timeout: Duration,
    cancelled: &dyn Fn() -> bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<StreamedOutput> {
    let name = program.to_string_lossy().into_owned();
    let mut cmd = Command::new(program);
    cmd.args(args);
    configure(&mut cmd);
    let mut child = cmd.spawn().map_err(|source| MediaError::ToolSpawn {
        program: name.clone(),
        source,
    })?;
    let stdout = child.stdout.take().expect("標準出力はパイプにしている");
    let stderr = child
        .stderr
        .take()
        .expect("標準エラー出力はパイプにしている");
    let (line_tx, line_rx) = mpsc::channel::<String>();
    let (err_tx, err_rx) = mpsc::channel();
    thread::spawn(move || send_lines(stdout, line_tx));
    thread::spawn(move || {
        let _ = err_tx.send(read_tail(stderr));
    });

    let mut last_activity = Instant::now();
    let mut stdout_open = true;
    let status = loop {
        // 届いている行をすべて渡す（行が届いたら、止まっていない）。
        if stdout_open {
            match line_rx.recv_timeout(STREAM_POLL_INTERVAL) {
                Ok(line) => {
                    last_activity = Instant::now();
                    on_line(&line);
                    while let Ok(line) = line_rx.try_recv() {
                        on_line(&line);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => stdout_open = false,
            }
        } else {
            thread::sleep(STREAM_POLL_INTERVAL);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_and_reap(&mut child);
                return Err(MediaError::io_no_path(e));
            }
        }
        if cancelled() {
            kill_and_reap(&mut child);
            return Err(MediaError::Cancelled);
        }
        if last_activity.elapsed() >= idle_timeout {
            kill_and_reap(&mut child);
            return Err(MediaError::ToolTimeout {
                program: name,
                timeout: idle_timeout,
            });
        }
    };
    // 終わった後に残った行を渡す（孫プロセスがパイプを開いたままでも、待ち続けない）。
    let deadline = Instant::now() + DRAIN_GRACE;
    while stdout_open {
        let rest = deadline.saturating_duration_since(Instant::now());
        match line_rx.recv_timeout(rest) {
            Ok(line) => on_line(&line),
            Err(_) => stdout_open = false,
        }
    }
    let stderr = err_rx
        .recv_timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .max(STREAM_POLL_INTERVAL),
        )
        .unwrap_or_default();
    Ok(StreamedOutput {
        status,
        stderr_tail: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, timeout: Duration, max: usize) -> Result<ProcessOutput> {
        run_with_timeout(OsStr::new("sh"), &["-c", script], timeout, max)
    }

    #[test]
    fn captures_stdout_and_stderr_tail() {
        let out = sh(
            "printf hello; printf 'err-message' >&2; exit 3",
            Duration::from_secs(10),
            1024,
        )
        .unwrap();
        assert_eq!(out.stdout, b"hello");
        assert_eq!(out.stderr_tail, "err-message");
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn timeout_kills_the_child() {
        let start = Instant::now();
        let r = sh("exec sleep 30", Duration::from_millis(200), 1024);
        assert!(matches!(r, Err(MediaError::ToolTimeout { .. })), "{r:?}");
        // 30 秒待たずに戻る。
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn grandchild_holding_the_pipe_does_not_block_forever() {
        // sh はすぐ終わるが、バックグラウンドの sleep が標準出力を開いたままにする。
        let start = Instant::now();
        let r = sh("sleep 30 & printf x", Duration::from_millis(300), 1024);
        assert!(matches!(r, Err(MediaError::ToolTimeout { .. })), "{r:?}");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn output_limit_kills_the_child() {
        let r = sh(
            "while true; do printf '0123456789abcdef'; done",
            Duration::from_secs(20),
            100_000,
        );
        assert!(
            matches!(r, Err(MediaError::ToolOutputTooLarge { max: 100_000, .. })),
            "{r:?}"
        );
    }

    #[test]
    fn long_stderr_keeps_only_the_tail() {
        let out = sh(
            "i=0; while [ $i -lt 2000 ]; do printf 'line %d\\n' $i >&2; i=$((i+1)); done; printf END >&2",
            Duration::from_secs(20),
            1024,
        )
        .unwrap();
        assert!(out.stderr_tail.len() <= STDERR_TAIL_BYTES);
        assert!(out.stderr_tail.ends_with("END"));
    }

    #[test]
    fn missing_program_is_a_spawn_error() {
        let r = run_with_timeout(
            OsStr::new("/nonexistent/genzo-no-such-program"),
            &["x"],
            Duration::from_secs(1),
            10,
        );
        assert!(matches!(r, Err(MediaError::ToolSpawn { .. })), "{r:?}");
    }

    fn stream(
        script: &str,
        idle: Duration,
        cancel_after: Option<Duration>,
    ) -> (Result<StreamedOutput>, Vec<String>) {
        let start = Instant::now();
        let cancelled = move || cancel_after.is_some_and(|d| start.elapsed() >= d);
        let mut lines = Vec::new();
        let r = run_streaming(
            OsStr::new("sh"),
            &["-c", script],
            idle,
            &cancelled,
            &mut |l| lines.push(l.to_owned()),
        );
        (r, lines)
    }

    #[test]
    fn streaming_passes_lines_and_the_stderr_tail() {
        let (r, lines) = stream(
            "printf 'a=1\\r\\nb=2\\n'; printf 'oops' >&2; printf 'last'; exit 4",
            Duration::from_secs(10),
            None,
        );
        let out = r.unwrap();
        assert_eq!(lines, ["a=1", "b=2", "last"]);
        assert_eq!(out.status.code(), Some(4));
        assert_eq!(out.stderr_tail, "oops");
    }

    #[test]
    fn streaming_long_lines_are_cut() {
        let (r, lines) = stream(
            "i=0; while [ $i -lt 1000 ]; do printf '0123456789'; i=$((i+1)); done; printf '\\nend\\n'",
            Duration::from_secs(20),
            None,
        );
        r.unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), MAX_STREAM_LINE_BYTES);
        assert_eq!(lines[1], "end");
    }

    #[test]
    fn streaming_is_cancellable() {
        let start = Instant::now();
        let (r, lines) = stream(
            "while true; do echo tick; sleep 0.05; done",
            Duration::from_secs(30),
            Some(Duration::from_millis(300)),
        );
        assert!(matches!(r, Err(MediaError::Cancelled)), "{r:?}");
        assert!(!lines.is_empty());
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn streaming_stops_when_the_output_stalls() {
        let start = Instant::now();
        // 進み続けている間は打ち切らない（全体の時間は idle より長い）。
        let (r, lines) = stream(
            "i=0; while [ $i -lt 8 ]; do echo $i; sleep 0.1; i=$((i+1)); done",
            Duration::from_millis(600),
            None,
        );
        assert!(r.unwrap().status.success());
        assert_eq!(lines.len(), 8);
        // 出力が途絶えたら打ち切る。
        let (r, _) = stream(
            "echo start; exec sleep 30",
            Duration::from_millis(300),
            None,
        );
        assert!(
            matches!(r, Err(MediaError::ToolTimeout { timeout, .. }) if timeout == Duration::from_millis(300)),
            "{r:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn streaming_missing_program_is_a_spawn_error() {
        let r = run_streaming(
            OsStr::new("/nonexistent/genzo-no-such-program"),
            &["x"],
            Duration::from_secs(1),
            &|| false,
            &mut |_| {},
        );
        assert!(matches!(r, Err(MediaError::ToolSpawn { .. })), "{r:?}");
    }

    #[test]
    fn stdin_is_closed() {
        // 標準入力を読むプログラムがすぐに EOF を受け取って終わる。
        let out = sh("cat; printf done", Duration::from_secs(10), 1024).unwrap();
        assert_eq!(out.stdout, b"done");
    }
}
