//! Bounded network connectors for direct, proxy, and system-routed SSH streams.

use std::fmt;
use std::fmt::Write as _;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use firstshell_relay_client::RelayConnectSpec;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpStream, lookup_host};
use url::Url;
use zeroize::Zeroizing;

const MAX_HOST_BYTES: usize = 253;
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether the client or proxy resolves a target hostname.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyDnsMode {
    /// Send the hostname to the proxy and let it resolve the target.
    #[default]
    Remote,
    /// Resolve the target locally before asking the proxy to connect.
    Local,
}

/// One validated TCP target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectTarget {
    /// DNS name or IP address.
    pub host: String,
    /// Non-zero TCP port.
    pub port: u16,
}

impl ConnectTarget {
    /// Creates a validated target.
    ///
    /// # Errors
    /// Returns [`ConnectorError::InvalidEndpoint`] for empty, oversized, or unsafe values.
    pub fn new(host: impl Into<String>, port: u16) -> Result<Self, ConnectorError> {
        let value = Self {
            host: host.into(),
            port,
        };
        validate_endpoint(&value.host, value.port)?;
        Ok(value)
    }

    fn authority(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Secret-bearing proxy endpoint retained only by the trusted Rust process.
#[derive(Clone)]
pub struct ProxyEndpoint {
    /// Proxy DNS name or IP address.
    pub host: String,
    /// Proxy TCP port.
    pub port: u16,
    /// Optional proxy account.
    pub username: String,
    /// Optional proxy password.
    pub password: Option<Zeroizing<String>>,
    /// Target DNS resolution policy.
    pub dns_mode: ProxyDnsMode,
}

impl fmt::Debug for ProxyEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyEndpoint")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "REDACTED"))
            .field("dns_mode", &self.dns_mode)
            .finish()
    }
}

/// Concrete route used to create an SSH byte stream.
#[derive(Debug, Clone)]
pub enum ProxyRoute {
    /// Connect directly to the target.
    Direct,
    /// Connect through a SOCKS5 proxy.
    Socks5(ProxyEndpoint),
    /// Connect through an HTTP CONNECT proxy.
    HttpConnect(ProxyEndpoint),
    /// Resolve the operating-system proxy once for this connection.
    System,
    /// Use a direct-first `FirstShell` relay profile with encrypted fallback.
    Relay(RelayConnectSpec),
}

/// Type-erased async byte stream accepted by `russh::client::connect_stream`.
pub struct BoxedStream(Pin<Box<dyn AsyncStream>>);

impl fmt::Debug for BoxedStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BoxedStream(REDACTED)")
    }
}

trait AsyncStream: AsyncRead + AsyncWrite + Send {}
impl<T> AsyncStream for T where T: AsyncRead + AsyncWrite + Send {}

impl BoxedStream {
    pub(crate) fn new(stream: impl AsyncRead + AsyncWrite + Send + 'static) -> Self {
        Self(Box::pin(stream))
    }
}

impl AsyncRead for BoxedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.0.as_mut().poll_read(context, buffer)
    }
}

impl AsyncWrite for BoxedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.0.as_mut().poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.as_mut().poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.as_mut().poll_shutdown(context)
    }
}

/// Opens bounded direct or proxy streams without exposing proxy credentials to the caller.
#[derive(Debug, Clone)]
pub struct Connector {
    route: ProxyRoute,
    timeout: Duration,
}

