//! (v1 の map) オンラインの地図（位置情報の後付け LIB-21 の第 2 段階）: 設定の保存と読み直し、地名の検索と
//! 地図のタイル（`genzo://localhost/tile/{z}/{x}/{y}`）が有効なときだけ通信すること、地名の検索は検索の
//! サービスを設定したとき（検索サービスの URL か、OSM の公開の Nominatim を使う切り替え。既定はどちらもない）
//! だけ通信すること、タイルのキャッシュの大きさと消去。
//!
//! 通信は差し替える（[`FakeHttp`]。本物の OSM のサーバーに問い合わせない。SEC-01・CI）。地名の検索の
//! 「1 秒に 1 回」の間隔はプロセス全体の関所（genzo-geo）が守るので、通信する検索を続けると 1 秒ずつ待つ
//! （間隔そのものは genzo-geo の単体テストが差し替えた時計で確かめる。ここでは時間で結果を決めない）。

use std::sync::Arc;

use genzo_api::ErrorKind;
use genzo_app::map::{self, MapService};
use genzo_app::protocol;
use genzo_app::settings::MapPreferences;
use genzo_app::state::AppState;
use genzo_geo::{CacheInfo, GeoError, HttpClient, HttpResponse};
use parking_lot::Mutex;

use crate::support::{Env, WAIT_LIMIT, assert_kind};

/// 要求の URL から応答を決める関数。
type Respond = Box<dyn Fn(&str) -> Result<HttpResponse, GeoError> + Send + Sync>;

/// 差し替えの HTTP のクライアント（要求の URL を記録し、決めた応答を返す）。
pub struct FakeHttp {
    urls: Mutex<Vec<String>>,
    respond: Respond,
}

impl FakeHttp {
    pub fn new(
        respond: impl Fn(&str) -> Result<HttpResponse, GeoError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            urls: Mutex::new(Vec::new()),
            respond: Box::new(respond),
        })
    }

    /// 要求の URL（届いた順）。
    pub fn urls(&self) -> Vec<String> {
        self.urls.lock().clone()
    }

    /// 要求の数。
    pub fn count(&self) -> usize {
        self.urls.lock().len()
    }
}

impl HttpClient for FakeHttp {
    fn get(
        &self,
        url: &str,
        _headers: &[(&str, String)],
        _max_body_bytes: u64,
    ) -> Result<HttpResponse, GeoError> {
        self.urls.lock().push(url.to_owned());
        (self.respond)(url)
    }
}

/// 応答（`headers` の名前は小文字）。
pub fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
        body: body.to_vec(),
    }
}

/// 通信を差し替えた AppState（タイルのキャッシュは一時フォルダの `map-tiles`）。
pub fn state_with_fake(env: &Env, fake: &Arc<FakeHttp>) -> AppState {
    env.state().with_map(MapService::with_client(
        Arc::clone(fake) as Arc<dyn HttpClient>,
        Some(env.root().join("キャッシュ").join(map::MAP_CACHE_DIR)),
    ))
}

/// オンラインの地図を有効にする（ほかの設定は既定）。
pub fn enable(state: &AppState) {
    let saved = map::save_preferences(
        state,
        MapPreferences {
            online: true,
            ..MapPreferences::default()
        },
    )
    .unwrap();
    assert!(saved.map.online);
}

