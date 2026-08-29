//! Direct-first connector with QUIC/TLS and TCP/TLS relay fallback.

use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use firstshell_relay_protocol::{
    ClientHello, PROTOCOL_VERSION, ServerHello, read_frame, write_frame,
};
use rustls::pki_types::{CertificateDer, ServerName};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::{Zeroize, Zeroizing};

/// Transport selected for one established connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedTransport {
    /// Direct TCP reached the target without involving relayd.
    Direct,
    /// QUIC/TLS relay path.
    QuicTls,
    /// TCP/TLS relay fallback.
    TcpTls,
}

/// Direct and relay endpoints required for one connection.
#[derive(Clone)]
pub struct RelayConnectSpec {
    /// Optional direct target attempted before either relay transport.
    pub direct_target: Option<(String, u16)>,
    /// QUIC/TLS relay socket.
    pub quic_address: SocketAddr,
    /// TCP/TLS fallback relay socket.
    pub tls_address: SocketAddr,
    /// TLS server name verified against the pinned CA.
    pub server_name: String,
    /// DER certificate trusted only for this relay profile.
    pub root_certificate_der: Vec<u8>,
    /// Secret bearer token retained only by trusted Rust code.
    pub token: Zeroizing<String>,
    /// Server-defined target mapping.
    pub mapping: String,
    /// Deadline applied independently to each route attempt.
    pub timeout: Duration,
}

impl fmt::Debug for RelayConnectSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayConnectSpec")
            .field("direct_target", &self.direct_target)
            .field("quic_address", &self.quic_address)
            .field("tls_address", &self.tls_address)
            .field("server_name", &self.server_name)
            .field("root_certificate_der", &"PINNED")
            .field("token", &"REDACTED")
            .field("mapping", &self.mapping)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Established opaque byte stream plus the selected observable route.
pub struct RelayConnection {
    /// Selected route for diagnostics and audit correlation.
    pub transport: SelectedTransport,
    /// Byte stream ready for SSH, RDP, or another allowed protocol.
    pub stream: RelayStream,
}

/// Type-preserving direct or encrypted relay byte stream.
pub enum RelayStream {
    /// Direct target TCP stream.
    Direct(TcpStream),
    /// QUIC bidirectional stream with its owning connection retained.
    Quic {
        /// Combined QUIC send/receive halves.
        io: QuicBiStream,
        /// Owning connection retained for the lifetime of its streams.
        connection: quinn::Connection,
    },
    /// TCP protected by rustls.
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl fmt::Debug for RelayStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let route = match self {
            Self::Direct(_) => "Direct",
            Self::Quic { .. } => "QuicTls",
            Self::Tls(_) => "TcpTls",
        };
        formatter.debug_tuple("RelayStream").field(&route).finish()
    }
}

/// Combined QUIC stream implementing Tokio byte I/O.
pub struct QuicBiStream {
    send: quinn::SendStream,
    receive: quinn::RecvStream,
}

impl AsyncRead for QuicBiStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.receive).poll_read(context, buffer)
    }
}

impl AsyncWrite for QuicBiStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.send), context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.send), context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.send), context)
    }
}

impl AsyncRead for RelayStream {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Direct(stream) => Pin::new(stream).poll_read(context, buffer),
            Self::Quic { io, .. } => Pin::new(io).poll_read(context, buffer),
            Self::Tls(stream) => Pin::new(stream).poll_read(context, buffer),
        }
    }
}

impl AsyncWrite for RelayStream {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Direct(stream) => Pin::new(stream).poll_write(context, buffer),
            Self::Quic { io, .. } => Pin::new(io).poll_write(context, buffer),
            Self::Tls(stream) => Pin::new(stream).poll_write(context, buffer),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Direct(stream) => Pin::new(stream).poll_flush(context),
            Self::Quic { io, .. } => Pin::new(io).poll_flush(context),
            Self::Tls(stream) => Pin::new(stream).poll_flush(context),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Direct(stream) => Pin::new(stream).poll_shutdown(context),
            Self::Quic { io, .. } => Pin::new(io).poll_shutdown(context),
            Self::Tls(stream) => Pin::new(stream).poll_shutdown(context),
        }
    }
}

