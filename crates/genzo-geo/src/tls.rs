//! native-tls による TLS の接続の部品（ureq の [`Connector`]）。
//!
//! TLS は OS の実装と証明書の保管場所を使う（Windows は SChannel、macOS は Security.framework、Linux などは
//! OpenSSL。native-tls が OS ごとに選ぶ）。ureq 3.4.2 の TLS の機能はどれも使えないため、ureq の
//! unversioned の API（`ureq::unversioned::transport`。semver の約束の外）で、ここに部品を組む:
//! - 機能 rustls は ring と webpki-roots（CDLA-Permissive-2.0。deny.toml で許していない）を入れる。
//! - 機能 native-tls は webpki-root-certs（同じく CDLA-Permissive-2.0）を必ず入れる。
//! - 機能 native-tls-no-default は native-tls の crate を入れるだけで、ureq の接続の部品
//!   （`NativeTlsConnector`）が機能 native-tls の cfg の中にあるため、https の要求が
//!   「uri scheme is https, provider is NativeTls but feature is not enabled」で panic する
//!   （2026-10-19 にこのリポジトリで確かめた）。
//!
//! 流れは ureq の `NativeTlsConnector`（ureq の src/tls/native_tls.rs）と同じ。違い:
//! - 証明書は OS の保管場所だけで確かめる（native-tls の既定。ルート証明書を足さない。検証は切らない）。
//! - TLS の最低の版は 1.2（native-tls 0.2.18 の既定と同じだが、版を上げても変わらないよう明示する）。
//! - クライアント証明書は使わない。
//! - ureq の `ErrorCapture`（ハンドシェイクの途中の下の層の誤りを取り出す）は持たない。ハンドシェイクの
//!   途中のタイムアウトも TLS の誤りとして返る（どちらも [`crate::GeoError::Network`] になるので、違いは
//!   メッセージだけ）。
//!
//! ureq を上げるときは、unversioned の API（[`Connector`]・[`Transport`]・[`TransportAdapter`]・
//! [`LazyBuffers`] など）が変わっていないかを確かめる。

use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;

use native_tls::{HandshakeError, Protocol, TlsConnector, TlsStream};
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, Either, LazyBuffers, NextTimeout, Transport,
    TransportAdapter,
};

/// https の接続を TLS で包む部品。http の接続と、すでに TLS の接続はそのまま次へ渡す。
pub(crate) struct NativeTlsConnector {
    connector: Arc<TlsConnector>,
}

impl NativeTlsConnector {
    /// OS の TLS の実装を準備する。
    pub(crate) fn new() -> Result<Self, native_tls::Error> {
        let connector = TlsConnector::builder()
            .min_protocol_version(Some(Protocol::Tlsv12))
            .build()?;
        Ok(Self {
            connector: Arc::new(connector),
        })
    }
}

impl fmt::Debug for NativeTlsConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeTlsConnector").finish_non_exhaustive()
    }
}

impl<In: Transport> Connector<In> for NativeTlsConnector {
    type Out = Either<In, NativeTlsTransport>;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        let Some(transport) = chained else {
            // 前の部品が接続を作らなかった（ureq の部品は panic するが、ここではそのまま渡す。
            // 接続がないことは ureq が誤りにする）。
            return Ok(None);
        };
        // CONNECT のプロキシのトンネルは is_tls() が false なので、対象のサーバーとの TLS はここで張る。
        if !details.needs_tls() || transport.is_tls() {
            return Ok(Some(Either::A(transport)));
        }
        let host = details
            .uri
            .host()
            .ok_or_else(|| ureq::Error::BadUri("ホスト名がありません".to_string()))?;
        // IPv6 のアドレスは [] を外す（証明書の名前の照合と SNI のため。ureq の host_bare と同じ）。
        let domain = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);

        let mut adapter = TransportAdapter::new(Box::new(transport) as Box<dyn Transport>);
        adapter.set_timeout(details.timeout);
        let stream = match self.connector.connect(domain, adapter) {
            Ok(stream) => stream,
            Err(HandshakeError::Failure(e)) => {
                return Err(ureq::Error::Other(Box::new(TlsFailure(e))));
            }
            // 下の接続は待つ形（ブロッキング）なので、普通は起きない。
            Err(HandshakeError::WouldBlock(_)) => {
                return Err(ureq::Error::Io(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "TLS のハンドシェイクが途中で止まりました",
                )));
            }
        };
        let buffers = LazyBuffers::new(
            details.config.input_buffer_size(),
            details.config.output_buffer_size(),
        );
        Ok(Some(Either::B(NativeTlsTransport { buffers, stream })))
    }
}

/// TLS で包んだ接続。
pub(crate) struct NativeTlsTransport {
    buffers: LazyBuffers,
    stream: TlsStream<TransportAdapter>,
}

impl fmt::Debug for NativeTlsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeTlsTransport").finish_non_exhaustive()
    }
}

impl Transport for NativeTlsTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.stream.get_mut().set_timeout(timeout);
        let output = &self.buffers.output()[..amount];
        // 下の層の ureq の誤り（タイムアウトなど）は io::Error に包まれて戻り、From で取り出される。
        self.stream.write_all(output)?;
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.stream.get_mut().set_timeout(timeout);
        let input = self.buffers.input_append_buf();
        let amount = self.stream.read(input)?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        self.stream.get_mut().get_mut().is_open()
    }

    fn is_tls(&self) -> bool {
        true
    }
}

/// TLS のハンドシェイクの失敗（証明書の検証の失敗を含む）。
#[derive(Debug)]
pub(crate) struct TlsFailure(native_tls::Error);

impl fmt::Display for TlsFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TLS の接続に失敗しました（証明書の検証を含む）: {}",
            self.0
        )
    }
}

impl std::error::Error for TlsFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
