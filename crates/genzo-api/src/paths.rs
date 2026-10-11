//! パスとボリュームの対応（04 の 3.3 節「ファイルの場所は『ボリューム ＋ ボリューム内の相対パス』で
//! 管理する」）。
//!
//! **設計からの逸脱（仮置き）**: OS のボリューム ID の取得（外付けドライブの識別。FILE-03、v1）はまだ
//! ない。MVP では、パスの先頭（Unix の `/`、Windows のドライブ `C:\` や UNC の `\\server\share\`）を
//! ボリュームとし、その文字列から ID（`path:/`、`path:C:\` など）を作る。ドライブ文字やマウント先が
//! 変わると別のボリュームになる（FILE-03 で OS のボリューム ID に置き換える）。ドライブ文字は大文字に、
//! UNC のサーバー名・共有名は小文字にそろえる（書き方の大文字・小文字の違いでは分かれない）が、同じ共有を
//! 別の名前（FQDN と短い名前、IP アドレス、割り当てたドライブ `Z:` と UNC）で指定すると別のボリュームになる。

use std::path::{Component, Path, PathBuf, Prefix};

use crate::error::ApiError;

/// ボリュームの ID の接頭辞（OS のボリューム ID ではなく、パスの先頭から作ったものの印）。
pub(crate) const PATH_VOLUME_PREFIX: &str = "path:";

/// ボリュームとボリューム内の相対パスに分けたパス。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VolumePath {
    /// ボリュームの ID（`volume.uuid`）。
    pub uuid: String,
    /// マウント先（`volume.last_mount_path`）。
    pub mount: PathBuf,
    /// ボリューム内の相対パス（'/' 区切り。ルートは空文字列）。
    pub rel: String,
}

/// 絶対パスにして、`.` と `..` を取り除く（`..` の直前のシンボリックリンクを除き、リンクは解決しない）。
///
/// `..` は、**OS が開くときと同じ意味**にする。直前の部分が実在するシンボリックリンクなら、字面で
/// その親へ戻らず、リンクを解決してからリンク先の親へ進む（`/photos/link` が `/mnt/library/sub` を
/// 指すとき、`/photos/link/../target` は `/mnt/library/target`。字面で畳むと `/photos/target` という
/// 別のフォルダを取り込み・書き出しの先にしてしまう）。直前がリンクでない部分（存在しない部分を含む）は、
/// これまでどおり字面で畳む。リンクの先がなく解決できないときは、別のフォルダを読み書きしないよう、
/// 字面で畳まずにエラーにする。`..` を含まないパスの結果は変わらない。
///
/// Windows の `std::path::absolute` は、Win32 の規則（`..` は字面で畳む）で `..` を取り除くので、
/// ここに `..` が残るのは、`\\?\` の形のパスだけ。
pub(crate) fn absolute_lexical(path: &Path) -> Result<PathBuf, ApiError> {
    let abs = std::path::absolute(path).map_err(|e| ApiError::io(path, e))?;
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if is_symlink(&out)? {
                    out = std::fs::canonicalize(&out).map_err(|e| ApiError::FileAccess {
                        path: path.to_path_buf(),
                        message: format!(
                            "シンボリックリンク {} の先を解決できないため、その後の `..` を解釈できません: {e}",
                            out.display()
                        ),
                    })?;
                }
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

/// `path` がシンボリックリンク（Windows ではジャンクションも）か。存在しなければ `false`（字面のまま扱う）。
/// 存在するかを調べられない（権限がないなど）ときはエラー（字面で畳むと、別のフォルダになりうるため）。
fn is_symlink(path: &Path) -> Result<bool, ApiError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => Ok(m.file_type().is_symlink()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(ApiError::io(path, e)),
    }
}

/// 絶対パスを、ボリュームとボリューム内の相対パスに分ける。
pub(crate) fn split_volume(abs: &Path) -> Result<VolumePath, ApiError> {
    let mut mount = String::new();
    let mut parts: Vec<&str> = Vec::new();
    for c in abs.components() {
        match c {
            Component::Prefix(p) => mount.push_str(&prefix_key(p.kind(), p.as_os_str())?),
            Component::RootDir => mount.push(std::path::MAIN_SEPARATOR),
            Component::Normal(s) => parts.push(s.to_str().ok_or_else(|| {
                ApiError::InvalidArgument(format!(
                    "UTF-8 で表せないパスは扱えません: {}",
                    abs.display()
                ))
            })?),
            Component::CurDir | Component::ParentDir => {
                return Err(ApiError::InvalidArgument(format!(
                    "正規化されていないパスです: {}",
                    abs.display()
                )));
            }
        }
    }
    if mount.is_empty() {
        return Err(ApiError::InvalidArgument(format!(
            "絶対パスではありません: {}",
            abs.display()
        )));
    }
    let rel = genzo_catalog::normalize_rel_path(&parts.join("/"))?;
    Ok(VolumePath {
        uuid: format!("{PATH_VOLUME_PREFIX}{mount}"),
        mount: PathBuf::from(mount),
        rel,
    })
}

