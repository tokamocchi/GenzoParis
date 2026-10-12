//! カタログのファイルの扱い（既定の場所・データのフォルダ・GenzoParis のカタログかの判定）。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// 既定のカタログのファイル名。
pub const DEFAULT_CATALOG_FILE: &str = "catalog.db";

/// 既定のカタログを置くフォルダの名前（ピクチャのフォルダの下）。
pub const DEFAULT_CATALOG_DIR: &str = "GenzoParis";

/// データのフォルダ（サムネイル・プレビューのキャッシュ・自動バックアップ）。カタログの隣の
/// `<カタログ名>-data`（CLI の `genzo` と同じ。同じカタログを CLI とアプリで使える）。
pub fn data_dir_for(catalog: &Path) -> PathBuf {
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

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// ファイルの状態（カタログを開く前の確認）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFileState {
    /// ない（新しく作れる）。
    Missing,
    /// GenzoParis のカタログ。
    Catalog,
    /// カタログではないファイル、またはフォルダ（開かない・上書きしない）。
    NotCatalog,
}

/// SQLite の WAL のファイル（`<カタログ>-wal`）。
fn wal_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push("-wal");
    PathBuf::from(s)
}

/// パスの状態を確かめる。
///
/// 1. SQLite の先頭（16 バイト）でなければカタログではない（SQLite で開かない。カタログでないファイルに
///    触れない）。
/// 2. オフセット 68 の application_id が GenzoParis のカタログのものならカタログ。
/// 3. 違っていても、`-wal` があれば、SQLite に読み取り専用で開かせて確かめる。カタログは WAL モードで、
///    開いている間と異常終了の後は、application_id などの変更が `-wal` にあり、本体の先頭は書き戻す
///    （閉じる・チェックポイント）まで古いまま（作ったばかりのカタログでは 0）のため。`-wal` がなければ、
///    本体の先頭が最新なので、そのまま判定する。
/// 4. 読み取り専用で開いて、GenzoParis のカタログ（古い版・新しい版のスキーマを含む）ならカタログ、別の
///    DB・空の DB ならカタログではない。それ以外の失敗（読めないなど）はエラーにする。
pub fn inspect(path: &Path) -> std::io::Result<CatalogFileState> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(CatalogFileState::Missing),
        Err(e) => return Err(e),
    };
    if !meta.is_file() {
        return Ok(CatalogFileState::NotCatalog);
    }
    let mut head = [0u8; 100];
    let mut f = File::open(path)?;
    let mut read = 0;
    while read < head.len() {
        let n = f.read(&mut head[read..])?;
        if n == 0 {
            break;
        }
        read += n;
    }
    if read < head.len() || &head[..16] != SQLITE_HEADER {
        return Ok(CatalogFileState::NotCatalog);
    }
    let id = i32::from_be_bytes([head[68], head[69], head[70], head[71]]);
    if id == genzo_catalog::CATALOG_APPLICATION_ID {
        return Ok(CatalogFileState::Catalog);
    }
    if !wal_path(path).exists() {
        return Ok(CatalogFileState::NotCatalog);
    }
    use genzo_catalog::CatalogError;
    match genzo_catalog::Catalog::open_read_only(path) {
        Ok(_) | Err(CatalogError::FutureSchema { .. }) => Ok(CatalogFileState::Catalog),
        // 版 0 は、テーブルのない空の DB（GenzoParis のカタログとして作り終えていない）。
        Err(CatalogError::OutdatedSchema { found, .. }) if found > 0 => {
            Ok(CatalogFileState::Catalog)
        }
        Err(CatalogError::OutdatedSchema { .. }) | Err(CatalogError::NotOurDatabase(_)) => {
            Ok(CatalogFileState::NotCatalog)
        }
        Err(e) => Err(std::io::Error::other(e)),
    }
}

