//! カタログの排他（1 つのカタログは 1 つのアプリだけが開く。Lightroom と同様。implementation_status の
//! No.29）。
//!
//! - カタログの隣にロックファイル（`<カタログのファイル名>.lock`）を置き、OS のファイルロック
//!   （genzo-media の `file_lock`。Unix は `flock`、Windows は共有しないで開く。標準ライブラリの
//!   `File::try_lock` は Rust 1.89 からのため使わない）で排他にする。既に開かれていれば
//!   [`ApiError::CatalogInUse`] を返す。
//! - プロセスが異常終了しても、ロックは OS が解放する（ロックファイル自体は残るが、害はない。削除すると、
//!   別のプロセスが同時に開いたロックファイルと食い違うので削除しない）。
//! - ロックを持つのは [`crate::Core`] を開いている間と、復元の差し替え（[`crate::apply_restore`]）の間。
//! - 読み取り専用で開くカタログの接続（詳細チェックの読み取り用）は、同じプロセスがロックを持っている間
//!   だけ使うので、ロックを取らない。外部のツールが読み取り専用で開くことは防がない。
//! - ファイルロックが使えないファイルシステム（一部のネットワークのボリューム）では、排他にできないことを
//!   警告して続ける（開けなくするより、作業を続けられることを優先する。呼び出し側が警告を出す）。

use std::path::{Path, PathBuf};

use genzo_media::file_lock::{LockedFile, TryLock, try_lock_exclusive};

use crate::error::ApiError;

/// カタログのロックファイルのパス（`<カタログ>.lock`）。
pub(crate) fn lock_path(catalog: &Path) -> PathBuf {
    let mut s = catalog.as_os_str().to_owned();
    s.push(".lock");
    PathBuf::from(s)
}

/// 取ったロック（drop で解放する）。
#[derive(Debug)]
pub(crate) struct CatalogLock {
    /// ロックしているファイル（ロックを取れないファイルシステムでは `None`）。
    _file: Option<LockedFile>,
}

/// ロックを取った結果。
pub(crate) enum Acquired {
    /// 取れた。
    Locked(CatalogLock),
    /// ファイルロックが使えない（排他にできない）。理由。
    Unsupported(CatalogLock, String),
}

/// ロックが取れないときに試し直す回数と間隔（**仮置き**: 20 回 × 50 ms ≒ 1 秒）。
///
/// Unix の `flock` のロックは、開いたファイルの記述を共有する複製が残っている間は外れない。閉じた直後に、
/// 同じプロセスの別のスレッドが子プロセス（ワーカーなど）を起動していると、子プロセスが `exec` するまでの
/// 短い間だけ複製が残り、開き直しが「使用中」になることがある（テストで再現）。そのため少しだけ待つ。
const LOCK_RETRIES: u32 = 20;
const LOCK_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// カタログのロックを取る。既に別の [`crate::Core`]（同じプロセスを含む）が開いていれば
/// [`ApiError::CatalogInUse`]（少しだけ待ってから返す。[`LOCK_RETRIES`]）。
pub(crate) fn acquire(catalog: &Path) -> Result<Acquired, ApiError> {
    let path = lock_path(catalog);
    let mut tries = 0;
    loop {
        match try_lock_exclusive(&path, true).map_err(|e| ApiError::io(&path, e))? {
            TryLock::Locked(f) => return Ok(Acquired::Locked(CatalogLock { _file: Some(f) })),
            TryLock::WouldBlock => {
                tries += 1;
                if tries >= LOCK_RETRIES {
                    return Err(ApiError::CatalogInUse {
                        path: catalog.to_path_buf(),
                    });
                }
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            TryLock::Unsupported(e) => {
                return Ok(Acquired::Unsupported(
                    CatalogLock { _file: None },
                    format!("{}: {e}", path.display()),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_fails_until_the_first_is_dropped() {
        let d = tempfile::tempdir().unwrap();
        let cat = d.path().join("catalog.db");
        let first = acquire(&cat).unwrap();
        assert!(matches!(first, Acquired::Locked(_)));
        assert!(matches!(acquire(&cat), Err(ApiError::CatalogInUse { .. })));
        drop(first);
        assert!(matches!(acquire(&cat).unwrap(), Acquired::Locked(_)));
        assert_eq!(lock_path(&cat), d.path().join("catalog.db.lock"));
    }
}
