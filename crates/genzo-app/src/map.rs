//! オンラインの地図（位置情報の後付け LIB-21 の第 2 段階。docs/04_architecture.md の 3.16 節）。
//!
//! 写真に位置を付けるダイアログの地図（OpenStreetMap のタイル）と地名の検索（Nominatim）は、genzo-geo で
//! Rust の側から外部のサーバーに問い合わせる（WebView は外部と通信しない。tauri.conf.json の CSP は
//! 変えない）。
//!
//! | 画面からの入口 | 内容 |
//! |---|---|
//! | `genzo://localhost/tile/{z}/{x}/{y}`（Windows は `http://genzo.localhost/tile/...`） | 地図のタイル（PNG。[`crate::protocol::reply_tile`]。ディスクにキャッシュする） |
//! | コマンド `search_places` | 地名の検索（[`search_places`]） |
//! | コマンド `save_map_preferences` | 有効・無効と提供元の URL の保存（[`save_preferences`]） |
//! | コマンド `map_cache_info`・`clear_map_cache` | タイルのキャッシュの大きさと消去（[`cache_info`]・[`clear_cache`]） |
//! | コマンド `open_map_link` | 帰属の表示・地図の誤りの報告・利用ポリシーのページを既定のブラウザで開く（[`MapLink`]。決めたページだけ） |
//!
//! - オンラインの地図は **既定で無効**（docs/02 の SEC-02。SEC-01 の例外）。無効の間は、タイルの要求に 403 を
//!   返し、検索はエラーにして、どちらも通信しない（[`crate::settings::MapPreferences`]）。キャッシュの大きさと
//!   消去、ページを開くことは、無効でも使える（外部のサーバーと通信しない）。
//! - 本物の HTTP のクライアント（genzo-geo の `UreqClient`）は、初めて通信するときに作る（`LazyClient`。無効の間は
//!   TLS の準備もしない）。1 つをタイルと検索で共有する（接続を使い回す）。
//! - 提供元は設定の URL から作り、設定が変わったら作り直す（[`MapService`]）。地名の検索の「1 秒に 1 回」の関所は
//!   genzo-geo がプロセス全体で 1 つ持つので、作り直しても間隔は保たれる。
//! - タイルのキャッシュは、OS のキャッシュのフォルダ（`app_cache_dir()`）の下の専用のサブフォルダ
//!   [`MAP_CACHE_DIR`] に置く。Windows ではこのフォルダが WebView2 のデータとログの親なので、消すのは
//!   サブフォルダの中の提供元のフォルダだけ（genzo-geo の `TileService::clear_cache`）。場所が分からなければ、
//!   ディスクにキャッシュせずに動く（起動を止めない。lib.rs の `setup`）。
//! - ログには検索語・タイルの番号・座標を書かない（決定事項ログ No.75 の方針）。
//!
//! OSM Foundation の利用ポリシー（タイル: <https://operations.osmfoundation.org/policies/tiles/> 、Nominatim:
//! <https://operations.osmfoundation.org/policies/nominatim/> ）で守ることは genzo-geo の doc を参照。この
//! モジュールが受け持つのは、有効にするまで通信しないこと・提供元を設定で切り替えられること・帰属の表示の
//! リンクを WebView の中で開かないこと（既定のブラウザで開く）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use genzo_api::{ErrorInfo, ErrorKind};
use genzo_geo::{
    CacheInfo, GeoError, Geocoder, GeocoderProvider, HttpClient, HttpResponse, OSM_ATTRIBUTION,
    OSM_MAX_ZOOM, Place, TileProvider, TileService, UreqClient,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::settings::{AppSettings, MapPreferences};
use crate::state::{AppState, error};

/// タイルのキャッシュのフォルダの名前（OS のキャッシュのフォルダ `app_cache_dir()` の下の専用のサブフォルダ）。
pub const MAP_CACHE_DIR: &str = "map-tiles";

/// オンラインの地図が無効なときの案内。
const HINT_ENABLE: &str = "設定でオンラインの地図を有効にしてください";
/// 通信に失敗したときの案内。
const HINT_NETWORK: &str = "ネットワークの接続とプロキシの設定を確かめて、やり直してください";
/// サーバーがエラー・不正な応答を返したときの案内。
const HINT_SERVER: &str =
    "しばらくしてからやり直してください。続くときは、設定の地図の提供元（URL）を確かめてください";
/// 設定の提供元の URL が使えないときの案内（settings.json を手で直した場合など）。
const HINT_PROVIDER: &str =
    "設定のタイル・検索の URL を直すか、空にして既定（OpenStreetMap）に戻してください";
/// キャッシュのファイルを読み書きできないときの案内。
const HINT_CACHE: &str = "地図のキャッシュのフォルダの空き容量と権限を確かめてください";

/// 地図のタイルと地名の検索（提供元ごとに作り、設定の URL が変わったら作り直す）。複数のスレッドから同時に
/// 使える。
pub struct MapService {
    client: Arc<dyn HttpClient>,
    cache_dir: Option<PathBuf>,
    tiles: Mutex<Option<Arc<TileService>>>,
    geocoder: Mutex<Option<Arc<Geocoder>>>,
}

impl std::fmt::Debug for MapService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapService")
            .field("cache_dir", &self.cache_dir)
            .finish_non_exhaustive()
    }
}

