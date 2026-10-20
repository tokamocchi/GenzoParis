//! 本物の HTTP のクライアント（[`UreqClient`]）を手元のサーバーで確かめる。

use super::server::{LoopbackServer, response};
use crate::{GeoError, HttpClient, UreqClient, user_agent};

#[test]
fn user_agent_names_the_app_and_its_repository() {
    assert_eq!(
        user_agent(),
        format!(
            "GenzoParis/{} (+https://github.com/tokamocchi/GenzoParis)",
            env!("CARGO_PKG_VERSION")
        )
    );
}

#[test]
fn requests_send_our_user_agent_and_no_cache_busting_headers() {
    let server =
        LoopbackServer::start(|_| response("200 OK", &[("Content-Type", "image/png")], b"png"));
    let client = UreqClient::for_test(true);
    let r = client
        .get(
            &server.url("/12/3638/1612.png"),
            &[("If-None-Match", "\"abc\"".to_string())],
            1024,
        )
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"png");
    assert_eq!(r.header("content-type"), Some("image/png"));
    assert_eq!(r.header("Content-Type"), Some("image/png"));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.line, "GET /12/3638/1612.png HTTP/1.1");
    assert_eq!(req.header("user-agent"), Some(user_agent()));
    assert_eq!(req.header("if-none-match"), Some("\"abc\""));
    // ポリシー: no-cache の類を送らない。圧縮は読めないので Accept-Encoding も送らない。
    for name in [
        "cache-control",
        "pragma",
        "accept-encoding",
        "referer",
        "cookie",
    ] {
        assert_eq!(req.header(name), None, "{name}");
    }
}

#[test]
fn non_success_statuses_are_returned_as_responses() {
    let server = LoopbackServer::start(|req| match req.target() {
        "/missing" => response("404 Not Found", &[], b"no"),
        "/not-modified" => response("304 Not Modified", &[("ETag", "\"v1\"")], b""),
        _ => response("503 Service Unavailable", &[], b""),
    });
    let client = UreqClient::for_test(true);
    assert_eq!(
        client
            .get(&server.url("/missing"), &[], 1024)
            .unwrap()
            .status,
        404
    );
    let r = client.get(&server.url("/not-modified"), &[], 1024).unwrap();
    assert_eq!(r.status, 304);
    assert_eq!(r.header("etag"), Some("\"v1\""));
    assert!(r.body.is_empty());
    assert_eq!(
        client.get(&server.url("/busy"), &[], 1024).unwrap().status,
        503
    );
}

#[test]
fn repeated_headers_are_kept_and_can_be_joined() {
    let server = LoopbackServer::start(|_| {
        response(
            "200 OK",
            &[
                ("Cache-Control", "public"),
                ("Cache-Control", "max-age=600"),
            ],
            b"x",
        )
    });
    let r = UreqClient::for_test(true)
        .get(&server.url("/"), &[], 1024)
        .unwrap();
    assert_eq!(r.header("cache-control"), Some("public"));
    assert_eq!(
        r.header_list("cache-control").as_deref(),
        Some("public, max-age=600")
    );
    assert_eq!(r.header_list("expires"), None);
}

#[test]
fn bodies_over_the_limit_are_rejected() {
    let server = LoopbackServer::start(|_| response("200 OK", &[], &[7u8; 2000]));
    let client = UreqClient::for_test(true);
    let e = client.get(&server.url("/"), &[], 1000).unwrap_err();
    assert!(matches!(e, GeoError::BadResponse(_)), "{e:?}");
    assert!(e.to_string().contains("上限 1000 バイト"), "{e}");
    let e = client.get(&server.url("/"), &[], 1999).unwrap_err();
    assert!(matches!(e, GeoError::BadResponse(_)), "{e:?}");
    // 上限ちょうどは受け付ける。
    assert_eq!(
        client.get(&server.url("/"), &[], 2000).unwrap().body.len(),
        2000
    );
}

#[test]
fn redirects_are_followed() {
    let server = LoopbackServer::start(|req| match req.target() {
        "/old" => response("301 Moved Permanently", &[("Location", "/new")], b""),
        _ => response("200 OK", &[], b"moved"),
    });
    let r = UreqClient::for_test(true)
        .get(&server.url("/old"), &[], 1024)
        .unwrap();
    assert_eq!(r.body, b"moved");
    let targets: Vec<_> = server
        .requests()
        .iter()
        .map(|r| r.target().to_string())
        .collect();
    assert_eq!(targets, ["/old", "/new"]);
}

