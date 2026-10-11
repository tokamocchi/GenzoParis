//! カタログの排他（1 つのカタログは 1 つのアプリだけが開く。Lightroom と同様。implementation_status の
//! No.29）。
//!
//! - カタログの隣にロックファイル（`<カタログのファイル名>.lock`）を置き、OS のファイルロック
//!   （genzo-media の `file_lock`。Unix は `flock`、Windows は共有しないで開く。標準ライブラリの
//!   `File::try_lock` は Rust 1.89 からのため使わない）で排他にする。既に開かれていれば
//!   [`ApiError::CatalogInUse`] を返す。
//! - ロックファイルの名前は、渡されたパスの書き方ではなく、**リンクを解決したカタログのパス**から求める
//!   （[`lock_path`]）。シンボリックリンクの別名で開いても、本体の名前で開いたときと同じロックになる。
//!   **限界**: ハードリンクの別名（同じファイルへの別の名前）は、パスからは同じカタログと分からないので、
//!   同時に開くのを防げない（ファイルの同一性でロックするには、共通の置き場とその掃除が要り、大きな仕組みに
//!   なるため採らない。SQLite も、別名ごとに `-wal`・ジャーナルの名前が分かれるので、ハードリンクの別名の
//!   同時使用は想定していない）。カタログをハードリンクで複数の名前にしないこと。
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

/// ロックファイルの名前を求めるための、カタログの解決したパス。
///
/// 渡されたパスの書き方（シンボリックリンクの別名・リンクの連鎖・`..` を含む書き方）によらず、同じ
/// カタログなら同じパスにする（書き方ごとに `.lock` を付けると別のロックファイルになり、同じカタログを
/// 同時に開けてしまう）。
/// - カタログがあれば、[`std::fs::canonicalize`]（リンクをすべて解決する）。
/// - なければ（初めて作る・リンク切れ）、親フォルダを解決して、渡されたファイル名を付ける。
/// - 解決できなければ、渡されたパスのまま（ロックを開くときのエラーに任せる）。
///
/// Windows の `canonicalize` は `\\?\` で始まる形（`\\?\C:\...`、UNC は `\\?\UNC\...`）を返す。どの書き方も
/// この関数を通して同じ形にそろうので、そのままロックファイルを開くのに使う（ロックファイルを開けない
/// ときの [`ApiError::Io`] の `path` と、ファイルロックが使えない理由の文言には、この形が出る。
/// [`ApiError::CatalogInUse`] には、渡されたパスを使う）。
///
/// **限界**: ハードリンクの別名は、パスからは同じファイルと分からないので、防げない（ファイルの
/// 同一性でロックするには、カタログとは別の共通の置き場とその掃除が要り、大きな仕組みになる）。
/// SQLite も、別名ごとに `-wal`・ジャーナルの名前が分かれるので、ハードリンクの別名の同時使用は
/// 想定していない。リンク切れのシンボリックリンクを、リンク先がまだないうちに開く場合も、別名の
/// 名前のロックになる（リンク先ができた後は、リンク先の名前で解決する）。
fn resolved_catalog_path(catalog: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(catalog) {
        return resolved;
    }
    if let Some(name) = catalog.file_name() {
        let parent = catalog
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if let Ok(parent) = std::fs::canonicalize(parent) {
            return parent.join(name);
        }
    }
    catalog.to_path_buf()
}

/// カタログのロックファイルのパス（`<解決したカタログのパス>.lock`。[`resolved_catalog_path`]）。
pub(crate) fn lock_path(catalog: &Path) -> PathBuf {
    let mut s = resolved_catalog_path(catalog).into_os_string();
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
        // ロックファイルは、リンクを解決したカタログのパスの隣に置く（一時フォルダ自体が
        // シンボリックリンクの下（macOS の /var → /private/var）や短い名前（Windows の 8.3 形式）の
        // ことがあるので、解決した一時フォルダと比べる）。
        assert_eq!(
            lock_path(&cat),
            std::fs::canonicalize(d.path())
                .unwrap()
                .join("catalog.db.lock")
        );
    }

    /// カタログも親フォルダもない（解決できない）ときは、渡されたパスのまま（開くときのエラーに任せる）。
    #[test]
    fn an_unresolvable_path_keeps_its_spelling() {
        let d = tempfile::tempdir().unwrap();
        let cat = d.path().join("missing").join("catalog.db");
        assert_eq!(
            lock_path(&cat),
            d.path().join("missing").join("catalog.db.lock")
        );
        assert!(matches!(acquire(&cat), Err(ApiError::Io { .. })));
    }

    /// 同じカタログの別名（シンボリックリンク）は、同じロックファイルになる（以前は、渡されたパスの
    /// それぞれに `.lock` を付けたので別のロックになり、同じカタログを同時に開けた）。
    #[cfg(unix)]
    #[test]
    fn symlink_aliases_of_one_catalog_share_one_lock_file() {
        use std::os::unix::fs::symlink;

        let d = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(d.path()).unwrap();
        let real_dir = root.join("real");
        let link_dir = root.join("links");
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::create_dir_all(&link_dir).unwrap();
        let real = real_dir.join("catalog.db");
        std::fs::write(&real, b"").unwrap();
        let expected = real_dir.join("catalog.db.lock");

        // ファイルのシンボリックリンク（別のフォルダ・別の名前）、その連鎖、`..` を含む書き方。
        let alias = link_dir.join("alias.db");
        symlink(&real, &alias).unwrap();
        let chain = link_dir.join("chain.db");
        symlink(&alias, &chain).unwrap();
        // 相対パスのリンク。
        let relative = link_dir.join("relative.db");
        symlink("../real/catalog.db", &relative).unwrap();
        let dotted = link_dir.join("..").join("links").join("alias.db");
        for p in [&real, &alias, &chain, &relative, &dotted] {
            assert_eq!(lock_path(p), expected, "{}", p.display());
        }

        // フォルダのシンボリックリンク経由（カタログがまだなくても、親フォルダを解決する）。
        let dir_link = root.join("dir-link");
        symlink(&real_dir, &dir_link).unwrap();
        assert_eq!(lock_path(&dir_link.join("catalog.db")), expected);
        assert_eq!(
            lock_path(&dir_link.join("new.db")),
            real_dir.join("new.db.lock")
        );

        // 実際に排他になる（別名で取ると、本体の名前でも取れない。逆も同じ）。
        let held = acquire(&alias).unwrap();
        assert!(matches!(held, Acquired::Locked(_)));
        for p in [&real, &chain, &relative, &dotted] {
            assert!(
                matches!(acquire(p), Err(ApiError::CatalogInUse { .. })),
                "{}",
                p.display()
            );
        }
        drop(held);
        let held = acquire(&real).unwrap();
        assert!(matches!(
            acquire(&alias),
            Err(ApiError::CatalogInUse { .. })
        ));
        drop(held);
    }
}