/// カタログの隣のデータのフォルダ（[`data_dir_for`]）に、GenzoParis のデータ（サムネイルの DB、または
/// このカタログの名前のバックアップ）があるか。
///
/// カタログの先頭が壊れて [`inspect`] が [`CatalogFileState::NotCatalog`] を返したときに、壊れたカタログ
/// （バックアップから復元を案内する）か、カタログでないファイル（開かない・変えない）かを見分けるのに使う。
pub fn has_catalog_data(catalog: &Path) -> bool {
    if !catalog.is_file() {
        return false;
    }
    let config = genzo_api::CoreConfig::new(catalog, data_dir_for(catalog));
    if config.thumbs_path().is_file() {
        return true;
    }
    let stem = catalog
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    genzo_catalog::list_backups_by_stem(&config.backup_dir(), &stem).is_ok_and(|l| !l.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_next_to_the_catalog() {
        assert_eq!(
            data_dir_for(Path::new("/p/GenzoParis/catalog.db")),
            PathBuf::from("/p/GenzoParis/catalog-data")
        );
        assert_eq!(
            data_dir_for(Path::new("main.db")),
            PathBuf::from("./main-data")
        );
    }

    /// 開いている（または異常終了した）カタログは、WAL の変更を本体に書き戻すまで、本体の先頭の
    /// application_id が 0 のまま。それでもカタログと判定する（先頭だけで判定すると、開いているカタログを
    /// 開き直す・別のアプリが開いているカタログ・異常終了したカタログを「カタログではない」としてしまう）。
    #[test]
    fn inspect_reads_the_wal_of_open_catalogs() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("open.db");
        let cat = genzo_catalog::Catalog::open(&path).unwrap();
        assert!(wal_path(&path).exists(), "WAL モード");
        assert_eq!(inspect(&path).unwrap(), CatalogFileState::Catalog);
        // 異常終了の後に残るファイル（本体と -wal）を模す: 開いている間に写し取る。
        let crashed = d.path().join("crashed.db");
        std::fs::copy(&path, &crashed).unwrap();
        std::fs::copy(wal_path(&path), wal_path(&crashed)).unwrap();
        assert_eq!(inspect(&crashed).unwrap(), CatalogFileState::Catalog);
        // 接続を閉じると本体に書き戻すので、先頭だけで判定できる。
        drop(cat);
        assert!(!wal_path(&path).exists());
        assert_eq!(inspect(&path).unwrap(), CatalogFileState::Catalog);
        // GenzoParis の別の DB（サムネイルの DB）は、WAL があってもカタログではない。
        let thumbs = d.path().join("thumbs.db");
        let store = genzo_catalog::ThumbStore::open(&thumbs).unwrap();
        assert_eq!(inspect(&thumbs).unwrap(), CatalogFileState::NotCatalog);
        store.close().unwrap();
        assert_eq!(inspect(&thumbs).unwrap(), CatalogFileState::NotCatalog);
    }

    #[test]
    fn catalog_data_is_detected_next_to_a_damaged_file() {
        let d = tempfile::tempdir().unwrap();
        let broken = d.path().join("壊れた カタログ.db");
        std::fs::write(&broken, b"GARBAGE").unwrap();
        assert!(!has_catalog_data(&broken), "データのフォルダがない");
        let data = data_dir_for(&broken);
        std::fs::create_dir_all(data.join("backups")).unwrap();
        assert!(!has_catalog_data(&broken), "空のデータのフォルダ");
        // 別の名前のカタログのバックアップは数えない。
        std::fs::write(
            data.join("backups")
                .join("other-backup-20261009T010203456Z.db"),
            b"",
        )
        .unwrap();
        assert!(!has_catalog_data(&broken));
        std::fs::write(
            data.join("backups")
                .join("壊れた カタログ-backup-20261009T010203456Z.db"),
            b"",
        )
        .unwrap();
        assert!(
            has_catalog_data(&broken),
            "このカタログの名前のバックアップ"
        );
        let other = d.path().join("other.db");
        std::fs::write(&other, b"x").unwrap();
        std::fs::create_dir_all(data_dir_for(&other)).unwrap();
        std::fs::write(data_dir_for(&other).join("thumbs.db"), b"").unwrap();
        assert!(has_catalog_data(&other), "サムネイルの DB");
        assert!(!has_catalog_data(d.path()), "フォルダはカタログではない");
        assert!(!has_catalog_data(&d.path().join("none.db")), "ないファイル");
    }

    #[test]
    fn inspect_detects_catalogs_and_other_files() {
        let d = tempfile::tempdir().unwrap();
        let missing = d.path().join("none.db");
        assert_eq!(inspect(&missing).unwrap(), CatalogFileState::Missing);
        assert_eq!(inspect(d.path()).unwrap(), CatalogFileState::NotCatalog);
        let photo = d.path().join("a.jpg");
        std::fs::write(&photo, b"\xff\xd8\xff\xe0 not a catalog").unwrap();
        assert_eq!(inspect(&photo).unwrap(), CatalogFileState::NotCatalog);
        let other_db = d.path().join("other.db");
        let mut head = Vec::from(&SQLITE_HEADER[..]);
        head.resize(100, 0);
        std::fs::write(&other_db, &head).unwrap();
        assert_eq!(inspect(&other_db).unwrap(), CatalogFileState::NotCatalog);
        let cat = d.path().join("cat.db");
        head[68..72].copy_from_slice(&genzo_catalog::CATALOG_APPLICATION_ID.to_be_bytes());
        std::fs::write(&cat, &head).unwrap();
        assert_eq!(inspect(&cat).unwrap(), CatalogFileState::Catalog);
    }
}
