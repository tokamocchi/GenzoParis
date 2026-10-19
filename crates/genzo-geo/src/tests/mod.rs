//! genzo-geo の結合テスト（テスト用の口（`UreqClient::for_test` など）を使うため、crate の中に置く）。
//!
//! 本物の OSM のサーバーには問い合わせない。手元の `TcpListener` の小さなサーバー（[`server`]）か、
//! [`crate::HttpClient`] の差し替えで確かめる。

mod http;
mod server;