impl MapService {
    /// 本物の通信（genzo-geo の `UreqClient`。初めて通信するときに作る）で作る。`cache_dir` はタイルの
    /// キャッシュの専用のフォルダ（`None` ならディスクにキャッシュしない）。
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self::with_client(Arc::new(LazyClient::default()), cache_dir)
    }

    /// HTTP のクライアントを指定して作る（テスト用。本物の外部のサーバーに問い合わせないため）。
    pub fn with_client(client: Arc<dyn HttpClient>, cache_dir: Option<PathBuf>) -> Self {
        Self {
            client,
            cache_dir,
            tiles: Mutex::new(None),
            geocoder: Mutex::new(None),
        }
    }

    /// タイルのキャッシュのフォルダ。
    pub fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    /// 設定の提供元のタイル（前と同じ提供元なら作り直さない）。設定の URL が使えなければ
    /// [`GeoError::InvalidInput`]。オンラインの地図が有効かは呼び出し元が確かめる。
    pub fn tiles(&self, preferences: &MapPreferences) -> Result<Arc<TileService>, GeoError> {
        let provider = tile_provider(preferences)?;
        let mut slot = self.tiles.lock();
        if let Some(service) = slot.as_ref()
            && *service.provider() == provider
        {
            return Ok(Arc::clone(service));
        }
        let service = Arc::new(TileService::new(
            Arc::clone(&self.client),
            provider,
            self.cache_dir.clone(),
        ));
        *slot = Some(Arc::clone(&service));
        Ok(service)
    }

    /// 設定の提供元の地名の検索（前と同じ提供元なら作り直さない。結果のキャッシュを保つ）。設定の URL が
    /// 使えなければ [`GeoError::InvalidInput`]。オンラインの地図が有効かは呼び出し元が確かめる。
    pub fn geocoder(&self, preferences: &MapPreferences) -> Result<Arc<Geocoder>, GeoError> {
        let provider = match &preferences.search_url {
            None => GeocoderProvider::nominatim(),
            Some(url) => GeocoderProvider::new(url)?,
        };
        let mut slot = self.geocoder.lock();
        if let Some(geocoder) = slot.as_ref()
            && *geocoder.provider() == provider
        {
            return Ok(Arc::clone(geocoder));
        }
        let geocoder = Arc::new(Geocoder::new(Arc::clone(&self.client), provider));
        *slot = Some(Arc::clone(&geocoder));
        Ok(geocoder)
    }

    /// キャッシュの大きさと消去に使うタイル（キャッシュの中のすべての提供元のフォルダを扱うので、設定の
    /// URL が使えないときは既定の提供元で作る。通信はしない）。
    fn cache_service(&self, preferences: &MapPreferences) -> Arc<TileService> {
        self.tiles(preferences).unwrap_or_else(|_| {
            Arc::new(TileService::new(
                Arc::clone(&self.client),
                TileProvider::openstreetmap(),
                self.cache_dir.clone(),
            ))
        })
    }
}