impl Connector {
    /// Creates a connector with the default 30 second deadline.
    #[must_use]
    pub fn new(route: ProxyRoute) -> Self {
        Self {
            route,
            timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// Overrides the connection deadline, primarily for controlled tests.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Opens one stream and freezes any system-proxy resolution for its lifetime.
    ///
    /// # Errors
    /// Returns a stable connector failure for invalid endpoints, timeouts, authentication,
    /// unsupported proxy responses, or network I/O errors.
    pub async fn connect(&self, target: &ConnectTarget) -> Result<BoxedStream, ConnectorError> {
        validate_endpoint(&target.host, target.port)?;
        tokio::time::timeout(self.timeout, connect_route(&self.route, target))
            .await
            .map_err(|_| ConnectorError::Timeout)?
    }
}

async fn connect_route(
    route: &ProxyRoute,
    target: &ConnectTarget,
) -> Result<BoxedStream, ConnectorError> {
    match route {
        ProxyRoute::Direct => Ok(BoxedStream::new(connect_tcp(target).await?)),
        ProxyRoute::Socks5(proxy) => Ok(BoxedStream::new(connect_socks5(proxy, target).await?)),
        ProxyRoute::HttpConnect(proxy) => Ok(BoxedStream::new(connect_http(proxy, target).await?)),
        ProxyRoute::System => {
            let target = target.clone();
            let resolution_target = target.clone();
            let route =
                tokio::task::spawn_blocking(move || resolve_system_route(&resolution_target))
                    .await
                    .map_err(|_| ConnectorError::SystemProxyUnavailable)??;
            Box::pin(connect_route(&route, &target)).await
        }
        ProxyRoute::Relay(spec) => firstshell_relay_client::connect(spec)
            .await
            .map(|connection| BoxedStream::new(connection.stream))
            .map_err(|_| ConnectorError::RelayUnavailable),
    }
}

async fn connect_tcp(target: &ConnectTarget) -> Result<TcpStream, ConnectorError> {
    TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(ConnectorError::Io)
}

async fn connect_proxy_tcp(proxy: &ProxyEndpoint) -> Result<TcpStream, ConnectorError> {
    validate_endpoint(&proxy.host, proxy.port)?;
    TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(ConnectorError::Io)
}

async fn connect_socks5(
    proxy: &ProxyEndpoint,
    target: &ConnectTarget,
) -> Result<TcpStream, ConnectorError> {
    let mut stream = connect_proxy_tcp(proxy).await?;
    let wants_auth = !proxy.username.is_empty() || proxy.password.is_some();
    let methods: &[u8] = if wants_auth { &[0x00, 0x02] } else { &[0x00] };
    let method_count = if wants_auth { 2_u8 } else { 1_u8 };
    stream.write_all(&[0x05, method_count]).await?;
    stream.write_all(methods).await?;
    let mut selection = [0_u8; 2];
    stream.read_exact(&mut selection).await?;
    if selection[0] != 0x05 {
        return Err(ConnectorError::InvalidProxyResponse);
    }
    match selection[1] {
        0x00 => {}
        0x02 if wants_auth => authenticate_socks5(&mut stream, proxy).await?,
        0xff => return Err(ConnectorError::ProxyAuthentication),
        _ => return Err(ConnectorError::InvalidProxyResponse),
    }

    let address = socks_target_address(target, proxy.dns_mode).await?;
    let mut request = vec![0x05, 0x01, 0x00];
    request.extend_from_slice(&address);
    request.extend_from_slice(&target.port.to_be_bytes());
    stream.write_all(&request).await?;

    let mut response = [0_u8; 4];
    stream.read_exact(&mut response).await?;
    if response[0] != 0x05 || response[2] != 0x00 {
        return Err(ConnectorError::InvalidProxyResponse);
    }
    if response[1] != 0x00 {
        return Err(ConnectorError::ProxyRejected(response[1]));
    }
    consume_socks_address(&mut stream, response[3]).await?;
    Ok(stream)
}

async fn authenticate_socks5(
    stream: &mut TcpStream,
    proxy: &ProxyEndpoint,
) -> Result<(), ConnectorError> {
    let password = proxy.password.as_deref().map_or("", String::as_str);
    if proxy.username.len() > u8::MAX as usize || password.len() > u8::MAX as usize {
        return Err(ConnectorError::ProxyAuthentication);
    }
    let username_length =
        u8::try_from(proxy.username.len()).map_err(|_| ConnectorError::ProxyAuthentication)?;
    let password_length =
        u8::try_from(password.len()).map_err(|_| ConnectorError::ProxyAuthentication)?;
    stream.write_all(&[0x01, username_length]).await?;
    stream.write_all(proxy.username.as_bytes()).await?;
    stream.write_all(&[password_length]).await?;
    stream.write_all(password.as_bytes()).await?;
    let mut response = [0_u8; 2];
    stream.read_exact(&mut response).await?;
    if response != [0x01, 0x00] {
        return Err(ConnectorError::ProxyAuthentication);
    }
    Ok(())
}

async fn socks_target_address(
    target: &ConnectTarget,
    dns_mode: ProxyDnsMode,
) -> Result<Vec<u8>, ConnectorError> {
    let address = match (target.host.parse::<IpAddr>(), dns_mode) {
        (Ok(address), _) => address,
        (Err(_), ProxyDnsMode::Remote) => {
            if target.host.len() > u8::MAX as usize {
                return Err(ConnectorError::InvalidEndpoint);
            }
            let host_length =
                u8::try_from(target.host.len()).map_err(|_| ConnectorError::InvalidEndpoint)?;
            let mut encoded = vec![0x03, host_length];
            encoded.extend_from_slice(target.host.as_bytes());
            return Ok(encoded);
        }
        (Err(_), ProxyDnsMode::Local) => lookup_host((target.host.as_str(), target.port))
            .await?
            .next()
            .map(|socket| socket.ip())
            .ok_or(ConnectorError::DnsResolution)?,
    };
    Ok(match address {
        IpAddr::V4(value) => {
            let mut encoded = vec![0x01];
            encoded.extend_from_slice(&value.octets());
            encoded
        }
        IpAddr::V6(value) => {
            let mut encoded = vec![0x04];
            encoded.extend_from_slice(&value.octets());
            encoded
        }
    })
}

async fn consume_socks_address(
    stream: &mut TcpStream,
    address_type: u8,
) -> Result<(), ConnectorError> {
    let length = match address_type {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut length = [0_u8; 1];
            stream.read_exact(&mut length).await?;
            usize::from(length[0])
        }
        _ => return Err(ConnectorError::InvalidProxyResponse),
    };
    let mut address_and_port = vec![0_u8; length + 2];
    stream.read_exact(&mut address_and_port).await?;
    Ok(())
}

async fn connect_http(
    proxy: &ProxyEndpoint,
    target: &ConnectTarget,
) -> Result<TcpStream, ConnectorError> {
    let mut stream = connect_proxy_tcp(proxy).await?;
    let authority = target.authority();
    let mut request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if !proxy.username.is_empty() || proxy.password.is_some() {
        let password = proxy.password.as_deref().map_or("", String::as_str);
        let encoded = BASE64_STANDARD.encode(format!("{}:{password}", proxy.username));
        write!(request, "Proxy-Authorization: Basic {encoded}\r\n")
            .map_err(|_| ConnectorError::InvalidProxyResponse)?;
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;

    let mut header = Vec::with_capacity(256);
    while header.len() < MAX_HTTP_HEADER_BYTES && !header.ends_with(b"\r\n\r\n") {
        header.push(stream.read_u8().await?);
    }
    if !header.ends_with(b"\r\n\r\n") {
        return Err(ConnectorError::InvalidProxyResponse);
    }
    let first_line = header
        .split(|byte| *byte == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .ok_or(ConnectorError::InvalidProxyResponse)?;
    let status = first_line
        .split_ascii_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(ConnectorError::InvalidProxyResponse)?;
    match status {
        200 => Ok(stream),
        407 => Err(ConnectorError::ProxyAuthentication),
        _ => Err(ConnectorError::HttpProxyRejected(status)),
    }
}

fn resolve_system_route(target: &ConnectTarget) -> Result<ProxyRoute, ConnectorError> {
    #[cfg(windows)]
    let value = windows_system_proxy()?;
    #[cfg(not(windows))]
    let value = environment_system_proxy();

    let Some(value) = value else {
        return Ok(ProxyRoute::Direct);
    };
    let route = parse_proxy_url(&value)?;
    if should_bypass_system_proxy(target) {
        Ok(ProxyRoute::Direct)
    } else {
        Ok(route)
    }
}

#[cfg(windows)]
fn windows_system_proxy() -> Result<Option<String>, ConnectorError> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let settings = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .map_err(|_| ConnectorError::SystemProxyUnavailable)?;
    let enabled = settings.get_value::<u32, _>("ProxyEnable").unwrap_or(0) == 1;
    if !enabled {
        return Ok(None);
    }
    let raw = settings
        .get_value::<String, _>("ProxyServer")
        .map_err(|_| ConnectorError::SystemProxyUnavailable)?;
    select_windows_proxy(&raw).map(Some)
}

#[cfg(windows)]
fn select_windows_proxy(raw: &str) -> Result<String, ConnectorError> {
    if !raw.contains('=') {
        return Ok(format_proxy_url("http", raw));
    }
    for (name, scheme) in [("socks", "socks5"), ("https", "http"), ("http", "http")] {
        if let Some(value) = raw.split(';').find_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        }) {
            return Ok(format_proxy_url(scheme, value));
        }
    }
    Err(ConnectorError::SystemProxyUnavailable)
}

