//! HTTP の取得（テストで差し替える trait と、ureq と rustls による本物の実装）。
//!
//! 送るもの・送らないもの（OSM Foundation のタイルと Nominatim の利用ポリシー）:
//! - User-Agent は必ず [`user_agent`]（アプリの名前・版・連絡先の URL。ライブラリの既定の `ureq/<版>` は
//!   使わない。ポリシーでは、ライブラリの既定の User-Agent はブロックされる）。利用者の個人の情報は入れない。
//! - `Cache-Control: no-cache`・`Pragma: no-cache` などは送らない（ureq は自分では足さない。呼び出し元も
//!   足さない。テストで確かめる）。
//! - `Accept-Encoding` は送らない（ureq の gzip の機能を入れていないので、圧縮された本文を読めないため）。
//! - Referer は送らない（ポリシーでは、ネイティブのアプリに Referer がないのはかまわない）。
//!
//! 本物の実装 [`UreqClient`] のきまり:
//! - https だけ（転送（リダイレクト）先も。ureq の `https_only`）。転送の回数は ureq の既定（10 回）。
//! - TLS は ureq の公開の設定（`TlsConfig`）で rustls を選ぶ。暗号の実装は ring（[`UreqClient`] を作るときに
//!   プロセスの既定として入れる。ほかの部品が先に別の実装を入れていたら、それを使う）。証明書の検証は OS に任せる
//!   （ureq の `RootCerts::PlatformVerifier`。rustls-platform-verifier が、Windows は CryptoAPI、macOS は
//!   Security.framework、Linux は OS の証明書の保管場所で確かめる）。検証は切らない。TLS の版は 1.2 と 1.3
//!   （rustls が 1.1 以前を持たない）。ureq の unversioned の API は使わない。
//! - タイムアウトは接続 [`CONNECT_TIMEOUT`]・全体 [`TOTAL_TIMEOUT`]（どちらも仮置き）。
//! - 本文は呼び出し元が決めた上限まで読む（超えたら [`GeoError::BadResponse`]）。
//! - 状態コードが 2xx でなくても誤りにせず返す（304 や 404 を呼び出し元が見分けるため）。
//! - プロキシは環境変数（`HTTPS_PROXY`・`NO_PROXY` など）と、Windows ではレジストリの設定（ureq の機能
//!   win-system-proxy）に従う。macOS のシステムの設定のプロキシは読まない（ureq が対応していない）。SOCKS の
//!   プロキシは使えない（ureq の機能 socks-proxy を入れていない）。SOCKS が設定されていたら、直接つながずに
//!   誤りにする（ureq の既定は、環境変数の SOCKS を警告するだけで直接つなぐ。利用者がプロキシを通すつもりの
//!   通信を直接送らないため）。`NO_PROXY` に当たるホストへは直接つなぐが、そのときは転送をたどらない（転送先が
//!   `NO_PROXY` の外なら、SOCKS を通さずに直接つないでしまうため）。

use std::time::Duration;

use ureq::ProxyProtocol;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::GeoError;

/// 接続（名前の解決・TCP・TLS のハンドシェイク）のタイムアウト（仮置き）。
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 1 回の要求の全体（転送を含む）のタイムアウト（仮置き）。
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);

/// 外部のサーバーに送る User-Agent。
///
/// `GenzoParis/<版> (+<リポジトリの URL>)` の形（例 `GenzoParis/0.1.0 (+https://github.com/tokamocchi/GenzoParis)`）。
/// 版は workspace の版、URL は workspace の `repository`。ポリシーは、アプリの名前と（任意で）連絡先を含む、
/// 変わらない User-Agent を求める（版の番号は変えてよい）。
pub fn user_agent() -> &'static str {
    concat!(
        "GenzoParis/",
        env!("CARGO_PKG_VERSION"),
        " (+",
        env!("CARGO_PKG_REPOSITORY"),
        ")"
    )
}

