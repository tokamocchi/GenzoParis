//! 取り込む前の選択（IMP-02 の残り。implementation_status の「v1 の残り」No.9）: 取り込み元のフォルダの写真・動画を
//! 一覧にし、サムネイルを見て、取り込むファイルを選べるようにする。
//!
//! - [`Core::scan_import_source`] は、フォルダを走査して、取り込める写真・動画の一覧を返す（何も登録しない。走査の
//!   規則は取り込みと同じ: 隠しファイル・リンク・アプリのデータのフォルダを飛ばす。`import` の `scan`）。多すぎる
//!   ときは先頭の [`MAX_IMPORT_SCAN_FILES`] 件までにして、切り詰めたことを知らせる（その場合、選んで取り込む
//!   ことはできず、フォルダの全体を取り込む）。結果はコアが覚える（最後の 1 回分だけ）。
//! - [`Core::import_source_thumbnail`] は、覚えている一覧の 1 件のサムネイル（ワーカーで作る JPEG。RAW は埋め込みの
//!   JPEG、動画は代表フレーム。取り込みのときのサムネイルと同じ作り方）。**一覧の番号で指定する**（パスを
//!   受け取らない。画面からは、走査したフォルダの中の、取り込めるファイルしか読めない）。キャッシュには保存しない
//!   （まだカタログにないファイルなので）。
//! - 選んだファイルだけを取り込むには、取り込みの設定の [`crate::ImportOptions::only`] に、一覧の
//!   [`ImportSourceFile::rel_path`] を渡す（走査し直した結果のうち、渡したものだけを取り込む。なくなっていた
//!   ファイルは、結果の「登録しなかったファイル」に入る）。

use std::path::{Path, PathBuf};

use genzo_jobs::CancellationToken;
use genzo_media::CacheSpec;
use genzo_model::AssetKind;
use genzo_worker::Lane;

use crate::core::Core;
use crate::error::ApiError;
use crate::import::{excluded_dirs, scan};
use crate::paths::absolute_lexical;
use crate::types::{ImportSourceFile, ImportSourceScan};

/// 一覧に入れるファイルの数の上限（**仮置き**: 5,000。応答の大きさと画面の一覧の重さを抑える）。
pub const MAX_IMPORT_SCAN_FILES: usize = 5_000;

/// 覚えている走査の結果（サムネイルを番号で引くため）。
#[derive(Debug)]
pub(crate) struct StoredImportScan {
    id: u64,
    files: Vec<(PathBuf, AssetKind)>,
}

/// 取り込み元のフォルダからの相対パス（区切りは `/`）。フォルダの外・UTF-8 で表せなければ `None`。
pub(crate) fn rel_path_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(s) => parts.push(s.to_str()?),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl Core {
    /// 取り込み元のフォルダを走査して、取り込める写真・動画の一覧を返す（何も登録しない。モジュールの doc）。
    ///
    /// エラー: フォルダを読めなければ [`ApiError::FileAccess`]、フォルダでない・アプリのデータのフォルダなら
    /// [`ApiError::InvalidArgument`]。
    pub fn scan_import_source(
        &self,
        path: &Path,
        recursive: bool,
    ) -> Result<ImportSourceScan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let root = absolute_lexical(path)?;
        let meta = std::fs::metadata(&root).map_err(|e| ApiError::FileAccess {
            path: root.clone(),
            message: e.to_string(),
        })?;
        if !meta.is_dir() {
            return Err(ApiError::InvalidArgument(format!(
                "フォルダではありません: {}",
                root.display()
            )));
        }
        let excluded = excluded_dirs(inner);
        if excluded.contains(&root, std::fs::canonicalize(&root).ok().as_deref()) {
            return Err(ApiError::InvalidArgument(format!(
                "アプリのデータのフォルダ（プレビューのキャッシュ・バックアップ）は取り込めません: {}",
                root.display()
            )));
        }
        // 走査の相対パスは使わない（ここでは取り込み元のフォルダからの相対パスを求め直す）。
        let scanned = scan(&root, "", recursive, &excluded, &CancellationToken::new());
        let total = scanned.files.len() as u64;
        let mut files = Vec::new();
        let mut stored = Vec::new();
        for c in scanned.files.into_iter().take(MAX_IMPORT_SCAN_FILES) {
            let Some(rel_path) = rel_path_of(&root, &c.path) else {
                continue;
            };
            let meta = std::fs::metadata(&c.path).ok();
            files.push(ImportSourceFile {
                index: stored.len() as u32,
                rel_path,
                name: c.name,
                kind: c.kind,
                size: meta.as_ref().map(std::fs::Metadata::len),
                modified: meta
                    .and_then(|m| m.modified().ok())
                    .map(chrono::DateTime::<chrono::Utc>::from),
            });
            stored.push((c.path, c.kind));
        }
        let scan_id = {
            let mut slot = inner.import_scan.lock();
            let id = slot.as_ref().map_or(1, |s| s.id + 1);
            *slot = Some(StoredImportScan { id, files: stored });
            id
        };
        Ok(ImportSourceScan {
            scan_id,
            root,
            truncated: total > files.len() as u64,
            total,
            files,
            unreadable: scanned.unreadable.len() as u64,
            skipped_links: scanned.skipped_links,
        })
    }

    /// 覚えている走査の結果（[`Core::scan_import_source`]）の `index` 番目のファイルのサムネイル（JPEG。長辺 320）。
    ///
    /// エラー: 走査の結果が新しいものに変わっている・番号が範囲の外なら [`ApiError::NotFound`]。ファイルを
    /// 読めない・サムネイルを作れなければ、ワーカーのエラー。
    pub fn import_source_thumbnail(&self, scan_id: u64, index: u32) -> Result<Vec<u8>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let (path, kind) = {
            let slot = inner.import_scan.lock();
            slot.as_ref()
                .filter(|s| s.id == scan_id)
                .and_then(|s| s.files.get(index as usize).cloned())
                .ok_or_else(|| {
                    ApiError::NotFound(format!("取り込み元の一覧 {scan_id} の {index} 番目"))
                })?
        };
        let pool = inner.pool()?;
        let token = CancellationToken::new();
        let thumb = match kind {
            AssetKind::Photo => {
                pool.thumbnail(Lane::Batch, &path, CacheSpec::L0_THUMBNAIL, &token)?
            }
            AssetKind::Video => {
                pool.video_thumbnail(Lane::Batch, &path, CacheSpec::L0_THUMBNAIL, &token)?
            }
        };
        Ok(thumb.jpeg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_use_forward_slashes_and_stay_inside_the_root() {
        let root = Path::new("/photos/card");
        assert_eq!(
            rel_path_of(root, &root.join("DCIM").join("100MSDCF").join("A.ARW")).as_deref(),
            Some("DCIM/100MSDCF/A.ARW")
        );
        assert_eq!(
            rel_path_of(root, &root.join("B.jpg")).as_deref(),
            Some("B.jpg")
        );
        assert_eq!(rel_path_of(root, root), None);
        assert_eq!(rel_path_of(root, Path::new("/photos/other/C.jpg")), None);
        assert_eq!(rel_path_of(root, &root.join("..").join("C.jpg")), None);
    }
}