/// 設定のタイルの提供元（`None` なら OSM の標準のタイル）。設定の URL の提供元の最大のズームと帰属の表示は、
/// OSM の標準と同じとみなす（仮置き。OSM のデータを使う別の提供元を想定。画面の地図も同じ値を使う）。
fn tile_provider(preferences: &MapPreferences) -> Result<TileProvider, GeoError> {
    match &preferences.tile_url {
        None => Ok(TileProvider::openstreetmap()),
        Some(url) => TileProvider::new(url, OSM_MAX_ZOOM, OSM_ATTRIBUTION),
    }
}

/// 本物の HTTP のクライアント（genzo-geo の `UreqClient`）を、初めて通信するときに作る（オンラインの地図が
/// 無効の間は TLS の準備もしない）。作れなければ、そのときの要求を [`GeoError::Network`] にして、次の要求で
/// 作り直す。
#[derive(Default)]
struct LazyClient {
    inner: Mutex<Option<Arc<UreqClient>>>,
}

impl HttpClient for LazyClient {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, String)],
        max_body_bytes: u64,
    ) -> Result<HttpResponse, GeoError> {
        let client = {
            let mut slot = self.inner.lock();
            match slot.as_ref() {
                Some(client) => Arc::clone(client),
                None => {
                    let client = Arc::new(UreqClient::new()?);
                    *slot = Some(Arc::clone(&client));
                    client
                }
            }
        };
        // 要求の間はロックを持たない（タイルと検索を並べて送れるように。接続は ureq が使い回す）。
        client.get(url, headers, max_body_bytes)
    }
}

/// 既定のブラウザで開くページ（`open_map_link` のコマンドの `kind`）。任意の URL は開かない（画面から渡せるのは
/// この種類だけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapLink {
    /// OSM の著作権とライセンス（地図の帰属の表示「© OpenStreetMap contributors」のリンク先）。
    Copyright,
    /// 地図の誤りの報告（タイルの利用ポリシーが、アプリに置くことを勧めるリンク）。
    FixTheMap,
    /// タイルの利用ポリシー（有効にする前の確認で示す）。
    TilePolicy,
    /// Nominatim（地名の検索）の利用ポリシー（有効にする前の確認で示す）。
    NominatimPolicy,
}

impl MapLink {
    /// すべての種類。
    pub const ALL: [MapLink; 4] = [
        Self::Copyright,
        Self::FixTheMap,
        Self::TilePolicy,
        Self::NominatimPolicy,
    ];

    /// 開く URL（ポリシーの原文（そのページの元の GitHub のソース）にある URL。公開ページは未確認）。
    pub const fn url(self) -> &'static str {
        match self {
            Self::Copyright => "https://www.openstreetmap.org/copyright",
            Self::FixTheMap => "https://www.openstreetmap.org/fixthemap",
            Self::TilePolicy => "https://operations.osmfoundation.org/policies/tiles/",
            Self::NominatimPolicy => "https://operations.osmfoundation.org/policies/nominatim/",
        }
    }
}

/// オンラインの地図が無効なときのエラー（通信していない）。
pub fn disabled() -> ErrorInfo {
    error(
        ErrorKind::InvalidInput,
        "オンラインの地図は無効です",
        Some(HINT_ENABLE),
    )
}

