//! ファイルの内容をストレージへ書き出す（fsync）。OS ごとの違いをここにまとめる（指摘 F19）。
//!
//! | 関数 | 用途 | Apple（macOS） | それ以外の OS |
//! |---|---|---|---|
//! | [`sync_file_full`] | 失うと困るファイル（書き出し・カタログのバックアップ） | `fcntl(F_FULLFSYNC)`（装置のキャッシュまで書き出す）。失敗したら `fsync` に戻す | `File::sync_all` |
//! | [`sync_file_fast`] | 作り直せるキャッシュ（L1 のプレビューなど） | `fsync`（装置のキャッシュは書き出さない） | `File::sync_all` |
//!
//! - Rust の std の `File::sync_all` は、Apple では `fcntl(F_FULLFSYNC)` だけを呼び（EINTR のときだけ
//!   やり直す）、失敗をそのまま返す（Rust 1.97.0 のソース `library/std/src/sys/fs/unix.rs` で確認）。
//!   F_FULLFSYNC に対応しないファイルシステム（SMB の NAS など。Go の golang/go#64215 では ENOTSUP）
//!   では、書き出し・バックアップが毎回失敗する。SQLite（os_unix.c の full_fsync）と同じく、
//!   F_FULLFSYNC が失敗したら（理由によらず）`fsync` に戻す。戻した場合は装置のキャッシュに残り
//!   うる（電源断で失いうる）ので、[`SyncLevel::Fallback`] で知らせる。
//! - 作り直せるキャッシュに F_FULLFSYNC を使うと、書き込みごとに装置のキャッシュの書き出しが起き、
//!   取り込み直後の大量の生成が遅くなる（thumbs.db を `synchronous=NORMAL` にしている設計と
//!   釣り合わない）。通常の `fsync` にする（中身が途中までのファイルを残さないため、同期は省かない）。
//! - **要確認（実機）**: macOS の SMB・WebDAV・exFAT などで F_FULLFSYNC が返すエラーと、戻した場合の
//!   動き。F_FULLFSYNC と fsync の時間の差（PoC-6 の計測の項目）。

use std::fs::File;
use std::io;

/// [`sync_file_full`] がどこまで書き出したか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncLevel {
    /// 求めたとおり書き出した（Apple では F_FULLFSYNC。それ以外の OS では `File::sync_all`）。
    Full,
    /// Apple で F_FULLFSYNC が失敗し、`fsync` に戻した（OS のバッファは書き出したが、装置の
    /// キャッシュに残りうる）。
    Fallback,
}

/// 失うと困るファイルの内容をストレージへ書き出す（モジュールの説明を参照）。
///
/// Apple では F_FULLFSYNC を試し、失敗したら `fsync` に戻す。どちらも失敗したら `fsync` のエラーを
/// 返す。それ以外の OS では `File::sync_all`（Linux・Windows の std は戻す必要がない）。
pub fn sync_file_full(file: &File) -> io::Result<SyncLevel> {
    #[cfg(target_vendor = "apple")]
    {
        full_sync_with_fallback(
            || rustix::fs::fcntl_fullfsync(file).map_err(io::Error::from),
            || rustix::fs::fsync(file).map_err(io::Error::from),
        )
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        file.sync_all().map(|()| SyncLevel::Full)
    }
}

/// 作り直せるファイル（キャッシュ）の内容をストレージへ書き出す。Apple でも F_FULLFSYNC を使わず
/// `fsync` にする（モジュールの説明を参照）。
pub fn sync_file_fast(file: &File) -> io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        retry_interrupted(|| rustix::fs::fsync(file).map_err(io::Error::from))
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        file.sync_all()
    }
}

/// [`sync_file_full`] の Apple での戻し方。OS によらずテストできるよう、呼ぶ処理を引数で受け取る
/// （`full` は F_FULLFSYNC、`plain` は fsync）。
///
/// どちらも EINTR（[`io::ErrorKind::Interrupted`]）ならやり直す。`full` がそれ以外で失敗したら、
/// 理由によらず `plain` に戻す（SQLite と同じ。対応しないファイルシステムが返すエラーの種類は
/// 実機で確かめていないため、種類で絞らない）。
#[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
pub(crate) fn full_sync_with_fallback(
    full: impl FnMut() -> io::Result<()>,
    plain: impl FnMut() -> io::Result<()>,
) -> io::Result<SyncLevel> {
    match retry_interrupted(full) {
        Ok(()) => Ok(SyncLevel::Full),
        Err(_) => retry_interrupted(plain).map(|()| SyncLevel::Fallback),
    }
}