#[cfg(windows)]
fn format_proxy_url(scheme: &str, value: &str) -> String {
    if value.contains("://") {
        value.to_owned()
    } else {
        format!("{scheme}://{value}")
    }
}

#[cfg(not(windows))]
fn environment_system_proxy() -> Option<String> {
    [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .into_iter()
    .find_map(|key| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn parse_proxy_url(value: &str) -> Result<ProxyRoute, ConnectorError> {
    let parsed = Url::parse(value).map_err(|_| ConnectorError::SystemProxyUnavailable)?;
    let host = parsed
        .host_str()
        .ok_or(ConnectorError::SystemProxyUnavailable)?
        .to_owned();
    let scheme = parsed.scheme().to_ascii_lowercase();
    let port = parsed
        .port_or_known_default()
        .ok_or(ConnectorError::SystemProxyUnavailable)?;
    let endpoint = ProxyEndpoint {
        host,
        port,
        username: parsed.username().to_owned(),
        password: parsed
            .password()
            .map(|value| Zeroizing::new(value.to_owned())),
        dns_mode: ProxyDnsMode::Remote,
    };
    match scheme.as_str() {
        "socks" | "socks5" | "socks5h" => Ok(ProxyRoute::Socks5(endpoint)),
        "http" => Ok(ProxyRoute::HttpConnect(endpoint)),
        _ => Err(ConnectorError::SystemProxyUnavailable),
    }
}

fn should_bypass_system_proxy(target: &ConnectTarget) -> bool {
    target.host.eq_ignore_ascii_case("localhost")
        || target
            .host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_endpoint(host: &str, port: u16) -> Result<(), ConnectorError> {
    if host.is_empty()
        || host.len() > MAX_HOST_BYTES
        || port == 0
        || host.starts_with('-')
        || host
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ConnectorError::InvalidEndpoint);
    }
    Ok(())
}

/// Stable failure returned by one connector attempt.
#[derive(Debug, Error)]
pub enum ConnectorError {
    /// Host or port failed validation.
    #[error("invalid connector endpoint")]
    InvalidEndpoint,
    /// The operation exceeded its deadline.
    #[error("connector timed out")]
    Timeout,
    /// DNS resolution produced no usable address.
    #[error("target DNS resolution failed")]
    DnsResolution,
    /// The selected proxy rejected or could not perform authentication.
    #[error("proxy authentication failed")]
    ProxyAuthentication,
    /// A SOCKS5 proxy rejected the target with a protocol status.
    #[error("SOCKS5 proxy rejected the target with status {0}")]
    ProxyRejected(u8),
    /// An HTTP proxy returned a non-success status.
    #[error("HTTP proxy rejected the target with status {0}")]
    HttpProxyRejected(u16),
    /// The proxy sent a malformed or unsupported response.
    #[error("invalid proxy response")]
    InvalidProxyResponse,
    /// The operating-system proxy could not be resolved safely.
    #[error("system proxy is unavailable")]
    SystemProxyUnavailable,
    /// Direct, QUIC/TLS, and TCP/TLS relay routes were unavailable or rejected.
    #[error("relay route is unavailable")]
    RelayUnavailable,
    /// Underlying socket I/O failed.
    #[error("connector I/O failed")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn proxy_endpoint(port: u16) -> ProxyEndpoint {
        ProxyEndpoint {
            host: "127.0.0.1".to_owned(),
            port,
            username: "operator".to_owned(),
            password: Some(Zeroizing::new("secret".to_owned())),
            dns_mode: ProxyDnsMode::Remote,
        }
    }

    #[tokio::test]
    async fn socks5_should_authenticate_and_preserve_remote_dns() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind SOCKS test");
        let port = listener.local_addr().expect("SOCKS address").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept SOCKS test");
            let mut greeting = [0_u8; 4];
            stream
                .read_exact(&mut greeting)
                .await
                .expect("read methods");
            assert_eq!(greeting, [5, 2, 0, 2]);
            stream.write_all(&[5, 2]).await.expect("select auth");
            let mut auth = [0_u8; 17];
            stream.read_exact(&mut auth).await.expect("read auth");
            assert_eq!(&auth, b"\x01\x08operator\x06secret");
            stream.write_all(&[1, 0]).await.expect("accept auth");
            let mut request = [0_u8; 18];
            stream.read_exact(&mut request).await.expect("read connect");
            assert_eq!(&request, b"\x05\x01\x00\x03\x0bexample.com\x00\x16");
            stream
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 22])
                .await
                .expect("accept connect");
        });
        Connector::new(ProxyRoute::Socks5(proxy_endpoint(port)))
            .connect(&ConnectTarget::new("example.com", 22).expect("target"))
            .await
            .expect("SOCKS connection");
        server.await.expect("SOCKS server");
    }

    #[tokio::test]
    async fn http_connect_should_send_basic_auth_without_leaking_it_to_errors() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind HTTP test");
        let port = listener.local_addr().expect("HTTP address").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept HTTP test");
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.expect("read HTTP request"));
            }
            let request = String::from_utf8(request).expect("HTTP request text");
            assert!(request.starts_with("CONNECT example.com:22 HTTP/1.1\r\n"));
            assert!(request.contains("Proxy-Authorization: Basic b3BlcmF0b3I6c2VjcmV0\r\n"));
            stream
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                .await
                .expect("reject HTTP request");
        });
        let error = Connector::new(ProxyRoute::HttpConnect(proxy_endpoint(port)))
            .connect(&ConnectTarget::new("example.com", 22).expect("target"))
            .await
            .expect_err("HTTP authentication should fail");
        assert_eq!(error.to_string(), "proxy authentication failed");
        server.await.expect("HTTP server");
    }

    #[test]
    fn proxy_debug_should_redact_password() {
        let debug = format!("{:?}", proxy_endpoint(1080));
        assert!(!debug.contains("secret"));
    }
}
