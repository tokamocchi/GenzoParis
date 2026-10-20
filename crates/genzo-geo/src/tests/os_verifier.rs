//! OS による証明書の検証（rustls-platform-verifier の [`Verifier`]。ureq の `RootCerts::PlatformVerifier` が
//! 使うもの）が、実際に証明書を確かめることを確かめる。Windows は CryptoAPI、macOS は Security.framework、Linux は
//! OS の証明書の保管場所と rustls-webpki で確かめる（CI の test は windows-latest・macos-latest で動く）。
//! [`super::http`] の `tls_uses_rustls_with_ring_and_the_os_verifier` は設定を見るだけ。
//!
//! 外部のネットワークは使わない（証明書を検証の関数に直接渡す。TLS の接続はしない）。

use std::sync::Arc;
use std::time::Duration;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, RootCertStore};
use rustls_platform_verifier::Verifier;

// テストの資料の証明書（公開鍵の証明書だけ。秘密鍵は作ってすぐ消し、リポジトリには入れていない）。
// 自己署名で、CN・SAN（DNS）は localhost。鍵は EC P-256、署名は ecdsa-with-SHA256。基本制約は CA:FALSE、
// 鍵の用途は digitalSignature、拡張鍵の用途は serverAuth。有効期間は 2026-10-20T02:57:14Z〜2027-10-20T02:57:14Z
// （365 日。macOS の TLS のサーバーの証明書の上限 398 日より短い）。SHA-256 の指紋は
// C0:38:A6:0A:29:86:2C:E3:5A:85:76:25:71:92:FC:11:C7:43:70:DB:06:83:2D:F1:85:42:00:02:7C:CD:B9:65。
// 発行者（= 主体）の名前に O を足して、OS の証明書の保管場所の証明書と名前が重ならないようにしている（CN = localhost
// だけだと、Ubuntu の ssl-cert の snakeoil の証明書（/etc/ssl/certs。CN はホスト名）と重なることがある。2026-10-20 に
// 開発のコンテナ（snakeoil の CN が localhost）で、webpki がそれを発行者の候補にし、UnknownIssuer でなく署名の方式の
// 誤り（UnsupportedSignatureAlgorithmForPublicKeyContext）になった）。
// 作り方（OpenSSL 3.0.13）:
//   openssl req -x509 -new -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout key.pem \
//     -sha256 -days 365 -config req.cnf -out cert.pem
// req.cnf は [req] に distinguished_name = dn・prompt = no・x509_extensions = ext、[dn] に CN = localhost・
// O = GenzoParis genzo-geo test (untrusted)、[ext] に basicConstraints = critical, CA:FALSE・
// keyUsage = critical, digitalSignature・extendedKeyUsage = serverAuth・subjectAltName = DNS:localhost・
// subjectKeyIdentifier = hash。
const SELF_SIGNED_LOCALHOST_PEM: &str = "\
-----BEGIN CERTIFICATE-----
MIIB9DCCAZqgAwIBAgIUVrg2z1jNwgD+SZIgNjBvD56t2h8wCgYIKoZIzj0EAwIw
RDESMBAGA1UEAwwJbG9jYWxob3N0MS4wLAYDVQQKDCVHZW56b1BhcmlzIGdlbnpv
LWdlbyB0ZXN0ICh1bnRydXN0ZWQpMB4XDTI2MTAyMDAyNTcxNFoXDTI3MTAyMDAy
NTcxNFowRDESMBAGA1UEAwwJbG9jYWxob3N0MS4wLAYDVQQKDCVHZW56b1Bhcmlz
IGdlbnpvLWdlbyB0ZXN0ICh1bnRydXN0ZWQpMFkwEwYHKoZIzj0CAQYIKoZIzj0D
AQcDQgAEuqlGlIUQWZbVlm7Hbz7RlwgctsncBfXziejZ9eNPNwSVQtuZwa5BLH2u
YsZroDPfBZYjfqRBoITjt2ivRPbjQKNqMGgwDAYDVR0TAQH/BAIwADAOBgNVHQ8B
Af8EBAMCB4AwEwYDVR0lBAwwCgYIKwYBBQUHAwEwFAYDVR0RBA0wC4IJbG9jYWxo
b3N0MB0GA1UdDgQWBBQhaNOOoqW7pCjMFDea0xgRtc3pCDAKBggqhkjOPQQDAgNI
ADBFAiA9rxuwNVBtT+RsU9OE0BpPyltNGwE9K94IvD2DL2QHHAIhAN7M5bBZqhxa
R8rJyGbPttL9sYxnFWybyw2OhEPiiJ5S
-----END CERTIFICATE-----
";

/// 検証の時刻（2027-04-20T00:00:00Z。証明書の有効期間の中の固定の時刻）。
fn verification_time() -> UnixTime {
    UnixTime::since_unix_epoch(Duration::from_secs(1_808_179_200))
}

#[test]
fn the_os_verifier_rejects_a_self_signed_certificate() {
    let cert = CertificateDer::from_pem_slice(SELF_SIGNED_LOCALHOST_PEM.as_bytes()).unwrap();
    let name = ServerName::try_from("localhost").unwrap();
    // genzo-geo と同じ暗号の実装（ring）。ureq は、プロセスの既定の実装（UreqClient を作るときに ring を入れる）を
    // Verifier に渡す。
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    // 資料の確認: この証明書を信頼するなら、形・名前・時刻では拒まれない（下で OS が拒むのは、信頼していないため）。
    let mut roots = RootCertStore::empty();
    roots.add(cert.clone()).unwrap();
    WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .unwrap()
        .verify_server_cert(&cert, &[], &name, &[], verification_time())
        .unwrap();

    let verifier = Verifier::new(provider).unwrap();
    let e = verifier
        .verify_server_cert(&cert, &[], &name, &[], verification_time())
        .unwrap_err();
    let rustls::Error::InvalidCertificate(reason) = &e else {
        panic!("{e:?}");
    };
    // Windows（CERT_E_UNTRUSTEDROOT）と Linux（webpki）は UnknownIssuer。macOS は、Security.framework の誤りの
    // うち rustls-platform-verifier 0.7.1 が写すもの（errSecCreateChainFailed）なら UnknownIssuer、写さないもの
    // （自己署名で信頼されていないときの errSecNotTrusted など）なら、OS の説明と番号を持つ Other になる。
    #[cfg(not(target_vendor = "apple"))]
    assert!(matches!(reason, CertificateError::UnknownIssuer), "{e:?}");
    #[cfg(target_vendor = "apple")]
    assert!(
        matches!(
            reason,
            CertificateError::UnknownIssuer | CertificateError::Other(_)
        ),
        "{e:?}"
    );
}
