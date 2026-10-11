//! ロックファイルによるプロセス間の排他（カタログを同時に開かない・共有メモリの一時ディレクトリの持ち主
//! が生きているかを確かめる）。
//!
//! 標準ライブラリの `File::try_lock` は Rust 1.89 からのため（この workspace の最小の版は 1.88）、OS ごとに
//! 作る:
//! - Unix（Linux・macOS）: `flock(LOCK_EX | LOCK_NB)`（rustix。安全な関数）。
//! - Windows: ほかのプロセスと共有しないで開く（`share_mode(0)`。開いている間は、ほかのプロセス（同じ
//!   プロセスの別のハンドルを含む）が開けない）。
//!
//! どちらも、プロセスが終わると（異常終了を含む）OS が解放する。同じプロセスの中でも、別に開いた
//! ハンドル同士は排他になる（Unix の `flock` は開いたファイルの記述ごと、Windows は共有の設定）。

use std::fs::File;
use std::io;
use std::path::Path;

/// ロックしているファイル（drop で解放する）。
#[derive(Debug)]
pub struct LockedFile {
    _file: File,
}

/// [`try_lock_exclusive`] の結果。
#[derive(Debug)]
pub enum TryLock {
    /// ロックした。
    Locked(LockedFile),
    /// 別のプロセス（または同じプロセスの別のハンドル）がロックしている。
    WouldBlock,
    /// このファイルシステムではロックが使えない（一部のネットワークのボリュームなど）。理由。
    Unsupported(io::Error),
}

/// `path` のロックファイルを排他でロックする（待たない）。`create` なら、なければ作る（中身は空のまま。
/// 既にあっても中身は変えない）。`create` でなく、ファイルがなければ `Err`（`NotFound`）。
pub fn try_lock_exclusive(path: &Path, create: bool) -> io::Result<TryLock> {
    imp::try_lock_exclusive(path, create)
}

#[cfg(unix)]
mod imp {
    use super::*;

    pub(super) fn try_lock_exclusive(path: &Path, create: bool) -> io::Result<TryLock> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .open(path)?;
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(TryLock::Locked(LockedFile { _file: file })),
            Err(e) if e == rustix::io::Errno::WOULDBLOCK || e == rustix::io::Errno::AGAIN => {
                Ok(TryLock::WouldBlock)
            }
            Err(e) => Ok(TryLock::Unsupported(e.into())),
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::fs::OpenOptionsExt;

    use super::*;

    /// ERROR_SHARING_VIOLATION（Win32）。ほかのハンドルが共有しないで開いている。
    const ERROR_SHARING_VIOLATION: i32 = 32;
    /// ERROR_LOCK_VIOLATION（Win32）。
    const ERROR_LOCK_VIOLATION: i32 = 33;

    pub(super) fn try_lock_exclusive(path: &Path, create: bool) -> io::Result<TryLock> {
        // ウイルス対策ソフトなどが作った直後のファイルを短く開いていることがあるので、少しだけ試し直す。
        let mut tries = 0;
        loop {
            let opened = File::options()
                .read(true)
                .write(true)
                .create(create)
                .truncate(false)
                .share_mode(0)
                .open(path);
            match opened {
                Ok(file) => return Ok(TryLock::Locked(LockedFile { _file: file })),
                Err(e)
                    if matches!(
                        e.raw_os_error(),
                        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
                    ) =>
                {
                    tries += 1;
                    if tries >= 3 {
                        return Ok(TryLock::WouldBlock);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::*;

    pub(super) fn try_lock_exclusive(path: &Path, create: bool) -> io::Result<TryLock> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .open(path)?;
        drop(file);
        Ok(TryLock::Unsupported(io::Error::new(
            io::ErrorKind::Unsupported,
            "この OS ではファイルのロックに対応していません",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_fails_until_the_first_is_released() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.lock");
        assert_eq!(
            try_lock_exclusive(&p, false).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let first = match try_lock_exclusive(&p, true).unwrap() {
            TryLock::Locked(l) => l,
            other => panic!("{other:?}"),
        };
        assert!(matches!(
            try_lock_exclusive(&p, true).unwrap(),
            TryLock::WouldBlock
        ));
        drop(first);
        assert!(matches!(
            try_lock_exclusive(&p, false).unwrap(),
            TryLock::Locked(_)
        ));
    }
}