/// 設定を保存し、URL を整えて確かめ、同意の日時はアプリが入れて変えない。別の AppState で読み直せる。
/// 使えない URL は保存しない。キャッシュは、タイルがなければ空で、消してもよい（通信しない）。
pub fn map_preferences_are_saved_and_read_again() {
    let env = Env::new();
    let fake = FakeHttp::new(|_| panic!("通信しない"));
    let state = state_with_fake(&env, &fake);
    assert_eq!(state.settings().map, MapPreferences::default());
    assert!(!state.settings().map.online, "既定で無効（SEC-02）");

    let saved = map::save_preferences(
        &state,
        MapPreferences {
            online: true,
            // 画面が送った値は使わない（アプリが初めて有効にした日時を入れる）。
            consented_at: Some("1999-01-01T00:00:00Z".to_owned()),
            tile_url: Some("  https://tiles.example.org/{z}/{x}/{y}.png ".to_owned()),
            search_url: Some(" ".to_owned()),
            public_nominatim: true,
            tile_attribution: Some(" Maps © 提供元 ".to_owned()),
        },
    )
    .unwrap();
    let m = saved.map.clone();
    assert!(m.online);
    let consented = m.consented_at.clone().expect("同意の日時");
    assert_ne!(consented, "1999-01-01T00:00:00Z");
    assert!(
        chrono::DateTime::parse_from_rfc3339(&consented).is_ok(),
        "{consented}"
    );
    assert_eq!(
        m.tile_url.as_deref(),
        Some("https://tiles.example.org/{z}/{x}/{y}.png")
    );
    assert_eq!(m.search_url, None, "空白だけなら URL なし");
    assert!(m.public_nominatim, "公開の Nominatim の切り替えも保存する");
    assert_eq!(m.tile_attribution.as_deref(), Some("Maps © 提供元"));
    assert_eq!(state.settings().map, m);

    // 使えない URL（http・サブドメインの置換子）は保存しない。
    for (tile_url, search_url) in [
        (Some("http://tile.openstreetmap.org/{z}/{x}/{y}.png"), None),
        (
            Some("https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png"),
            None,
        ),
        (None, Some("http://geocoder.example.org/search")),
    ] {
        let e = assert_kind(
            map::save_preferences(
                &state,
                MapPreferences {
                    online: true,
                    consented_at: None,
                    tile_url: tile_url.map(str::to_owned),
                    search_url: search_url.map(str::to_owned),
                    public_nominatim: false,
                    tile_attribution: None,
                },
            ),
            ErrorKind::InvalidInput,
        );
        assert!(e.message.contains("URL"), "{e:?}");
        assert_eq!(state.settings().map, m, "変えていない");
    }

    // 無効にしても、有効に戻しても、同意の日時は初めのまま。
    let off = map::save_preferences(&state, MapPreferences::default()).unwrap();
    assert!(!off.map.online);
    assert_eq!(off.map.consented_at.as_deref(), Some(consented.as_str()));
    assert_eq!(off.map.tile_url, None);
    assert!(!off.map.public_nominatim, "欠けた欄は既定（OFF）");
    let on = map::save_preferences(
        &state,
        MapPreferences {
            online: true,
            ..MapPreferences::default()
        },
    )
    .unwrap();
    assert_eq!(on.map.consented_at.as_deref(), Some(consented.as_str()));

    // 別の AppState（アプリの起動し直し）で読み直せる。
    let again = env.state();
    assert_eq!(again.settings().map, on.map);

    // タイルがなければキャッシュは空で、消してもよい（通信しない）。
    assert_eq!(map::cache_info(&state), CacheInfo::default());
    assert_eq!(map::clear_cache(&state).unwrap(), CacheInfo::default());
    // キャッシュの場所がない（ディスクにキャッシュしない）ときも同じ。
    assert_eq!(map::cache_info(&again), CacheInfo::default());
    assert_eq!(fake.count(), 0);
}

/// jsonv2 の検索の結果（Nominatim と同じ形。`lat`・`lon`・`boundingbox` は文字列）。
const TOKYO_STATION: &str = r#"[{"place_id": 1, "lat": "35.6812362", "lon": "139.7671248",
  "category": "railway", "type": "station", "name": "東京駅",
  "display_name": "東京駅, 丸の内一丁目, 千代田区, 東京都, 日本",
  "boundingbox": ["35.6762362", "35.6862362", "139.7621248", "139.7721248"]}]"#;

