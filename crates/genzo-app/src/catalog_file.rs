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

/// パスの状態を確かめる（SQLite の先頭と、オフセット 68 の application_id で判定する。ファイルを
/// データベースとしては開かない）。
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
    Ok(if id == genzo_catalog::CATALOG_APPLICATION_ID {
        CatalogFileState::Catalog
    } else {
        CatalogFileState::NotCatalog
    })
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