/// Opens a direct stream first, then QUIC/TLS, then TCP/TLS.
///
/// # Errors
/// Returns a redacted error only after every configured route fails.
pub async fn connect(spec: &RelayConnectSpec) -> Result<RelayConnection, RelayError> {
    validate_spec(spec)?;
    if let Some((host, port)) = &spec.direct_target
        && let Ok(Ok(stream)) =
            tokio::time::timeout(spec.timeout, TcpStream::connect((host.as_str(), *port))).await
    {
        audit_selected(SelectedTransport::Direct, &spec.mapping);
        return Ok(RelayConnection {
            transport: SelectedTransport::Direct,
            stream: RelayStream::Direct(stream),
        });
    }
    if let Ok(Ok(connection)) = tokio::time::timeout(spec.timeout, connect_quic(spec)).await {
        audit_selected(SelectedTransport::QuicTls, &spec.mapping);
        return Ok(connection);
    }
    let connection = tokio::time::timeout(spec.timeout, connect_tls(spec))
        .await
        .map_err(|_| RelayError::Unavailable)??;
    audit_selected(SelectedTransport::TcpTls, &spec.mapping);
    Ok(connection)
}

fn audit_selected(transport: SelectedTransport, mapping: &str) {
    let transport = match transport {
        SelectedTransport::Direct => "direct",
        SelectedTransport::QuicTls => "quic_tls",
        SelectedTransport::TcpTls => "tcp_tls",
    };
    eprintln!(
        "{{\"component\":\"relay-client\",\"event\":\"route_selected\",\"transport\":\"{transport}\",\"mapping\":{}}}",
        serde_json::to_string(mapping).unwrap_or_else(|_| "\"invalid\"".to_owned())
    );
}

fn audit_failure(transport: &str, stage: &str, mapping: &str) {
    eprintln!(
        "{{\"component\":\"relay-client\",\"event\":\"route_failed\",\"transport\":\"{transport}\",\"stage\":\"{stage}\",\"mapping\":{}}}",
        serde_json::to_string(mapping).unwrap_or_else(|_| "\"invalid\"".to_owned())
    );
}

async fn connect_quic(spec: &RelayConnectSpec) -> Result<RelayConnection, RelayError> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(spec.root_certificate_der.clone()))
        .map_err(|_| RelayError::Configuration)?;
    let config = quinn::ClientConfig::with_root_certificates(Arc::new(roots))
        .map_err(|_| RelayError::Configuration)?;
    let mut endpoint = quinn::Endpoint::client(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .map_err(|_| RelayError::Unavailable)?;
    endpoint.set_default_client_config(config);
    let connection = endpoint
        .connect(spec.quic_address, &spec.server_name)
        .map_err(|_| RelayError::Configuration)?
        .await
        .map_err(|_| {
            audit_failure("quic_tls", "handshake", &spec.mapping);
            RelayError::Unavailable
        })?;
    let (send, receive) = connection.open_bi().await.map_err(|_| {
        audit_failure("quic_tls", "open_stream", &spec.mapping);
        RelayError::Unavailable
    })?;
    let mut io = QuicBiStream { send, receive };
    authenticate(&mut io, spec).await?;
    Ok(RelayConnection {
        transport: SelectedTransport::QuicTls,
        stream: RelayStream::Quic { io, connection },
    })
}

async fn connect_tls(spec: &RelayConnectSpec) -> Result<RelayConnection, RelayError> {
    let tcp = TcpStream::connect(spec.tls_address)
        .await
        .map_err(|_| RelayError::Unavailable)?;
    let name =
        ServerName::try_from(spec.server_name.clone()).map_err(|_| RelayError::Configuration)?;
    let mut stream = TlsConnector::from(Arc::new(tls_client_config(spec)?))
        .connect(name, tcp)
        .await
        .map_err(|_| RelayError::Unavailable)?;
    authenticate(&mut stream, spec).await?;
    Ok(RelayConnection {
        transport: SelectedTransport::TcpTls,
        stream: RelayStream::Tls(Box::new(stream)),
    })
}