/// 地名の検索は、オンラインの地図が有効で、検索のサービスを設定したときだけ通信する（既定はどちらもなく、
/// 通信しない）。OSM の公開の Nominatim を使う切り替えを ON にすると公開の URL に、検索サービスの URL を書くと
/// その URL に問い合わせる。結果はキャッシュし、誤りは種類で返す。
pub fn places_are_searched_only_when_online_maps_are_enabled() {
    let env = Env::new();
    let fake = FakeHttp::new(|url| {
        if url.contains("q=%E5%A4%A7%E9%98%AA") {
            // 大阪: サーバーのエラー。
            Ok(response(503, &[], b""))
        } else if url.contains("q=%E4%BA%AC%E9%83%BD") {
            // 京都: 通信の失敗。
            Err(GeoError::Network("接続できません".to_owned()))
        } else {
            Ok(response(
                200,
                &[("content-type", "application/json; charset=utf-8")],
                TOKYO_STATION.as_bytes(),
            ))
        }
    });
    let state = state_with_fake(&env, &fake);

    // 無効なら通信しない。
    let e = assert_kind(
        map::search_places(&state, "東京駅"),
        ErrorKind::InvalidInput,
    );
    assert!(e.hint.as_deref().unwrap().contains("有効"), "{e:?}");
    assert_eq!(fake.count(), 0);

    // 有効にしても、既定（公開の Nominatim の切り替えは OFF・検索の URL なし）では、検索のサービスが設定されて
    // いないので通信しない。
    enable(&state);
    assert!(!state.settings().map.public_nominatim, "既定は OFF");
    let e = assert_kind(
        map::search_places(&state, "東京駅"),
        ErrorKind::InvalidInput,
    );
    assert_eq!(e.message, "地名の検索のサービスが設定されていません");
    let hint = e.hint.as_deref().unwrap();
    assert!(hint.contains("Nominatim") && hint.contains("URL"), "{hint}");
    assert_eq!(fake.count(), 0);

    // 公開の Nominatim を使う切り替えを ON にすると、公開の URL に問い合わせる。
    map::save_preferences(
        &state,
        MapPreferences {
            online: true,
            public_nominatim: true,
            ..MapPreferences::default()
        },
    )
    .unwrap();
    let places = map::search_places(&state, "東京駅").unwrap();
    assert_eq!(places.len(), 1);
    let p = &places[0];
    assert_eq!((p.name.as_str(), p.kind.as_str()), ("東京駅", "station"));
    assert!((p.lat - 35.681_236_2).abs() < 1e-9 && (p.lon - 139.767_124_8).abs() < 1e-9);
    assert_eq!(
        p.bbox,
        Some([35.676_236_2, 35.686_236_2, 139.762_124_8, 139.772_124_8])
    );
    // 画面に返す JSON の形（UI の Place の型）。
    let json = serde_json::to_value(p).unwrap();
    for key in [
        "name",
        "display_name",
        "lat",
        "lon",
        "bbox",
        "category",
        "kind",
    ] {
        assert!(json.get(key).is_some(), "{key}: {json}");
    }
    let urls = fake.urls();
    assert_eq!(urls.len(), 1);
    let url = &urls[0];
    assert!(
        url.starts_with("https://nominatim.openstreetmap.org/search?"),
        "{url}"
    );
    for part in [
        "q=%E6%9D%B1%E4%BA%AC%E9%A7%85",
        "format=jsonv2",
        "accept-language=ja",
    ] {
        assert!(url.contains(part), "{part}: {url}");
    }
    assert!(!url.contains("email"), "{url}");

    // 同じ検索語（前後の空白は除く）は、キャッシュから返して通信しない。空の検索語は通信しない。
    assert_eq!(map::search_places(&state, "  東京駅 ").unwrap(), places);
    assert_kind(map::search_places(&state, "   "), ErrorKind::InvalidInput);
    assert_eq!(fake.count(), 1);

    // サーバーのエラー・通信の失敗は network（やり直せる）。
    let e = assert_kind(map::search_places(&state, "大阪駅"), ErrorKind::Network);
    assert!(e.message.contains("503"), "{e:?}");
    assert_kind(map::search_places(&state, "京都駅"), ErrorKind::Network);
    assert_eq!(fake.count(), 3);

    // 検索サービスの URL を書くと、切り替えが ON でも、次の検索からその URL に送る（問い合わせの付いた URL にも
    // つなげる）。
    map::save_preferences(
        &state,
        MapPreferences {
            online: true,
            public_nominatim: true,
            search_url: Some("https://geocoder.example.org/search?key=abc".to_owned()),
            ..MapPreferences::default()
        },
    )
    .unwrap();
    map::search_places(&state, "東京駅").unwrap();
    let last = fake.urls().pop().unwrap();
    assert!(
        last.starts_with("https://geocoder.example.org/search?key=abc&q="),
        "{last}"
    );
    assert_eq!(fake.count(), 4);

    // 切り替えが OFF でも、URL を書けばその URL に送る。
    map::save_preferences(
        &state,
        MapPreferences {
            online: true,
            search_url: Some("https://geocoder.example.net/search".to_owned()),
            ..MapPreferences::default()
        },
    )
    .unwrap();
    map::search_places(&state, "東京駅").unwrap();
    let last = fake.urls().pop().unwrap();
    assert!(
        last.starts_with("https://geocoder.example.net/search?q="),
        "{last}"
    );
    assert_eq!(fake.count(), 5);

    // URL を空にして切り替えも OFF に戻すと、検索のサービスがないので通信しない。
    enable(&state);
    let e = assert_kind(
        map::search_places(&state, "東京駅"),
        ErrorKind::InvalidInput,
    );
    assert_eq!(e.message, "地名の検索のサービスが設定されていません");
    assert_eq!(fake.count(), 5);

    // 無効に戻すと、キャッシュにある検索語でも通信の前に断る。
    map::save_preferences(
        &state,
        MapPreferences {
            public_nominatim: true,
            ..MapPreferences::default()
        },
    )
    .unwrap();
    let e = assert_kind(
        map::search_places(&state, "東京駅"),
        ErrorKind::InvalidInput,
    );
    assert!(e.hint.as_deref().unwrap().contains("有効"), "{e:?}");
    assert_eq!(fake.count(), 5);
}

