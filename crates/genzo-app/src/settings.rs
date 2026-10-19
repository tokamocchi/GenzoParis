//! アプリの設定（カタログの場所・FFmpeg の場所・GPU の使用など）。
//!
//! カタログに保存する設定（キャッシュの場所と上限・既定の色空間・既定のタイムゾーン。genzo-api の
//! `CoreSettings`）とは別に、カタログを開く前に要るものと、カタログによらない取り込みのダイアログの設定
//! （コピー先・名前の規則・まとめて付けるメタデータ。[`ImportPreferences`]）を、OS のアプリの設定のフォルダの
//! `settings.json` に保存する。
//!
//! オンラインの地図（位置情報の後付け LIB-21 の第 2 段階。[`MapPreferences`]）の有効・無効と提供元も、
//! 利用者と PC ごとのものなのでここに置く（カタログを閉じていても効く。CLI は通信しないので使わない）。
//!
//! - 書き込みは一時ファイルに書いてから名前を変える（途中で落ちても前の設定が残る）。
//! - 読めない・壊れている場合は既定値で起動し、壊れたファイルは `settings.json.broken-<時刻>` に退避する
//!   （利用者の設定を黙って捨てない）。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use genzo_api::{FolderRule, GpuMode, ImportMetadata, RenameRule};
use genzo_geo::GeoError;
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
    /// メタデータに使う。FFmpeg は同梱しない。決定事項ログ No.62）。
    pub ffmpeg_dir: Option<PathBuf>,
    /// GPU の使用。
    pub gpu: GpuMode,
    /// アプリの終了時に、前回からの経過時間によらずカタログをバックアップするか（DATA-04）。
    pub backup_on_close: bool,
    /// 取り込みのダイアログの設定（次に取り込むときの既定。IMP-02・IMP-03・IMP-05）。
    pub import: ImportPreferences,
    /// オンラインの地図（地図のタイルと地名の検索。LIB-21）。保存は専用のコマンド（`save_map_preferences`）で
    /// 行う（設定の画面の `update_app_settings` では変えない）。
    pub map: MapPreferences,
}

/// 取り込みのダイアログの設定（取り込みを始めたときに保存し、次に開いたときの初めの値にする。カタログに
/// よらない。コピー先のフォルダはこの PC のものなので、カタログではなくアプリに保存する）。
///
/// 重複と思われるもの・RAW と JPEG の扱い（IMP-04・IMP-06）は、これまでどおりカタログの設定に保存する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportPreferences {
    /// コピーして取り込む（`false` なら今の場所のまま登録する）。
    pub copy: bool,
    /// サブフォルダも含める。
    pub recursive: bool,
    /// コピー先のフォルダ。
    pub destination: Option<PathBuf>,
    /// フォルダの規則。
    pub folder_rule: FolderRule,
    /// 名前を変える（`false` なら元の名前のまま）。
    pub rename: bool,
    /// リネームの規則（名前を変えないときも覚えておく）。
    pub rename_rule: RenameRule,
    /// 取り込んだ写真にまとめて付けるキーワード・説明のメタデータ。
    pub metadata: ImportMetadata,
}

impl Default for ImportPreferences {
    fn default() -> Self {
        Self {
            copy: false,
            recursive: true,
            destination: None,
            folder_rule: FolderRule::default(),
            rename: false,
            rename_rule: RenameRule::default(),
            metadata: ImportMetadata::default(),
        }
    }
}

/// オンラインの地図の設定（位置情報の後付け LIB-21 の第 2 段階。docs/04_architecture.md の 3.16 節）。
///
/// オンラインの地図は **既定で無効**（docs/02 の SEC-02。SEC-01 の例外）。無効の間は、地図のタイルの要求にも
/// 地名の検索にも外部と通信しない（genzo-app の `map`・`protocol` が通信の前に確かめる）。座標の貼り付けで
/// 位置を付ける機能（第 1 段階）は、無効でも使える。
///
/// 提供元の URL は、OSM Foundation のポリシーの「ソフトウェアの更新なしに切り替えられること」のため、
/// `settings.json` で変えられる（`None` なら OSM の既定。https だけ。[`MapPreferences::normalized`]）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MapPreferences {
    /// オンラインの地図を使う（利用者が明示的に有効にしたときだけ `true`）。
    pub online: bool,
    /// 初めて有効にした日時（UTC。RFC 3339。同意の記録。保存のときにアプリが入れ、その後は変えない）。
    pub consented_at: Option<String>,
    /// 地図のタイルの URL のひな形（`{z}`・`{x}`・`{y}` を 1 つずつ含む https の URL。`None` なら
    /// `https://tile.openstreetmap.org/{z}/{x}/{y}.png`）。
    pub tile_url: Option<String>,
    /// 地名の検索（Nominatim と同じ API の `/search`）の URL（https。`None` なら
    /// `https://nominatim.openstreetmap.org/search`）。
    pub search_url: Option<String>,
}

