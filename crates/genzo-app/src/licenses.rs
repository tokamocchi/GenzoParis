//! 配布物に同梱したライセンスの文書を、画面から読む（DIST-05。docs/third_party.md の 1.4 節）。
//!
//! 文書は、Tauri の資源のフォルダ（Windows はインストール先、macOS は `.app` の `Contents/Resources`）の
//! `licenses` の下にある。アプリ自体のライセンス（`GenzoParis/LICENSE.txt`）、第三者のライセンスの文書
//! （`third-party/rust.txt`・`third-party/npm.txt`。scripts/third-party.mjs が作る）、案内（`README.txt`）は
//! `tauri.conf.json` の `bundle.resources` で、LibRaw の文書（`LibRaw/`）は配布のビルドで足す
//! （scripts/lib/worker.mjs）。
//!
//! 画面には、このフォルダの中のファイルの一覧（[`list_documents`]）を返し、一覧にあるファイルだけを読む
//! （[`read_document`]。画面から任意のパスを読ませない）。

use std::io;
use std::path::{Path, PathBuf};

use genzo_api::{ErrorInfo, ErrorKind};
use serde::Serialize;

use crate::state::error;

/// 資源のフォルダの中の、ライセンスの文書のフォルダの名前。
pub const LICENSES_DIR: &str = "licenses";
/// 読む文書の大きさの上限（バイト。rust.txt は約 0.5 MB。**仮置き**）。
pub const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
/// たどるフォルダの深さの上限（LibRaw のビルドの手順の写しが `LibRaw/build-recipe/<port>/` にある）。
const MAX_DEPTH: usize = 6;
/// 一覧に載せるファイルの数の上限（**仮置き**）。
const MAX_FILES: usize = 1000;

/// ライセンスの文書（1 つのファイル）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LicenseDocument {
    /// ライセンスの文書のフォルダからの相対のパス（区切りは `/`）。
    pub path: String,
    /// 大きさ（バイト）。
    pub size: u64,
}

/// ライセンスの文書の一覧。
#[derive(Debug, Clone, Serialize)]
pub struct LicenseDocuments {
    /// ライセンスの文書のフォルダ（なければ `None`。開発用のビルドで資源を置いていないときなど）。
    pub dir: Option<PathBuf>,
    /// 文書（パスの順）。
    pub documents: Vec<LicenseDocument>,
}

/// フォルダの中のファイルを、下のフォルダまでたどって一覧にする（シンボリックリンクはたどらない。パスの順）。
pub fn list_documents(dir: &Path) -> io::Result<Vec<LicenseDocument>> {
    let mut out = Vec::new();
    walk(dir, "", 0, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<LicenseDocument>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        if out.len() >= MAX_FILES {
            break;
        }
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let meta = std::fs::symlink_metadata(entry.path())?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if meta.is_dir() {
            if depth + 1 < MAX_DEPTH {
                walk(&entry.path(), &path, depth + 1, out)?;
            }
        } else if meta.is_file() {
            out.push(LicenseDocument {
                path,
                size: meta.len(),
            });
        }
    }
    Ok(())
}

/// 一覧にある文書を読む（UTF-8 でないところは置き換える）。一覧にないパス・大きすぎるファイルはエラー。
pub fn read_document(dir: &Path, path: &str) -> Result<String, ErrorInfo> {
    let documents = list_documents(dir).map_err(|e| {
        error(
            ErrorKind::FileAccess,
            format!(
                "ライセンスの文書のフォルダ {} を読めません: {e}",
                dir.display()
            ),
            None,
        )
    })?;
    let Some(doc) = documents.iter().find(|d| d.path == path) else {
        return Err(error(
            ErrorKind::NotFound,
            format!("ライセンスの文書 {path} がありません"),
            None,
        ));
    };
    if doc.size > MAX_DOCUMENT_BYTES {
        return Err(error(
            ErrorKind::InvalidInput,
            format!(
                "ライセンスの文書 {path} が大きすぎます（{} バイト）。フォルダを開いて読んでください",
                doc.size
            ),
            None,
        ));
    }
    let file = path
        .split('/')
        .fold(dir.to_path_buf(), |p, part| p.join(part));
    let bytes = std::fs::read(&file).map_err(|e| {
        error(
            ErrorKind::FileAccess,
            format!("ライセンスの文書 {} を読めません: {e}", file.display()),
            None,
        )
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("third-party")).unwrap();
        std::fs::create_dir_all(root.join("LibRaw/build-recipe/vcpkg-ports-libraw")).unwrap();
        std::fs::write(root.join("README.txt"), "案内").unwrap();
        std::fs::write(root.join("third-party/rust.txt"), "Rust の crate").unwrap();
        std::fs::write(root.join("LibRaw/COPYRIGHT"), b"(C) LibRaw \xff").unwrap();
        std::fs::write(
            root.join("LibRaw/build-recipe/vcpkg-ports-libraw/portfile.cmake"),
            "vcpkg",
        )
        .unwrap();
        d
    }

    #[test]
    fn lists_files_in_subfolders_sorted_by_path() {
        let d = tree();
        let docs = list_documents(d.path()).unwrap();
        let paths: Vec<_> = docs.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "LibRaw/COPYRIGHT",
                "LibRaw/build-recipe/vcpkg-ports-libraw/portfile.cmake",
                "README.txt",
                "third-party/rust.txt",
            ]
        );
        assert_eq!(docs[2].size, "案内".len() as u64);
    }

    #[test]
    fn reads_only_listed_documents() {
        let d = tree();
        assert_eq!(
            read_document(d.path(), "third-party/rust.txt").unwrap(),
            "Rust の crate"
        );
        // UTF-8 でないところは置き換える。
        assert_eq!(
            read_document(d.path(), "LibRaw/COPYRIGHT").unwrap(),
            "(C) LibRaw \u{fffd}"
        );
        for bad in [
            "../secret.txt",
            "third-party/../README.txt",
            "/etc/passwd",
            "third-party",
            "none.txt",
            "",
        ] {
            let e = read_document(d.path(), bad).unwrap_err();
            assert_eq!(e.kind, ErrorKind::NotFound, "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks() {
        let d = tree();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "外").unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            d.path().join("secret.txt"),
        )
        .unwrap();
        let docs = list_documents(d.path()).unwrap();
        assert!(docs.iter().all(|d| !d.path.contains("secret")));
        assert!(read_document(d.path(), "link/secret.txt").is_err());
    }

    #[test]
    fn missing_folder_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let e = read_document(&d.path().join("none"), "README.txt").unwrap_err();
        assert_eq!(e.kind, ErrorKind::FileAccess);
    }
}