async fn authenticate<S>(stream: &mut S, spec: &RelayConnectSpec) -> Result<(), RelayError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut hello = ClientHello {
        version: PROTOCOL_VERSION,
        token: spec.token.to_string(),
        mapping: spec.mapping.clone(),
    };
    write_frame(stream, &hello)
        .await
        .map_err(|_| RelayError::Unavailable)?;
    hello.token.zeroize();
    let response: ServerHello = read_frame(stream)
        .await
        .map_err(|_| RelayError::Unavailable)?;
    if response.version != PROTOCOL_VERSION || !response.accepted {
        return Err(RelayError::Rejected(response.code));
    }
    Ok(())
}

fn tls_client_config(spec: &RelayConnectSpec) -> Result<rustls::ClientConfig, RelayError> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(spec.root_certificate_der.clone()))
        .map_err(|_| RelayError::Configuration)?;
    Ok(rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|_| RelayError::Configuration)?
    .with_root_certificates(roots)
    .with_no_client_auth())
}

fn validate_spec(spec: &RelayConnectSpec) -> Result<(), RelayError> {
    let hello = ClientHello {
        version: PROTOCOL_VERSION,
        token: spec.token.to_string(),
        mapping: spec.mapping.clone(),
    };
    hello.validate().map_err(|_| RelayError::Configuration)?;
    ServerName::try_from(spec.server_name.clone()).map_err(|_| RelayError::Configuration)?;
    if spec.root_certificate_der.is_empty() || spec.timeout.is_zero() {
        return Err(RelayError::Configuration);
    }
    Ok(())
}

/// Stable relay connection error.
#[derive(Debug, Error)]
pub enum RelayError {
    /// Local profile is malformed.
    #[error("relay configuration is invalid")]
    Configuration,
    /// Server rejected authentication, target policy, or quota.
    #[error("relay request was rejected: {0}")]
    Rejected(String),
    /// Direct, QUIC, and TCP fallback were unavailable.
    #[error("relay is unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn debug_should_redact_token_and_certificate() {
        let spec = RelayConnectSpec {
            direct_target: None,
            quic_address: "127.0.0.1:9443".parse().expect("valid address"),
            tls_address: "127.0.0.1:9444".parse().expect("valid address"),
            server_name: "relay.example".to_owned(),
            root_certificate_der: vec![1, 2, 3],
            token: Zeroizing::new("never-print-this".to_owned()),
            mapping: "ssh-prod".to_owned(),
            timeout: Duration::from_secs(5),
        };
        let output = format!("{spec:?}");
        assert!(!output.contains("never-print-this"));
        assert!(!output.contains("[1, 2, 3]"));
    }

    #[tokio::test]
    async fn connector_should_prefer_a_reachable_direct_target() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener must bind");
        let address = listener.local_addr().expect("listener address must exist");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("client must connect");
            let mut byte = [0_u8; 1];
            stream
                .read_exact(&mut byte)
                .await
                .expect("byte must arrive");
            stream.write_all(&byte).await.expect("byte must echo");
        });
        let spec = RelayConnectSpec {
            direct_target: Some((address.ip().to_string(), address.port())),
            quic_address: "127.0.0.1:9".parse().expect("valid address"),
            tls_address: "127.0.0.1:9".parse().expect("valid address"),
            server_name: "relay.example".to_owned(),
            root_certificate_der: vec![1],
            token: Zeroizing::new("secret".to_owned()),
            mapping: "ssh-prod".to_owned(),
            timeout: Duration::from_secs(1),
        };
        let mut connection = connect(&spec).await.expect("direct path must connect");
        assert_eq!(connection.transport, SelectedTransport::Direct);
        connection
            .stream
            .write_all(b"x")
            .await
            .expect("write must work");
        let mut byte = [0_u8; 1];
        connection
            .stream
            .read_exact(&mut byte)
            .await
            .expect("echo must arrive");
        assert_eq!(byte, *b"x");
    }
}
