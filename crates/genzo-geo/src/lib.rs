//! オンラインの地図の通信（位置情報の後付け LIB-21 の第 2 段階）。
//!
//! 写真に位置を付けるダイアログの地図（OpenStreetMap のタイル）と地名の検索（Nominatim）のために、
//! HTTPS で外部のサーバーに問い合わせる。外部と通信するのはこの crate だけで、使うのは genzo-app だけ
//! （CLI とコア API は通信しない。依存の向きは genzo-app → genzo-geo → genzo-model。
//! docs/04_architecture.md の 3.16 節）。オンラインの地図は既定で無効で、利用者が明示的に有効にしたときだけ
//! genzo-app がこの crate を呼ぶ（docs/02 の SEC-01 の例外と SEC-02。有効かどうかは genzo-app の設定が持つ）。
//!
//! - HTTP: [`HttpClient`]（テストで差し替える trait）と [`UreqClient`]（ureq と native-tls による本物の実装。
//!   TLS は OS の実装と証明書の保管場所を使う）。User-Agent は [`user_agent`]
//! - 提供元: [`TileProvider`]・[`GeocoderProvider`]（既定は OSM Foundation のサーバー。設定で変えられる。https だけ）
//! - タイル: [`TileService`]（ディスクのキャッシュ。HTTP のキャッシュの指示に従い、読めなければ 7 日。期限切れは
//!   条件付きの要求。同じタイルの同時の要求は 1 つにまとめる）
//! - 地名の検索: [`Geocoder`]（1 秒に 1 回まで。結果はキャッシュする。jsonv2）
//! - 時計: [`Clock`]（テストで差し替える）
//!
//! 守ること（OSM Foundation の利用ポリシー。タイルは <https://operations.osmfoundation.org/policies/tiles/> 、
//! Nominatim は <https://operations.osmfoundation.org/policies/nominatim/> 。原文はそのページの元の GitHub の
//! ソースで確かめた。公開ページは未確認で、人の確認待ち）:
//! - アプリを識別できる、変わらない User-Agent を送る（ライブラリの既定の User-Agent は使わない）。
//! - `Cache-Control: no-cache`・`Pragma: no-cache` などを送らない。
//! - タイルは HTTP のキャッシュの指示に従ってキャッシュし（読めなければ少なくとも 7 日）、期限切れは
//!   条件付きの要求にする。先読み・一括取得・オフライン用の取得の機能は作らない。
//! - Nominatim は 1 秒に 1 回まで（アプリ全体の合計）。結果はキャッシュする。オートコンプリートは作らない
//!   （画面は Enter かボタンのときだけ検索する）。
//! - 提供元はソフトウェアの更新なしに切り替えられる（genzo-app の設定の URL）。
//! - 帰属の表示（[`OSM_ATTRIBUTION`]）は画面の地図が常に出す。
//!
//! テストは本物の OSM のサーバーに問い合わせない（手元の `TcpListener` の小さなサーバーか、
//! [`HttpClient`] の差し替えで確かめる）。

#![forbid(unsafe_code)]

mod clock;
mod error;
mod freshness;
mod geocode;
mod http;
mod provider;
mod tiles;
mod tls;

#[cfg(test)]
mod tests;

pub use clock::{Clock, SystemClock};
pub use error::GeoError;
pub use geocode::{
    Geocoder, MAX_QUERY_CHARS, MAX_SEARCH_BYTES, MIN_REQUEST_INTERVAL, Place, RESULT_CACHE_ENTRIES,
    RESULT_LIMIT, SEARCH_LANGUAGE,
};
pub use http::{CONNECT_TIMEOUT, HttpClient, HttpResponse, TOTAL_TIMEOUT, UreqClient, user_agent};
pub use provider::{
    GeocoderProvider, MAX_ZOOM_LIMIT, NOMINATIM_SEARCH_URL, OSM_ATTRIBUTION, OSM_MAX_ZOOM,
    OSM_TILE_URL, TileProvider, validate_search_url, validate_tile_url_template,
};
pub use tiles::{
    CacheInfo, DEFAULT_CACHE_LIMIT_BYTES, EVICTION_CHECK_EVERY, MAX_TILE_BYTES, TILE_CONTENT_TYPE,
    Tile, TileService,
};