/// EINTR の間はやり直す。
#[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
fn retry_interrupted(mut f: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    loop {
        match f() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Write;

    /// 呼ばれた回数を数え、`results` を順に返す処理。
    fn scripted(results: Vec<io::Result<()>>, calls: &Cell<u32>) -> impl FnMut() -> io::Result<()> {
        let mut it = results.into_iter();
        move || {
            calls.set(calls.get() + 1);
            it.next().expect("想定より多く呼ばれた")
        }
    }

    fn os_err(kind: io::ErrorKind) -> io::Result<()> {
        Err(io::Error::from(kind))
    }

    /// F_FULLFSYNC が成功すれば fsync は呼ばない。
    #[test]
    fn full_sync_success_does_not_fall_back() {
        let (f, p) = (Cell::new(0), Cell::new(0));
        let r = full_sync_with_fallback(scripted(vec![Ok(())], &f), scripted(vec![], &p));
        assert_eq!(r.unwrap(), SyncLevel::Full);
        assert_eq!((f.get(), p.get()), (1, 0));
    }

    /// 再現（指摘 F19）: F_FULLFSYNC に対応しないファイルシステム（SMB など）のエラーで、std は
    /// そのまま失敗した。fsync に戻して成功とし、戻したことを返す。
    #[test]
    fn unsupported_full_sync_falls_back_to_fsync() {
        for e in [
            io::ErrorKind::Unsupported,
            io::ErrorKind::InvalidInput,
            io::ErrorKind::Other,
            io::ErrorKind::PermissionDenied,
        ] {
            let (f, p) = (Cell::new(0), Cell::new(0));
            let r =
                full_sync_with_fallback(scripted(vec![os_err(e)], &f), scripted(vec![Ok(())], &p));
            assert_eq!(r.unwrap(), SyncLevel::Fallback, "{e:?}");
            assert_eq!((f.get(), p.get()), (1, 1), "{e:?}");
        }
    }

    /// fsync も失敗したら、fsync のエラーを返す（失敗を隠さない）。
    #[test]
    fn fsync_failure_after_fallback_is_an_error() {
        let (f, p) = (Cell::new(0), Cell::new(0));
        let r = full_sync_with_fallback(
            scripted(vec![os_err(io::ErrorKind::Unsupported)], &f),
            scripted(vec![os_err(io::ErrorKind::StorageFull)], &p),
        );
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::StorageFull);
        assert_eq!((f.get(), p.get()), (1, 1));
    }

    /// EINTR はどちらでもやり直す（std の cvt_r と同じ）。
    #[test]
    fn interrupted_calls_are_retried() {
        let (f, p) = (Cell::new(0), Cell::new(0));
        let r = full_sync_with_fallback(
            scripted(vec![os_err(io::ErrorKind::Interrupted), Ok(())], &f),
            scripted(vec![], &p),
        );
        assert_eq!(r.unwrap(), SyncLevel::Full);
        assert_eq!((f.get(), p.get()), (2, 0));

        let (f, p) = (Cell::new(0), Cell::new(0));
        let r = full_sync_with_fallback(
            scripted(vec![os_err(io::ErrorKind::Unsupported)], &f),
            scripted(
                vec![
                    os_err(io::ErrorKind::Interrupted),
                    os_err(io::ErrorKind::Interrupted),
                    Ok(()),
                ],
                &p,
            ),
        );
        assert_eq!(r.unwrap(), SyncLevel::Fallback);
        assert_eq!((f.get(), p.get()), (1, 3));
    }

    /// 実際のファイルで呼べる（この OS の経路。Apple の経路は CI の macOS で通る）。
    #[test]
    fn real_files_can_be_synced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"genzo").unwrap();
        assert!(sync_file_full(&f).is_ok());
        sync_file_fast(&f).unwrap();
        drop(f);
        assert_eq!(std::fs::read(&path).unwrap(), b"genzo");
    }
}