/// PNG の署名で始まる本文（タイルの画像の代わり。genzo-geo は署名だけを確かめ、デコードしない）。
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n tile";

/// 地図のタイルは、オンラインの地図が有効なときだけ、カタログを開いていなくても返す。2 回目はキャッシュから
/// 返す。範囲外は 400、上流の 404 は 404、上流のエラー・通信の失敗・PNG でない応答は 502。キャッシュは専用の
/// フォルダの中の提供元のフォルダだけを消す。
pub fn tiles_are_served_only_when_online_maps_are_enabled() {
    let env = Env::new();
    let fake = FakeHttp::new(|url| match url {
        "https://tile.openstreetmap.org/1/0/0.png" | "https://tiles.example.org/1/0/0.png" => {
            Ok(response(
                200,
                &[
                    ("content-type", "image/png"),
                    ("cache-control", "max-age=86400"),
                ],
                PNG,
            ))
        }
        "https://tile.openstreetmap.org/1/1/1.png" => Ok(response(404, &[], b"")),
        "https://tile.openstreetmap.org/1/0/1.png" => Ok(response(500, &[], b"")),
        "https://tile.openstreetmap.org/1/1/0.png" => {
            Err(GeoError::Network("接続できません".to_owned()))
        }
        "https://tile.openstreetmap.org/2/0/0.png" => Ok(response(
            200,
            &[("content-type", "text/html")],
            b"<html></html>",
        )),
        other => panic!("思わない要求: {other}"),
    });
    let state = Arc::new(state_with_fake(&env, &fake));
    let get = |path: &str| protocol::reply(&state, path, None);

    // 無効なら 403 で、通信しない。カタログを開いていなくても 503 にしない（サムネイルは 503）。
    assert_eq!(get("/tile/1/0/0").status, 403);
    assert_eq!(get("/thumb/1").status, 503);
    // 形の違う URI・範囲外の番号は 400。
    for bad in ["/tile/1/2/0", "/tile/1/0/0.png", "/tile/1/0", "/tile/x/0/0"] {
        assert_eq!(get(bad).status, 400, "{bad}");
    }
    assert_eq!(fake.count(), 0);

    enable(&state);
    let r = get("/tile/1/0/0");
    assert_eq!(
        (r.status, r.content_type, r.cache_control),
        (200, "image/png", "no-cache")
    );
    assert_eq!(r.body, PNG);
    assert_eq!(fake.urls(), ["https://tile.openstreetmap.org/1/0/0.png"]);
    // 2 回目はキャッシュから返す（期限内なので通信しない）。
    assert_eq!(get("/tile/1/0/0").body, PNG);
    assert_eq!(fake.count(), 1);
    // 提供元の最大のズーム（OSM は 19）を超えるズームは、通信せずに 400。
    assert_eq!(get("/tile/20/0/0").status, 400);
    assert_eq!(fake.count(), 1);
    assert_eq!(get("/tile/1/1/1").status, 404);
    assert_eq!(
        get("/tile/1/0/1").status,
        502,
        "上流のエラー（キャッシュなし）"
    );
    assert_eq!(get("/tile/1/1/0").status, 502, "通信の失敗");
    assert_eq!(get("/tile/2/0/0").status, 502, "PNG でない応答");
    assert_eq!(fake.count(), 5);

    // 画面の要求の経路（handle → タイル専用のスレッド）でも同じ答え。
    let (tx, rx) = crossbeam_channel::unbounded();
    for tile in [(1, 0, 0), (1, 1, 1)] {
        let tx = tx.clone();
        protocol::submit_tile(
            Arc::clone(&state),
            tile,
            Box::new(move |r| {
                let _ = tx.send((tile, r.status, r.body));
            }),
        );
    }
    let mut replies: Vec<_> = (0..2)
        .map(|_| rx.recv_timeout(WAIT_LIMIT).expect("タイルの答え"))
        .collect();
    replies.sort();
    assert_eq!(
        replies,
        [((1, 0, 0), 200, PNG.to_vec()), ((1, 1, 1), 404, Vec::new())]
    );
    assert_eq!(fake.count(), 6, "404 はキャッシュしない");

    // キャッシュの大きさ（本文と付随の情報）。消すのは専用のフォルダの中の提供元のフォルダだけ（Windows では、
    // 親のフォルダに WebView2 のデータとログがある）。
    let info = map::cache_info(&state);
    assert_eq!(info.files, 2, "{info:?}");
    assert!(info.bytes > PNG.len() as u64, "{info:?}");
    let parent = env.root().join("キャッシュ");
    let tiles_dir = parent.join(map::MAP_CACHE_DIR);
    std::fs::write(parent.join("WebView2 のデータ.txt"), b"x").unwrap();
    std::fs::write(tiles_dir.join("メモ.txt"), b"x").unwrap();
    assert_eq!(map::clear_cache(&state).unwrap(), CacheInfo::default());
    assert!(parent.join("WebView2 のデータ.txt").exists());
    assert!(tiles_dir.join("メモ.txt").exists());
    // 消した後は取り直す。
    assert_eq!(get("/tile/1/0/0").status, 200);
    assert_eq!(fake.count(), 7);

    // 提供元を設定で変えると、次の要求からその URL に送る（キャッシュは提供元ごとのフォルダに分ける）。
    map::save_preferences(
        &state,
        MapPreferences {
            online: true,
            tile_url: Some("https://tiles.example.org/{z}/{x}/{y}.png".to_owned()),
            ..MapPreferences::default()
        },
    )
    .unwrap();
    assert_eq!(get("/tile/1/0/0").status, 200);
    assert_eq!(
        fake.urls().last().map(String::as_str),
        Some("https://tiles.example.org/1/0/0.png")
    );
    assert_eq!(map::cache_info(&state).files, 4);

    // 無効に戻すと、キャッシュにあっても 403 で、通信しない（キャッシュの大きさは無効でも返す）。
    map::save_preferences(&state, MapPreferences::default()).unwrap();
    assert_eq!(get("/tile/1/0/0").status, 403);
    assert_eq!(fake.count(), 8);
    assert_eq!(map::cache_info(&state).files, 4);
}