/// genzo-geo のエラーを画面に返す形にする。
///
/// | genzo-geo | 種類 |
/// |---|---|
/// | `Disabled` | `invalid_input`（[`disabled`]） |
/// | `InvalidInput`（空の検索語・https でない URL など） | `invalid_input` |
/// | `Network`（接続・TLS・タイムアウト・プロキシ） | `network` |
/// | `Status`・`BadResponse`（サーバーのエラー・不正な応答） | `network` |
/// | `Io`（タイルのキャッシュの読み書き） | `storage` |
pub fn geo_error(e: GeoError) -> ErrorInfo {
    match e {
        GeoError::Disabled => disabled(),
        GeoError::InvalidInput(message) => error(ErrorKind::InvalidInput, message, None),
        GeoError::Network(_) => error(ErrorKind::Network, e.to_string(), Some(HINT_NETWORK)),
        GeoError::Status(_) | GeoError::BadResponse(_) => {
            error(ErrorKind::Network, e.to_string(), Some(HINT_SERVER))
        }
        GeoError::Io { .. } => error(ErrorKind::Storage, e.to_string(), Some(HINT_CACHE)),
    }
}

/// 設定の提供元の URL が使えないときのエラー。
fn provider_error(e: GeoError) -> ErrorInfo {
    let mut info = geo_error(e);
    info.message = format!("地図の提供元の設定が正しくありません: {}", info.message);
    info.hint = Some(HINT_PROVIDER.to_owned());
    info
}

/// 地名を検索する（`search_places` のコマンドの本体）。オンラインの地図が無効なら、通信せずに
/// [`disabled`] のエラー。検索語の検査・1 秒に 1 回の間隔・結果のキャッシュは genzo-geo の `Geocoder`。
///
/// 画面は、入力の途中では呼ばず、Enter かボタンのときだけ呼ぶ（Nominatim のポリシーのオートコンプリートの禁止）。
pub fn search_places(state: &AppState, query: &str) -> Result<Vec<Place>, ErrorInfo> {
    let preferences = state.settings().map;
    if !preferences.online {
        return Err(disabled());
    }
    let geocoder = state.map().geocoder(&preferences).map_err(provider_error)?;
    geocoder.search(query).map_err(geo_error)
}

/// オンラインの地図の設定を保存する（`save_map_preferences` のコマンドの本体）。
///
/// - URL は前後の空白を除き、空なら既定の提供元にする。https でない・形の違う URL は `invalid_input` で、
///   何も保存しない（[`MapPreferences::normalized`]）。
/// - `consented_at`（初めて有効にした日時）はアプリが入れる: まだなく、有効にするときに今の日時（UTC）を
///   入れ、その後は変えない（画面が送った値は使わない）。
/// - すぐに効く（次のタイルの要求・検索から。カタログを開き直す必要はない）。
pub fn save_preferences(
    state: &AppState,
    preferences: MapPreferences,
) -> Result<AppSettings, ErrorInfo> {
    let mut next = preferences.normalized().map_err(geo_error)?;
    let was_online = state.settings().map.online;
    let saved = state.update_settings(|s| {
        next.consented_at = s
            .map
            .consented_at
            .clone()
            .or_else(|| next.online.then(now_utc));
        s.map = next;
    })?;
    if saved.map.online != was_online {
        if saved.map.online {
            tracing::info!("オンラインの地図を有効にしました");
        } else {
            tracing::info!("オンラインの地図を無効にしました");
        }
    }
    Ok(saved)
}

/// 今の日時（UTC。RFC 3339 の秒まで）。
fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// タイルのキャッシュの大きさ（`map_cache_info` のコマンドの本体。通信しない。オンラインの地図が無効でも返す）。
pub fn cache_info(state: &AppState) -> CacheInfo {
    state
        .map()
        .cache_service(&state.settings().map)
        .cache_info()
}