/// HTTP の応答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// 状態コード。
    pub status: u16,
    /// ヘッダー（名前は小文字。同じ名前が複数あれば、届いた順にすべて）。
    pub headers: Vec<(String, String)>,
    /// 本文。
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// 名前（大文字と小文字は区別しない）が同じ最初のヘッダーの値。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 名前が同じヘッダーをすべて「, 」でつないだ値（`Cache-Control` のような、値を並べるヘッダー用。
    /// RFC 9110 の 5.3 節）。1 つもなければ `None`。
    pub fn header_list(&self, name: &str) -> Option<String> {
        let values: Vec<&str> = self
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect();
        (!values.is_empty()).then(|| values.join(", "))
    }
}

/// HTTP の GET（テストでは差し替える。本物は [`UreqClient`]）。
///
/// 実装は User-Agent を必ず [`user_agent`] にし、`no-cache` の類のヘッダーを足さないこと。
pub trait HttpClient: Send + Sync {
    /// `url` を GET する。`headers` は要求に足すヘッダー（名前, 値）。本文が `max_body_bytes` を超えたら
    /// [`GeoError::BadResponse`]。状態コードが 2xx でなくても誤りにせず返す。
    fn get(
        &self,
        url: &str,
        headers: &[(&str, String)],
        max_body_bytes: u64,
    ) -> Result<HttpResponse, GeoError>;
}

/// ureq と rustls による [`HttpClient`]。接続を使い回す（keep-alive）ので、1 つを共有して使う。
pub struct UreqClient {
    agent: ureq::Agent,
    /// SOCKS のプロキシが設定されているとき、そのプロキシ（ureq には渡さない。`NO_PROXY` に当たらないホストへの
    /// 要求は、つながずに誤りにする。モジュールの doc）。
    refused_socks: Option<ureq::Proxy>,
}

impl std::fmt::Debug for UreqClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UreqClient").finish_non_exhaustive()
    }
}

impl UreqClient {
    /// https だけを使う本物のクライアントを作る（プロキシは環境変数・Windows のレジストリから）。
    pub fn new() -> Result<Self, GeoError> {
        Self::build(true, ureq::Proxy::try_from_env())
    }

    /// テスト用: プロキシを使わない（手元の `TcpListener` のサーバーと話すため）。`allow_http` なら http の URL も使う。
    #[cfg(test)]
    pub(crate) fn for_test(allow_http: bool) -> Self {
        Self::build(!allow_http, None).expect("クライアントの準備")
    }

    /// テスト用: ureq の設定（TLS の設定を確かめるため）。
    #[cfg(test)]
    pub(crate) fn config(&self) -> &ureq::config::Config {
        self.agent.config()
    }

    /// `proxy` のプロキシを使う（`None` なら直接つなぐ）。
    pub(crate) fn build(https_only: bool, proxy: Option<ureq::Proxy>) -> Result<Self, GeoError> {
        install_crypto_provider();
        // SOCKS は ureq に渡さない（ureq の機能 socks-proxy を入れていないので、渡すと直接つなぐか panic する）。
        let (proxy, refused_socks) = match proxy {
            Some(p) if is_socks(p.protocol()) => (None, Some(p)),
            other => (other, None),
        };
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let mut config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .https_only(https_only)
            .user_agent(user_agent())
            // gzip の機能を入れていないので空（ヘッダーを送らない）。既定でも送らないが、明示する。
            .accept_encoding("")
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(TOTAL_TIMEOUT))
            .tls_config(tls)
            // ureq の既定（環境変数などから読んだもの）の代わりに、渡されたものを使う。
            .proxy(proxy);
        if refused_socks.is_some() {
            // NO_PROXY のホストへ直接つなぐとき、NO_PROXY の外への転送をたどらない（3xx をそのまま返す）。
            config = config.max_redirects(0);
        }
        Ok(Self {
            agent: config.build().new_agent(),
            refused_socks,
        })
    }

    /// SOCKS のプロキシが設定されていて、`url` のホストが `NO_PROXY` に当たらなければ誤り（つながない）。
    fn refuse_socks(&self, url: &str) -> Result<(), GeoError> {
        let Some(socks) = &self.refused_socks else {
            return Ok(());
        };
        let uri: ureq::http::Uri = url
            .parse()
            .map_err(|_| GeoError::InvalidInput("URL を読めません".to_string()))?;
        if socks.is_no_proxy(&uri) {
            return Ok(());
        }
        Err(GeoError::Network(
            "プロキシ経由で接続できませんでした: SOCKS のプロキシには対応していません（HTTP のプロキシを指定してください）"
                .to_string(),
        ))
    }
}

