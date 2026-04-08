//! Custom `ServerConnector` for tokio-xmpp 5.
//!
//! Why this exists: tokio-xmpp 5.0.0 ships exactly one StartTLS connector
//! (`tokio_xmpp::connect::starttls::StartTlsServerConnector`) which is
//! literally `pub struct StartTlsServerConnector(pub DnsConfig)`. It carries
//! no rustls configuration of its own — TLS is built deep inside
//! `connect::tls_common::establish_tls_connection`, which constructs its
//! `ClientConfig` from feature-gated trust stores (`webpki-roots` and/or
//! `rustls-native-certs`) with no per-call override. There is no public hook
//! to inject a custom `ServerCertVerifier`, no env knob, and no builder.
//!
//! Our chat server (Prosody on `chat.taile0fb4.ts.net`, behind Tailscale
//! Serve TCP passthrough) presents a self-signed cert. The tokio-xmpp
//! defaults reject it unconditionally.
//!
//! The escape hatch is `Client::new_with_connector<C: ServerConnector>`,
//! which accepts any implementation of the `ServerConnector` trait. So we
//! mirror what `StartTlsServerConnector::connect` does — TCP, plaintext
//! `<stream>` open, `<starttls/>` negotiation, then the TLS handshake — but
//! we slot in our own `Arc<ClientConfig>` at the handshake step. Spike
//! verified this end-to-end against Prosody on mini (DM connect, presence,
//! MUC join, groupchat send, all green) before this code landed in
//! companion-core. The spike crate is preserved at
//! `~/.local/share/axios-companion/workspace/xmpp-spike` for future
//! debugging.

use std::borrow::Cow;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use sasl::common::ChannelBinding;
use tokio::io::BufStream;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_xmpp::connect::{DnsConfig, ServerConnector};
use tokio_xmpp::error::ProtocolError;
use tokio_xmpp::jid::Jid;
use tokio_xmpp::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_xmpp::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_xmpp::rustls::{
    ClientConfig, DigitallySignedStruct, ProtocolVersion, SignatureScheme,
};
use tokio_xmpp::xmlstream::{
    initiate_stream, PendingFeaturesRecv, ReadError, StreamHeader, Timeouts, XmppStream,
    XmppStreamElement,
};
use tokio_xmpp::Error as TokioXmppError;
use xmpp_parsers::starttls::{Nonza as StarttlsNonza, Request as StarttlsRequest};

// ---------------------------------------------------------------------------
// NoVerify — accept every server cert. Used when XmppConfig::tls_verify is
// false. This is the standard rustls escape hatch (`.dangerous()...`) and
// the same one ZeroClaw used to talk to its self-signed Prosody. Honest
// trade-off: we trust whatever the host route delivers. Acceptable here
// because the only path to chat is through Tailscale (encrypted at the
// wireguard layer) or loopback (never leaves the box).
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, tokio_xmpp::rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_xmpp::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_xmpp::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        tokio_xmpp::rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Build the rustls ClientConfig used for the StartTLS handshake.
///
/// Currently this always installs [`NoVerify`] — every server cert is
/// accepted. That matches our only deployment target (self-signed Prosody
/// reachable only over loopback or wireguard). When real certs land we add
/// a verified branch here and a `tls_verify` field on `XmppConfig` in the
/// same commit, so the operator is never able to ask for verification we
/// don't actually do.
pub(super) fn build_tls_config() -> Arc<ClientConfig> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify))
        .with_no_client_auth();
    Arc::new(config)
}

// ---------------------------------------------------------------------------
// Connector — implements ServerConnector by mirroring upstream's
// StartTlsServerConnector but with our own ClientConfig at the TLS step.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) struct Connector {
    pub dns_config: DnsConfig,
    pub tls_config: Arc<ClientConfig>,
}

impl ServerConnector for Connector {
    type Stream = BufStream<tokio_rustls::client::TlsStream<TcpStream>>;

    async fn connect(
        &self,
        jid: &Jid,
        ns: &'static str,
        timeouts: Timeouts,
    ) -> Result<(PendingFeaturesRecv<Self::Stream>, ChannelBinding), TokioXmppError> {
        // Phase 1: TCP + plaintext stream open.
        let tcp_stream = BufStream::new(self.dns_config.resolve().await?);
        let xmpp_stream = initiate_stream(
            tcp_stream,
            ns,
            StreamHeader {
                to: Some(Cow::Borrowed(jid.domain().as_str())),
                from: None,
                id: None,
            },
            timeouts,
        )
        .await?;

        let (features, mut xmpp_stream) = xmpp_stream.recv_features().await?;
        if !features.can_starttls() {
            return Err(TokioXmppError::Protocol(ProtocolError::NoTls));
        }

        // Phase 2: <starttls/> negotiation.
        xmpp_stream
            .send(&XmppStreamElement::Starttls(StarttlsNonza::Request(
                StarttlsRequest,
            )))
            .await?;

        loop {
            match xmpp_stream.next().await {
                Some(Ok(XmppStreamElement::Starttls(StarttlsNonza::Proceed(_)))) => break,
                Some(Ok(_)) => continue,
                Some(Err(ReadError::SoftTimeout)) => continue,
                Some(Err(ReadError::HardError(e))) => return Err(e.into()),
                Some(Err(ReadError::ParseError(e))) => {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e).into());
                }
                None | Some(Err(ReadError::StreamFooterReceived)) => {
                    return Err(TokioXmppError::Disconnected);
                }
            }
        }

        // Phase 3: TLS handshake using our own config.
        let inner: TcpStream = unwrap_to_tcp(xmpp_stream);
        let domain = ServerName::try_from(jid.domain().as_str().to_owned())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let connector = TlsConnector::from(self.tls_config.clone());
        let tls_stream = connector
            .connect(domain, inner)
            .await
            .map_err(TokioXmppError::Io)?;

        // Channel binding extraction (TLS 1.3 only — same shape as upstream).
        let (_, conn) = tls_stream.get_ref();
        let channel_binding = match conn.protocol_version() {
            Some(ProtocolVersion::TLSv1_3) => {
                let data = vec![0u8; 32];
                let data = conn
                    .export_keying_material(data, b"EXPORTER-Channel-Binding", None)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                ChannelBinding::TlsExporter(data)
            }
            _ => ChannelBinding::None,
        };

        // Phase 4: open a fresh XMPP stream over TLS.
        let pending = initiate_stream(
            BufStream::new(tls_stream),
            ns,
            StreamHeader {
                to: Some(Cow::Borrowed(jid.domain().as_str())),
                from: None,
                id: None,
            },
            timeouts,
        )
        .await?;

        Ok((pending, channel_binding))
    }
}

/// Strip XmppStream + BufStream layers to get back the raw TcpStream.
/// Mirrors `stream.into_inner().into_inner()` in upstream's
/// `connect/starttls.rs::starttls`.
fn unwrap_to_tcp(stream: XmppStream<BufStream<TcpStream>>) -> TcpStream {
    stream.into_inner().into_inner()
}
