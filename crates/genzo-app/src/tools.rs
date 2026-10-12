//! 外部の実行ファイル（ffmpeg / ffprobe）の場所を探す。
//!
//! FFmpeg はアプリに同梱しない（決定事項ログ No.60。ライセンスの構成と特許の確認が済んでいないため）。
//! 利用者が入れた ffmpeg・ffprobe を、次の順に探す。見つけたものはワーカーに環境変数
//! `GENZO_FFMPEG`・`GENZO_FFPROBE` で渡す（genzo-media の `video`）。
//!
//! 1. 設定で指定したフォルダ（[`crate::settings::AppSettings::ffmpeg_dir`]）
//! 2. 環境変数 `PATH`
//! 3. OS ごとの既定の場所（macOS の Finder から起動したアプリには、ターミナルの `PATH`
//!    （Homebrew の `/opt/homebrew/bin` など）が渡らないため）
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
        dirs.push(PathBuf::from(r"C:\ffmpeg\bin"));
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
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = configured {
        candidates.push(dir.join(&name));
    }
    if let Some(p) = path_var {
        candidates.extend(std::env::split_paths(&p).map(|d| d.join(&name)));
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