/// rustls のプロセスの既定の暗号の実装に ring を入れる（ureq の rustls は、`TlsConfig` で指定がなければ
/// プロセスの既定を使う）。すでに入っていれば（別の部品が先に入れた場合も）そのまま使う。
fn install_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // 同時に呼ばれて先を越されたときは Err になるが、どちらかの実装が入っていればよい。
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

fn is_socks(protocol: ProxyProtocol) -> bool {
    matches!(
        protocol,
        ProxyProtocol::Socks4
            | ProxyProtocol::Socks4A
            | ProxyProtocol::Socks5
            | ProxyProtocol::Socks5h
    )
}

impl HttpClient for UreqClient {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, String)],
        max_body_bytes: u64,
    ) -> Result<HttpResponse, GeoError> {
        self.refuse_socks(url)?;
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let mut response = request.call().map_err(map_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        // ureq の上限は「上限まで読んだ後にさらに読もうとしたら誤り」なので、上限ちょうどの本文も誤りになる。
        // 上限ちょうどまでを受け付けるよう 1 を足して渡す。
        let body = response
            .body_mut()
            .with_config()
            .limit(max_body_bytes.saturating_add(1))
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) => GeoError::BadResponse(format!(
                    "応答の本文が大きすぎます（上限 {max_body_bytes} バイト）"
                )),
                e => map_error(e),
            })?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// ureq の誤りを [`GeoError`] にする。URL（タイルの番号・検索語を含む）はメッセージに入れない。
fn map_error(e: ureq::Error) -> GeoError {
    use ureq::Error as E;
    match e {
        E::StatusCode(status) => GeoError::Status(status),
        E::BadUri(_) | E::Http(_) => GeoError::InvalidInput("URL を読めません".to_string()),
        E::RequireHttpsOnly(_) => {
            GeoError::BadResponse("https でない URL へは接続しません（転送先を含む）".to_string())
        }
        E::BodyExceedsLimit(_) => GeoError::BadResponse("応答の本文が大きすぎます".to_string()),
        E::TooManyRedirects => {
            GeoError::BadResponse("転送（リダイレクト）が多すぎます".to_string())
        }
        E::RedirectFailed => {
            GeoError::BadResponse("転送（リダイレクト）先を読めません".to_string())
        }
        E::Protocol(e) => GeoError::BadResponse(format!("HTTP の応答を読めません: {e}")),
        E::Timeout(_) => GeoError::Network("時間内に応答がありませんでした".to_string()),
        E::HostNotFound => GeoError::Network("サーバーの名前を解決できませんでした".to_string()),
        E::ConnectionFailed => GeoError::Network("サーバーに接続できませんでした".to_string()),
        E::InvalidProxyUrl => GeoError::Network("プロキシの設定を読めません".to_string()),
        E::ConnectProxyFailed(reason) => {
            GeoError::Network(format!("プロキシ経由で接続できませんでした: {reason}"))
        }
        E::TlsRequired => GeoError::Network("TLS で接続できませんでした".to_string()),
        E::Tls(reason) => GeoError::Network(format!("TLS で接続できませんでした: {reason}")),
        E::Rustls(e) => GeoError::Network(format!("TLS で接続できませんでした: {e}")),
        // TLS のハンドシェイクの失敗（証明書を確かめられない・TLS でない応答など）は、rustls の誤りを包んだ
        // 入出力のエラーで届く。
        E::Io(e) => match e
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        {
            Some(tls) => GeoError::Network(format!("TLS で接続できませんでした: {tls}")),
            None => GeoError::Network(format!("入出力のエラー: {e}")),
        },
        E::Other(e) => GeoError::Network(e.to_string()),
        other => GeoError::Network(other.to_string()),
    }
}