/// タイルのキャッシュを消して、消した後の大きさを返す（`clear_map_cache` のコマンドの本体。通信しない。
/// [`MAP_CACHE_DIR`] の中の提供元のフォルダだけを消す）。
pub fn clear_cache(state: &AppState) -> Result<CacheInfo, ErrorInfo> {
    let service = state.map().cache_service(&state.settings().map);
    service.clear_cache().map_err(geo_error)?;
    Ok(service.cache_info())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_links_are_only_the_known_pages() {
        let kinds: Vec<_> = MapLink::ALL
            .iter()
            .map(|k| serde_json::to_value(k).unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "copyright",
                "fix_the_map",
                "tile_policy",
                "nominatim_policy"
            ]
        );
        for k in MapLink::ALL {
            let url = k.url();
            assert!(
                url.starts_with("https://www.openstreetmap.org/")
                    || url.starts_with("https://operations.osmfoundation.org/policies/"),
                "{url}"
            );
            let back: MapLink = serde_json::from_value(serde_json::to_value(k).unwrap()).unwrap();
            assert_eq!(back, k);
        }
        // 画面から任意の URL や知らない種類は渡せない。
        for bad in [
            "https://example.com/",
            "Copyright",
            "fixthemap",
            "",
            "file:///etc/passwd",
        ] {
            assert!(
                serde_json::from_value::<MapLink>(serde_json::json!(bad)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn geo_errors_get_kinds_and_hints() {
        let e = geo_error(GeoError::Network("接続できません".into()));
        assert_eq!(e.kind, ErrorKind::Network);
        assert!(e.retryable && e.user_actionable);
        assert!(e.hint.is_some());
        assert_eq!(geo_error(GeoError::Status(503)).kind, ErrorKind::Network);
        assert_eq!(
            geo_error(GeoError::BadResponse("壊れた JSON".into())).kind,
            ErrorKind::Network
        );
        let e = geo_error(GeoError::InvalidInput("検索語が空です".into()));
        assert_eq!(
            (e.kind, e.message.as_str()),
            (ErrorKind::InvalidInput, "検索語が空です")
        );
        let e = geo_error(GeoError::Disabled);
        assert_eq!(e.kind, ErrorKind::InvalidInput);
        assert_eq!(e.hint.as_deref(), Some(HINT_ENABLE));
        let io = std::io::Error::other("書けない");
        let e = geo_error(GeoError::Io {
            path: "/c/map-tiles".into(),
            source: Arc::new(io),
        });
        assert_eq!(e.kind, ErrorKind::Storage);
    }

    /// 提供元が変わったときだけ作り直す（地名の検索の結果のキャッシュを保つ）。作るだけでは通信しない。
    #[test]
    fn services_are_rebuilt_only_when_the_provider_changes() {
        let service = MapService::new(None);
        let preferences = MapPreferences::default();
        let tiles = service.tiles(&preferences).unwrap();
        assert_eq!(tiles.provider(), &TileProvider::openstreetmap());
        // 同じ提供元なら作り直さない。URL が変われば作り直す。
        assert!(Arc::ptr_eq(&tiles, &service.tiles(&preferences).unwrap()));
        let custom = MapPreferences {
            tile_url: Some("https://tiles.example.org/{z}/{x}/{y}.png".into()),
            search_url: Some("https://geocoder.example.org/search".into()),
            ..MapPreferences::default()
        };
        let other = service.tiles(&custom).unwrap();
        assert!(!Arc::ptr_eq(&tiles, &other));
        assert_eq!(
            other.provider().url_template(),
            "https://tiles.example.org/{z}/{x}/{y}.png"
        );
        let g = service.geocoder(&custom).unwrap();
        assert_eq!(
            g.provider().search_url(),
            "https://geocoder.example.org/search"
        );
        assert!(Arc::ptr_eq(&g, &service.geocoder(&custom).unwrap()));
        assert_eq!(
            service.geocoder(&preferences).unwrap().provider(),
            &GeocoderProvider::nominatim()
        );
        // 手で書き換えた設定の使えない URL は誤り（既定の提供元に黙って戻さない）。
        let broken = MapPreferences {
            tile_url: Some("http://tiles.example.org/{z}/{x}/{y}.png".into()),
            ..MapPreferences::default()
        };
        assert!(matches!(
            service.tiles(&broken),
            Err(GeoError::InvalidInput(_))
        ));
    }
}
