//! 外部の実行ファイル（ffmpeg / ffprobe）の場所を探す。
//!
//! FFmpeg はアプリに同梱しない（決定事項ログ No.62。ライセンスの構成と特許の確認が済んでいないため）。
//! 利用者が入れた ffmpeg・ffprobe を、次の順に探す。見つけたものはワーカーに環境変数
//! `GENZO_FFMPEG`・`GENZO_FFPROBE` で渡す（genzo-media の `video`）。
//!
//! 1. 設定で指定したフォルダ（[`crate::settings::AppSettings::ffmpeg_dir`]）。指定したときは**ここだけ**を
//!    探す（なければ見つからないとして画面に出す。指定したつもりのないものを使わないため）
//! 2. 環境変数 `PATH`（絶対パスの項目だけ。`.` などの相対パスは、今のフォルダによって変わるので使わない）
//! 3. OS ごとの既定の場所（macOS の Finder から起動したアプリには、ターミナルの `PATH`
//!    （Homebrew の `/opt/homebrew/bin` など）が渡らないため）。Windows は、本人のフォルダ（winget の
//!    リンク）と、管理者しか書けない `Program Files` だけにする。`C:\ffmpeg\bin` のような、ほかの利用者も
//!    作れる・書ける場所は探さない（ほかの利用者が置いた実行ファイルを、この利用者の権限で動かさないため。
//!    CWE-427）
//!
//! 見つからなければ動画の登録はできるが、サムネイルとメタデータ（長さ・fps など）が取れない。画面で
//! 案内する。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// 見つけた外部の実行ファイル。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ExternalTools {
    /// ffmpeg。
    pub ffmpeg: Option<PathBuf>,
    /// ffprobe。
    pub ffprobe: Option<PathBuf>,
}

impl ExternalTools {
    /// ワーカーに渡す環境変数。
    pub fn worker_env(&self) -> Vec<(OsString, OsString)> {
        let mut env = Vec::new();
        if let Some(p) = &self.ffmpeg {
            env.push((OsString::from("GENZO_FFMPEG"), p.clone().into_os_string()));
        }
        if let Some(p) = &self.ffprobe {
            env.push((OsString::from("GENZO_FFPROBE"), p.clone().into_os_string()));
        }
        env
    }
}

fn exe_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

/// OS ごとの既定の場所。
fn platform_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(target_os = "macos") {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"] {
            dirs.push(PathBuf::from(d));
        }
    } else if cfg!(windows) {
        // winget はリンクを LOCALAPPDATA の WinGet\Links に置く。
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(
                Path::new(&local)
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links"),
            );
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            dirs.push(Path::new(&pf).join("ffmpeg").join("bin"));
        }
    } else {
        for d in ["/usr/bin", "/usr/local/bin"] {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs
}

/// `stem`（ffmpeg / ffprobe）を探す。
fn find_in(stem: &str, configured: Option<&Path>, path_var: Option<OsString>) -> Option<PathBuf> {
    let name = exe_name(stem);
    if let Some(dir) = configured {
        let c = dir.join(&name);
        return c.is_file().then_some(c);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = path_var {
        candidates.extend(
            std::env::split_paths(&p)
                .filter(|d| d.is_absolute())
                .map(|d| d.join(&name)),
        );
    }
    candidates.extend(platform_dirs().into_iter().map(|d| d.join(&name)));
    candidates.into_iter().find(|c| c.is_file())
}

/// ffmpeg・ffprobe を探す。
pub fn locate(configured_dir: Option<&Path>) -> ExternalTools {
    ExternalTools {
        ffmpeg: find_in("ffmpeg", configured_dir, std::env::var_os("PATH")),
        ffprobe: find_in("ffprobe", configured_dir, std::env::var_os("PATH")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_dir_wins_over_path() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for d in [a.path(), b.path()] {
            std::fs::write(d.join(exe_name("ffprobe")), b"").unwrap();
        }
        let path_var = std::env::join_paths([b.path()]).unwrap();
        assert_eq!(
            find_in("ffprobe", Some(a.path()), Some(path_var.clone())),
            Some(a.path().join(exe_name("ffprobe")))
        );
        assert_eq!(
            find_in("ffprobe", None, Some(path_var)),
            Some(b.path().join(exe_name("ffprobe")))
        );
    }

    /// 設定でフォルダを指定したら、そこになくても PATH・既定の場所へ進まない。PATH の相対パスは使わない。
    #[test]
    fn configured_dir_is_the_only_place_and_relative_path_entries_are_ignored() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join(exe_name("ffprobe")), b"").unwrap();
        let path_var = std::env::join_paths([b.path()]).unwrap();
        assert_eq!(
            find_in("ffprobe", Some(a.path()), Some(path_var)),
            None,
            "指定したフォルダにないのに、PATH のものを使った"
        );
        // 相対パスの項目（今のフォルダからの相対）は候補にしない。テストの今のフォルダ（crate のフォルダ）に
        // ある Cargo.toml を、PATH の `.` から探しても見つけない（Windows では実行ファイルの拡張子が付くので、
        // 名前が合わずに見つからない）。
        assert!(Path::new("Cargo.toml").is_file());
        let dot = std::env::join_paths([Path::new(".")]).unwrap();
        assert_eq!(find_in("Cargo.toml", None, Some(dot)), None);
        assert!(platform_dirs().iter().all(|d| d.is_absolute()));
    }

    #[test]
    fn missing_tool_is_none_and_env_lists_only_found_tools() {
        let a = tempfile::tempdir().unwrap();
        assert_eq!(find_in("no-such-tool-genzo", Some(a.path()), None), None);
        let t = ExternalTools {
            ffmpeg: Some(PathBuf::from("/x/ffmpeg")),
            ffprobe: None,
        };
        let env = t.worker_env();
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, OsString::from("GENZO_FFMPEG"));
    }
}