/// Windows のパスの先頭を、ボリュームのキーにする（ドライブ文字は大文字、UNC のサーバー名・共有名は小文字
/// （SMB では大文字・小文字を区別しないため。書き方の違いで別のボリュームにしない）、`\\?\` の形は普通の形に）。
fn prefix_key(kind: Prefix<'_>, raw: &std::ffi::OsStr) -> Result<String, ApiError> {
    let s = |o: &std::ffi::OsStr| -> Result<String, ApiError> {
        o.to_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::InvalidArgument("UTF-8 で表せないパスは扱えません".to_owned()))
    };
    Ok(match kind {
        Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
            format!("{}:", char::from(d).to_ascii_uppercase())
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            format!(
                r"\\{}\{}",
                s(server)?.to_lowercase(),
                s(share)?.to_lowercase()
            )
        }
        _ => s(raw)?,
    })
}

/// ボリュームのマウント先と相対パス・名前から、絶対パスを作る。
pub(crate) fn join_rel(mount: &Path, rel_dir: &str, name: &str) -> PathBuf {
    let mut p = mount.to_path_buf();
    for part in rel_dir.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    if !name.is_empty() {
        p.push(name);
    }
    p
}

/// ファイル名に使えない文字（Windows で使えない文字と制御文字）を `_` に置き換える。
pub(crate) fn sanitize_file_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "_".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_absolute_removes_dots() {
        let cwd = std::env::current_dir().unwrap();
        let p = absolute_lexical(Path::new("a/./b/../c")).unwrap();
        assert_eq!(p, cwd.join("a").join("c"));
    }

    /// シンボリックリンクの後の `..` の確認用のフォルダ（`tmp` の下）。
    ///
    /// - `A/sub`・`A/target`: 実際のフォルダ。
    /// - `L`: `A/sub` を指すリンク。
    /// - `target`: リンクの親（`tmp`）の下にある、別のフォルダ（字面で畳むと、こちらになってしまう）。
    #[cfg(unix)]
    struct LinkedDirs {
        tmp: tempfile::TempDir,
    }

    #[cfg(unix)]
    impl LinkedDirs {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let a = tmp.path().join("A");
            std::fs::create_dir_all(a.join("sub")).unwrap();
            std::fs::create_dir_all(a.join("target")).unwrap();
            std::fs::create_dir_all(tmp.path().join("target")).unwrap();
            std::os::unix::fs::symlink(a.join("sub"), tmp.path().join("L")).unwrap();
            Self { tmp }
        }

        fn path(&self) -> &Path {
            self.tmp.path()
        }
    }

    /// シンボリックリンクの後の `..` は、OS と同じく、リンク先の親へ進む（字面で `L` の親へ戻らない。
    /// `/photos/link` が `/mnt/library/sub` を指すとき、`/photos/link/../target` は
    /// `/mnt/library/target`。取り込み・書き出しが別のフォルダを読み書きしないように）。
    #[cfg(unix)]
    #[test]
    fn parent_dir_after_a_symlink_follows_the_link_target_like_the_os() {
        let d = LinkedDirs::new();
        let given = d.path().join("L").join("..").join("target");
        let got = absolute_lexical(&given).unwrap();
        // OS の解釈（A/target）と同じフォルダになる。リンクの親の target ではない。
        assert!(
            same_file::is_same_file(&got, &given).unwrap(),
            "OS の解釈と違う: {got:?}"
        );
        assert!(same_file::is_same_file(&got, d.path().join("A").join("target")).unwrap());
        assert!(!same_file::is_same_file(&got, d.path().join("target")).unwrap());
        // `..` が 2 つ続いても同じ（1 つ目でリンク先の中へ進み、2 つ目でその親へ進む）。
        std::fs::create_dir_all(d.path().join("A").join("sub").join("b")).unwrap();
        let given = d.path().join("L").join("b").join("..").join("..");
        let got = absolute_lexical(&given).unwrap();
        assert!(same_file::is_same_file(&got, d.path().join("A")).unwrap());
    }

    /// リンクの後の `..` より先に、存在しない部分があっても、存在する部分は OS の解釈で解決し、存在しない
    /// 部分だけ字面のまま畳む。
    #[cfg(unix)]
    #[test]
    fn nonexistent_parts_after_a_symlink_are_folded_lexically() {
        let d = LinkedDirs::new();
        let given = d
            .path()
            .join("L")
            .join("..")
            .join("not-yet")
            .join("..")
            .join("x");
        let got = absolute_lexical(&given).unwrap();
        let a = std::fs::canonicalize(d.path().join("A")).unwrap();
        assert_eq!(got, a.join("x"));
    }

    /// リンクの先がなく解決できないときは、字面で畳まず、エラーにする（別のフォルダを読み書きしない側に
    /// 倒す）。
    #[cfg(unix)]
    #[test]
    fn parent_dir_after_a_dangling_symlink_is_an_error() {
        let d = LinkedDirs::new();
        std::os::unix::fs::symlink(d.path().join("missing"), d.path().join("D")).unwrap();
        let given = d.path().join("D").join("..").join("target");
        assert!(absolute_lexical(&given).is_err());
        // `..` が続かなければ、リンクが切れていてもそのまま（従来どおり。存在の確認は呼び出し側）。
        assert_eq!(
            absolute_lexical(&d.path().join("D")).unwrap(),
            d.path().join("D")
        );
    }

    /// リンクでないフォルダの後の `..` は、従来どおり字面で畳む（解決しない。表記を変えない）。
    #[cfg(unix)]
    #[test]
    fn parent_dir_after_a_plain_folder_stays_lexical() {
        let d = LinkedDirs::new();
        let given = d.path().join("A").join("sub").join("..").join("target");
        assert_eq!(
            absolute_lexical(&given).unwrap(),
            d.path().join("A").join("target")
        );
        // リンクを経由していても、`..` の直前がリンクでなければ、リンクは解決しない
        // （`L/b/..` は、リンクの先の `A/sub` と同じフォルダで、字面は `L` のまま）。
        std::fs::create_dir_all(d.path().join("A").join("sub").join("b")).unwrap();
        let given = d.path().join("L").join("b").join("..");
        assert_eq!(absolute_lexical(&given).unwrap(), d.path().join("L"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_use_root_as_volume() {
        let v = split_volume(Path::new("/home/user/写真/2024")).unwrap();
        assert_eq!(v.uuid, "path:/");
        assert_eq!(v.mount, PathBuf::from("/"));
        assert_eq!(v.rel, "home/user/写真/2024");
        assert_eq!(
            join_rel(&v.mount, &v.rel, "a.jpg"),
            PathBuf::from("/home/user/写真/2024/a.jpg")
        );
        let root = split_volume(Path::new("/")).unwrap();
        assert_eq!(root.rel, "");
        assert!(split_volume(Path::new("relative/path")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_use_drive_as_volume() {
        let v = split_volume(Path::new(r"c:\Users\me\Pictures")).unwrap();
        assert_eq!(v.uuid, r"path:C:\");
        assert_eq!(v.rel, "Users/me/Pictures");
        let v2 = split_volume(Path::new(r"\\?\C:\Users\me")).unwrap();
        assert_eq!(v2.uuid, v.uuid);
        let unc = split_volume(Path::new(r"\\nas\photos\2024")).unwrap();
        assert_eq!(unc.uuid, r"path:\\nas\photos\");
        assert_eq!(unc.rel, "2024");
        let upper = split_volume(Path::new(r"\\NAS\Photos\2024")).unwrap();
        assert_eq!(upper.uuid, unc.uuid);
        let verbatim = split_volume(Path::new(r"\\?\UNC\Nas\PHOTOS\2024")).unwrap();
        assert_eq!(verbatim.uuid, unc.uuid);
    }

    /// UNC のサーバー名・共有名は大文字・小文字を区別しない（SMB）ので、書き方の違いで別のボリュームに
    /// しない（同じ NAS の写真が二重に登録されないように。指摘 F18）。`Prefix` を直接作るので、Windows
    /// 以外でも確かめられる。
    #[test]
    fn unc_server_and_share_names_ignore_case() {
        use std::ffi::OsStr;
        let a = prefix_key(
            Prefix::UNC(OsStr::new("NAS"), OsStr::new("Photos")),
            OsStr::new(r"\\NAS\Photos"),
        )
        .unwrap();
        let b = prefix_key(
            Prefix::UNC(OsStr::new("nas"), OsStr::new("photos")),
            OsStr::new(r"\\nas\photos"),
        )
        .unwrap();
        let c = prefix_key(
            Prefix::VerbatimUNC(OsStr::new("Nas"), OsStr::new("PHOTOS")),
            OsStr::new(r"\\?\UNC\Nas\PHOTOS"),
        )
        .unwrap();
        assert_eq!(a, r"\\nas\photos");
        assert_eq!(a, b);
        assert_eq!(a, c);
        // ドライブ文字は大文字にそろえる（従来どおり）。
        let d = prefix_key(Prefix::Disk(b'c'), OsStr::new("c:")).unwrap();
        let e = prefix_key(Prefix::VerbatimDisk(b'C'), OsStr::new(r"\\?\C:")).unwrap();
        assert_eq!(d, "C:");
        assert_eq!(d, e);
    }

    #[test]
    fn file_components_are_sanitized() {
        assert_eq!(sanitize_file_component("白黒: 強め?"), "白黒_ 強め_");
        assert_eq!(sanitize_file_component("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_file_component("  ...  "), "_");
        assert_eq!(sanitize_file_component("copy."), "copy");
    }
}
