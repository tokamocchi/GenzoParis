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

/// 絶対パスにして、`.` と `..` を字句的に取り除く（シンボリックリンクは解決しない）。
pub(crate) fn absolute_lexical(path: &Path) -> Result<PathBuf, ApiError> {
    let abs = std::path::absolute(path).map_err(|e| ApiError::io(path, e))?;
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
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
