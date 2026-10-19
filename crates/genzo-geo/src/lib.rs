//! オンラインの地図の通信（位置情報の後付け LIB-21 の第 2 段階）。
//!
//! 写真に位置を付けるダイアログの地図（OpenStreetMap のタイル）と地名の検索（Nominatim）のために、
//! HTTPS で外部のサーバーに問い合わせる。外部と通信するのはこの crate だけで、使うのは genzo-app だけ
//! （CLI とコア API は通信しない。依存の向きは genzo-app → genzo-geo。docs/04_architecture.md の 3.16 節）。
//! オンラインの地図は既定で無効で、利用者が明示的に有効にしたときだけ genzo-app がこの crate を呼ぶ
//! （docs/02 の SEC-01 の例外と SEC-02。有効かどうかは genzo-app の設定が持つ）。
//!
//! - HTTP: [`HttpClient`]（テストで差し替える trait）と [`UreqClient`]（ureq と native-tls による本物の実装。
//!   TLS は OS の実装と証明書の保管場所を使う）。User-Agent は [`user_agent`]
//! - 提供元: [`TileProvider`]・[`GeocoderProvider`]（既定は OSM Foundation のサーバー。設定で変えられる。https だけ）
//!
//! 守ること（OSM Foundation の利用ポリシー。タイルは https://operations.osmfoundation.org/policies/tiles/ 、
//! Nominatim は https://operations.osmfoundation.org/policies/nominatim/ 。原文はそのページの元の GitHub の
//! ソースで確かめた。公開ページは未確認で、人の確認待ち）:
//! - アプリを識別できる、変わらない User-Agent を送る（ライブラリの既定の User-Agent は使わない）。
//! - `Cache-Control: no-cache`・`Pragma: no-cache` などを送らない。
//!
//! テストは本物の OSM のサーバーに問い合わせない（手元の `TcpListener` の小さなサーバーか、
//! [`HttpClient`] の差し替えで確かめる）。

#![forbid(unsafe_code)]

mod error;
mod http;
mod provider;
mod tls;

#[cfg(test)]
mod tests;

pub use error::GeoError;
pub use http::{CONNECT_TIMEOUT, HttpClient, HttpResponse, TOTAL_TIMEOUT, UreqClient, user_agent};
pub use provider::{
    GeocoderProvider, MAX_ZOOM_LIMIT, NOMINATIM_SEARCH_URL, OSM_ATTRIBUTION, OSM_MAX_ZOOM,
    OSM_TILE_URL, TileProvider, validate_search_url, validate_tile_url_template,
};