impl MapPreferences {
    /// 保存の前に整える: URL の前後の空白を除き、空なら `None`（既定の提供元）にして、形を確かめる
    /// （genzo-geo の `validate_tile_url_template`・`validate_search_url`。https だけ）。
    pub fn normalized(mut self) -> Result<Self, GeoError> {
        fn non_empty(url: Option<String>) -> Option<String> {
            url.map(|u| u.trim().to_owned()).filter(|u| !u.is_empty())
        }
        self.tile_url = non_empty(self.tile_url);
        self.search_url = non_empty(self.search_url);
        if let Some(url) = &self.tile_url {
            genzo_geo::validate_tile_url_template(url)?;
        }
        if let Some(url) = &self.search_url {
            genzo_geo::validate_search_url(url)?;
        }
        Ok(self)
    }
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
            import: ImportPreferences::default(),
            map: MapPreferences::default(),
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
        s.import.copy = true;
        s.import.destination = Some(PathBuf::from("/写真"));
        s.import.folder_rule = FolderRule::YearMonthDay;
        s.import.rename = true;
        s.import.rename_rule.template = "{date}_{text}-{seq}".to_owned();
        s.import.metadata.keywords = vec![vec!["場所".to_owned(), "京都".to_owned()]];
        s.import
            .metadata
            .fields
            .insert(genzo_model::MetadataField::Creator, "岡本".to_owned());
        s.map.online = true;
        s.map.consented_at = Some("2026-10-20T01:02:03Z".to_owned());
        s.map.search_url = Some("https://geocoder.example.org/search?key=abc".to_owned());
        save(d.path(), &s).unwrap();
        assert_eq!(load(d.path()).settings, s);
    }

    /// 取り込みの設定のない古い設定のファイル・項目の欠けた設定は、既定の値で読む。
    #[test]
    fn import_preferences_default_when_missing() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join(SETTINGS_FILE),
            r#"{"version": 1, "import": {"copy": true, "folder_rule": "single"}}"#,
        )
        .unwrap();
        let l = load(d.path()).settings;
        assert!(l.import.copy);
        assert_eq!(l.import.folder_rule, FolderRule::Single);
        assert!(l.import.recursive);
        assert_eq!(l.import.rename_rule, RenameRule::default());
        fs::write(d.path().join(SETTINGS_FILE), r#"{"version": 1}"#).unwrap();
        assert_eq!(load(d.path()).settings.import, ImportPreferences::default());
    }

    /// 地図の設定のない古い設定のファイルは、オンラインの地図を無効として読む（SEC-02）。
    #[test]
    fn map_preferences_default_to_offline() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(SETTINGS_FILE), r#"{"version": 1}"#).unwrap();
        let l = load(d.path()).settings;
        assert!(!l.map.online);
        assert_eq!(l.map, MapPreferences::default());
        fs::write(
            d.path().join(SETTINGS_FILE),
            r#"{"version": 1, "map": {"online": true, "tile_url": "https://tiles.example.org/{z}/{x}/{y}.png"}}"#,
        )
        .unwrap();
        let l = load(d.path()).settings;
        assert!(l.map.online);
        assert_eq!(
            l.map.tile_url.as_deref(),
            Some("https://tiles.example.org/{z}/{x}/{y}.png")
        );
        assert_eq!((l.map.consented_at, l.map.search_url), (None, None));
    }

    #[test]
    fn map_urls_are_trimmed_and_checked() {
        let p = MapPreferences {
            online: true,
            consented_at: None,
            tile_url: Some("  https://tiles.example.org/{z}/{x}/{y}.png ".to_owned()),
            search_url: Some("   ".to_owned()),
        }
        .normalized()
        .unwrap();
        assert_eq!(
            p.tile_url.as_deref(),
            Some("https://tiles.example.org/{z}/{x}/{y}.png")
        );
        assert_eq!(p.search_url, None, "空白だけなら既定の提供元");
        for bad in [
            "http://tile.openstreetmap.org/{z}/{x}/{y}.png",
            "https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png",
            "https://tiles.example.org/{z}/{x}.png",
        ] {
            let r = MapPreferences {
                tile_url: Some(bad.to_owned()),
                ..MapPreferences::default()
            }
            .normalized();
            assert!(matches!(r, Err(GeoError::InvalidInput(_))), "{bad}");
        }
        let r = MapPreferences {
            search_url: Some("http://nominatim.openstreetmap.org/search".to_owned()),
            ..MapPreferences::default()
        }
        .normalized();
        assert!(matches!(r, Err(GeoError::InvalidInput(_))));
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
