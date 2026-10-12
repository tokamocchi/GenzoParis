//! アプリの設定（カタログの場所・FFmpeg の場所・GPU の使用など）。
//!
//! カタログに保存する設定（キャッシュの場所と上限・既定の色空間・既定のタイムゾーン。genzo-api の
//! `CoreSettings`）とは別に、カタログを開く前に要るものだけを、OS のアプリの設定のフォルダの
//! `settings.json` に保存する。
//!
//! - 書き込みは一時ファイルに書いてから名前を変える（途中で落ちても前の設定が残る）。
//! - 読めない・壊れている場合は既定値で起動し、壊れたファイルは `settings.json.broken-<時刻>` に退避する
//!   （利用者の設定を黙って捨てない）。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use genzo_api::GpuMode;
use serde::{Deserialize, Serialize};

/// 最近開いたカタログの数の上限。
pub const MAX_RECENT_CATALOGS: usize = 8;

/// 設定のファイル名。
pub const SETTINGS_FILE: &str = "settings.json";

/// アプリの設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// 設定の形式の版（将来の移行用）。
    pub version: u32,
    /// 前回開いたカタログ（起動時に開く）。
    pub last_catalog: Option<PathBuf>,
    /// 最近開いたカタログ（新しい順）。
    pub recent_catalogs: Vec<PathBuf>,
    /// ffmpeg・ffprobe のあるフォルダ（`None` なら PATH と既定の場所を探す。動画のサムネイル・
    /// メタデータに使う。FFmpeg は同梱しない。決定事項ログ No.60）。
    pub ffmpeg_dir: Option<PathBuf>,
    /// GPU の使用。
    pub gpu: GpuMode,
    /// アプリの終了時に、前回からの経過時間によらずカタログをバックアップするか（DATA-04）。
    pub backup_on_close: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: 1,
            last_catalog: None,
            recent_catalogs: Vec::new(),
            ffmpeg_dir: None,
            gpu: GpuMode::Auto,
            backup_on_close: false,
        }
    }
}

impl AppSettings {
    /// 開いたカタログを記録する（最近の一覧の先頭に移す）。
    pub fn remember_catalog(&mut self, path: &Path) {
        self.last_catalog = Some(path.to_path_buf());
        self.recent_catalogs.retain(|p| p != path);
        self.recent_catalogs.insert(0, path.to_path_buf());
        self.recent_catalogs.truncate(MAX_RECENT_CATALOGS);
    }
}

/// 設定の読み込みの結果。
#[derive(Debug)]
pub struct Loaded {
    /// 設定。
    pub settings: AppSettings,
    /// 壊れていて退避したファイル（あれば。画面に知らせる）。
    pub broken_backup: Option<PathBuf>,
}

/// 設定を読む（なければ既定値。壊れていれば退避して既定値）。
pub fn load(dir: &Path) -> Loaded {
    let path = dir.join(SETTINGS_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %e, "設定を読めないので既定値で起動する");
            }
            return Loaded {
                settings: AppSettings::default(),
                broken_backup: None,
            };
        }
    };
    match serde_json::from_str::<AppSettings>(&text) {
        Ok(settings) => Loaded {
            settings,
            broken_backup: None,
        },
        Err(e) => {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let backup = dir.join(format!("{SETTINGS_FILE}.broken-{stamp}"));
            tracing::warn!(error = %e, backup = %backup.display(), "設定が壊れているので退避して既定値で起動する");
            let broken_backup = fs::rename(&path, &backup).ok().map(|_| backup);
            Loaded {
                settings: AppSettings::default(),
                broken_backup,
            }
        }
    }
}

/// 設定を保存する（一時ファイルに書いてから名前を変える）。
pub fn save(dir: &Path, settings: &AppSettings) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(SETTINGS_FILE);
    let tmp = dir.join(format!(".{SETTINGS_FILE}.tmp"));
    let json = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let d = tempfile::tempdir().unwrap();
        let l = load(d.path());
        assert_eq!(l.settings, AppSettings::default());
        assert!(l.broken_backup.is_none());
    }

    #[test]
    fn round_trip_and_recent_list() {
        let d = tempfile::tempdir().unwrap();
        let mut s = AppSettings::default();
        for i in 0..10 {
            s.remember_catalog(Path::new(&format!("/c/{i}.db")));
        }
        s.remember_catalog(Path::new("/c/3.db"));
        assert_eq!(s.recent_catalogs.len(), MAX_RECENT_CATALOGS);
        assert_eq!(s.recent_catalogs[0], PathBuf::from("/c/3.db"));
        assert_eq!(
            s.recent_catalogs
                .iter()
                .filter(|p| p.as_path() == Path::new("/c/3.db"))
                .count(),
            1,
            "重複しない"
        );
        assert_eq!(s.last_catalog, Some(PathBuf::from("/c/3.db")));
        s.ffmpeg_dir = Some(PathBuf::from("/opt/ffmpeg/bin"));
        s.gpu = GpuMode::Off;
        save(d.path(), &s).unwrap();
        assert_eq!(load(d.path()).settings, s);
    }

    #[test]
    fn broken_file_is_moved_aside() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(SETTINGS_FILE), "{ not json").unwrap();
        let l = load(d.path());
        assert_eq!(l.settings, AppSettings::default());
        let b = l.broken_backup.expect("退避した");
        assert_eq!(fs::read_to_string(b).unwrap(), "{ not json");
        assert!(!d.path().join(SETTINGS_FILE).exists());
    }

    #[test]
    fn unknown_and_missing_fields_use_defaults() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join(SETTINGS_FILE),
            r#"{"last_catalog": "/a.db", "future_field": 1}"#,
        )
        .unwrap();
        let l = load(d.path());
        assert_eq!(l.settings.last_catalog, Some(PathBuf::from("/a.db")));
        assert_eq!(l.settings.gpu, GpuMode::Auto);
    }
}