#[test]
fn the_real_client_never_sends_plain_http() {
    let server = LoopbackServer::start(|_| response("200 OK", &[], b"x"));
    // 本物と同じ設定（https だけ）。new() のプロキシの設定は環境で変わるので、プロキシなしの口で確かめる。
    let client = UreqClient::for_test(false);
    let e = client.get(&server.url("/"), &[], 1024).unwrap_err();
    assert!(matches!(e, GeoError::BadResponse(_)), "{e:?}");
    // 接続する前に止める。
    assert_eq!(server.connections(), 0);
}

#[test]
fn https_goes_through_rustls_and_failures_are_network_errors() {
    // このサーバーは TLS を話さない（TLS のハンドシェイクに平文で応える）ので、TLS の接続が失敗する。
    // TLS の部品がない・暗号の実装がないときのように panic しないこと、TLS を張ろうとすること
    // （平文の HTTP を送らないこと）を確かめる。
    let server = LoopbackServer::start(|_| response("400 Bad Request", &[], b"not tls"));
    let client = UreqClient::for_test(false);
    let e = client.get(&server.https_url("/"), &[], 1024).unwrap_err();
    let GeoError::Network(message) = &e else {
        panic!("{e:?}");
    };
    assert!(message.contains("TLS"), "{message}");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].tls,
        "TLS のハンドシェイクが届いていない: {requests:?}"
    );
}

#[test]
fn connection_failures_are_network_errors() {
    // 使っていないポート（いったん bind して閉じる）。
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let e = UreqClient::for_test(true)
        .get(&format!("http://127.0.0.1:{port}/"), &[], 1024)
        .unwrap_err();
    assert!(matches!(e, GeoError::Network(_)), "{e:?}");
}

#[test]
fn the_real_client_can_be_built() {
    // 準備できること（通信はしない）。`new` は環境変数・レジストリのプロキシを読み、読めない値では誤りにするので、
    // 開発者の環境に左右されないよう、プロキシなしで本物と同じ設定（https だけ）を作る。
    UreqClient::build(true, None).unwrap();
}

#[test]
fn tls_uses_rustls_with_ring_and_the_os_verifier() {
    let client = UreqClient::for_test(false);
    let tls = client.config().tls_config();
    assert_eq!(tls.provider(), ureq::tls::TlsProvider::Rustls);
    // 証明書の検証は OS に任せる（webpki-roots の同梱の証明書は使わない）。検証は切らない。
    assert!(
        matches!(tls.root_certs(), ureq::tls::RootCerts::PlatformVerifier),
        "{:?}",
        tls.root_certs()
    );
    assert!(!tls.disable_verification());
    // 作るときに、プロセスの既定の暗号の実装が入っている（ureq は、なければ panic する）。
    assert!(rustls::crypto::CryptoProvider::get_default().is_some());
}

#[test]
fn socks_proxies_are_refused_instead_of_connecting_directly() {
    let server = LoopbackServer::start(|_| response("200 OK", &[], b"x"));
    let proxy = ureq::Proxy::new("socks5://127.0.0.1:1").unwrap();
    let client = UreqClient::build(false, Some(proxy)).unwrap();
    let e = client.get(&server.url("/"), &[], 1024).unwrap_err();
    let GeoError::Network(message) = &e else {
        panic!("{e:?}");
    };
    assert!(message.contains("SOCKS"), "{message}");
    assert_eq!(server.connections(), 0);
}

#[test]
fn with_a_socks_proxy_no_proxy_hosts_are_reached_directly_without_following_redirects() {
    let server = LoopbackServer::start(|req| match req.target() {
        "/old" => response("301 Moved Permanently", &[("Location", "/new")], b""),
        _ => response("200 OK", &[], b"direct"),
    });
    let proxy = ureq::Proxy::builder(ureq::ProxyProtocol::Socks5)
        .host("127.0.0.1")
        .port(1)
        .no_proxy("127.0.0.1")
        .build()
        .unwrap();
    let client = UreqClient::build(false, Some(proxy)).unwrap();
    assert_eq!(
        client.get(&server.url("/tile"), &[], 1024).unwrap().body,
        b"direct"
    );
    // 転送はたどらない（転送先が NO_PROXY の外なら、SOCKS を通さずに直接つないでしまうため）。
    let r = client.get(&server.url("/old"), &[], 1024).unwrap();
    assert_eq!(r.status, 301);
    let targets: Vec<_> = server
        .requests()
        .iter()
        .map(|r| r.target().to_string())
        .collect();
    assert_eq!(targets, ["/tile", "/old"]);
}
