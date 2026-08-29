//! Self-hosted, policy-bound `FirstShell` relay service.

use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Error as IoError, ErrorKind};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use firstshell_relay_protocol::{
    ALPN, ClientHello, PROTOCOL_VERSION, ServerHello, read_frame, write_frame,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_rustls::TlsAcceptor;
use uuid::Uuid;
use zeroize::Zeroize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    quic_listen: SocketAddr,
    tls_listen: SocketAddr,
    certificate_pem: PathBuf,
    private_key_pem: PathBuf,
    max_concurrent: usize,
    mappings: Vec<MappingConfig>,
    clients: Vec<ClientConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MappingConfig {
    name: String,
    target_host: String,
    target_port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClientConfig {
    id: String,
    token_sha256: String,
    allowed_mappings: Vec<String>,
    max_concurrent: usize,
    max_duration_seconds: u64,
    max_bytes: u64,
    bandwidth_bytes_per_second: u64,
}

struct ClientPolicy {
    id: String,
    token_hash: [u8; 32],
    allowed_mappings: HashSet<String>,
    max_concurrent: usize,
    max_duration: Duration,
    max_bytes: u64,
    bandwidth_bytes_per_second: u64,
    active: AtomicUsize,
}

struct Runtime {
    mappings: HashMap<String, MappingConfig>,
    clients: Vec<Arc<ClientPolicy>>,
    global: Arc<Semaphore>,
}

struct ClientSlot {
    policy: Arc<ClientPolicy>,
    _global: OwnedSemaphorePermit,
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        self.policy.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditEvent<'a> {
    timestamp_ms: u128,
    session_id: &'a str,
    client_id: &'a str,
    mapping: &'a str,
    transport: &'a str,
    outcome: &'a str,
    bytes: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = config_argument()?;
    let raw = tokio::fs::read(&path).await?;
    let config: Config = serde_json::from_slice(&raw)?;
    let runtime = Arc::new(compile_runtime(&config)?);
    let (certificates, private_key) =
        load_identity(&config.certificate_pem, &config.private_key_pem)?;
    let quic = quic_server_config(certificates.clone(), private_key.clone_key())?;
    let tls = tls_server_config(certificates, private_key)?;
    let endpoint = quinn::Endpoint::server(quic, config.quic_listen)?;
    let listener = TcpListener::bind(config.tls_listen).await?;
    eprintln!(
        "relayd listening on QUIC {} and TCP/TLS {}",
        config.quic_listen, config.tls_listen
    );

    tokio::try_join!(
        serve_quic(endpoint, Arc::clone(&runtime)),
        serve_tls(listener, TlsAcceptor::from(Arc::new(tls)), runtime),
    )?;
    Ok(())
}

async fn serve_quic(endpoint: quinn::Endpoint, runtime: Arc<Runtime>) -> Result<(), IoError> {
    while let Some(incoming) = endpoint.accept().await {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            let connection = match incoming.await {
                Ok(connection) => connection,
                Err(error) => {
                    eprintln!(
                        "{{\"component\":\"relayd\",\"event\":\"quic_handshake_failed\",\"reason\":{}}}",
                        serde_json::to_string(&error.to_string())
                            .unwrap_or_else(|_| "\"unavailable\"".to_owned())
                    );
                    return;
                }
            };
            loop {
                let Ok((send, receive)) = connection.accept_bi().await else {
                    break;
                };
                let runtime = Arc::clone(&runtime);
                tokio::spawn(async move {
                    let mut stream = QuicIo { send, receive };
                    let _ = Box::pin(handle(&mut stream, runtime, "quic_tls")).await;
                });
            }
        });
    }
    Ok(())
}

async fn serve_tls(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    runtime: Arc<Runtime>,
) -> Result<(), IoError> {
    loop {
        let (socket, _) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            let Ok(mut stream) = acceptor.accept(socket).await else {
                return;
            };
            let _ = Box::pin(handle(&mut stream, runtime, "tcp_tls")).await;
        });
    }
}

async fn handle<S>(stream: &mut S, runtime: Arc<Runtime>, transport: &str) -> Result<(), IoError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut hello: ClientHello = read_frame(stream)
        .await
        .map_err(|_| IoError::new(ErrorKind::InvalidData, "invalid relay hello"))?;
    if hello.validate().is_err() {
        reject(stream, "INVALID_REQUEST").await?;
        hello.token.zeroize();
        return Ok(());
    }
    let Some(policy) = authenticate(&runtime.clients, hello.token.as_bytes()) else {
        reject(stream, "AUTHENTICATION_FAILED").await?;
        hello.token.zeroize();
        return Ok(());
    };
    hello.token.zeroize();
    let Some(mapping) = runtime.mappings.get(&hello.mapping).cloned() else {
        reject(stream, "TARGET_DENIED").await?;
        return Ok(());
    };
    if !policy.allowed_mappings.contains(&mapping.name) {
        reject(stream, "TARGET_DENIED").await?;
        return Ok(());
    }
    let Some(slot) = reserve(policy, &runtime.global) else {
        reject(stream, "QUOTA_CONCURRENT").await?;
        return Ok(());
    };
    let Ok(Ok(mut target)) = tokio::time::timeout(
        Duration::from_secs(10),
        TcpStream::connect((mapping.target_host.as_str(), mapping.target_port)),
    )
    .await
    else {
        reject(stream, "TARGET_UNAVAILABLE").await?;
        return Ok(());
    };
    let session_id = Uuid::new_v4().to_string();
    write_frame(
        stream,
        &ServerHello {
            version: PROTOCOL_VERSION,
            accepted: true,
            code: "OK".to_owned(),
            session_id: Some(session_id.clone()),
        },
    )
    .await
    .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "relay response failed"))?;

    audit(
        &session_id,
        &slot.policy.id,
        &mapping.name,
        transport,
        "accepted",
        0,
    );
    let transferred = tokio::time::timeout(
        slot.policy.max_duration,
        Box::pin(pump(
            stream,
            &mut target,
            slot.policy.max_bytes,
            slot.policy.bandwidth_bytes_per_second,
        )),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default();
    audit(
        &session_id,
        &slot.policy.id,
        &mapping.name,
        transport,
        "closed",
        transferred,
    );
    Ok(())
}

async fn reject<S>(stream: &mut S, code: &str) -> Result<(), IoError>
where
    S: AsyncWrite + Unpin,
{
    write_frame(
        stream,
        &ServerHello {
            version: PROTOCOL_VERSION,
            accepted: false,
            code: code.to_owned(),
            session_id: None,
        },
    )
    .await
    .map_err(|_| IoError::new(ErrorKind::BrokenPipe, "relay rejection failed"))
}

fn reserve(policy: Arc<ClientPolicy>, global: &Arc<Semaphore>) -> Option<ClientSlot> {
    let global = Arc::clone(global).try_acquire_owned().ok()?;
    let reserved = policy
        .active
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            (active < policy.max_concurrent).then_some(active + 1)
        })
        .is_ok();
    reserved.then_some(ClientSlot {
        policy,
        _global: global,
    })
}

fn authenticate(clients: &[Arc<ClientPolicy>], token: &[u8]) -> Option<Arc<ClientPolicy>> {
    let digest: [u8; 32] = Sha256::digest(token).into();
    clients
        .iter()
        .find(|client| constant_time_equal(&client.token_hash, &digest))
        .cloned()
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

async fn pump<A, B>(
    left: &mut A,
    right: &mut B,
    max_bytes: u64,
    bytes_per_second: u64,
) -> Result<u64, IoError>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let total = Arc::new(AtomicU64::new(0));
    let (left_read, left_write) = tokio::io::split(left);
    let (right_read, right_write) = tokio::io::split(right);
    let upstream = copy_limited(
        left_read,
        right_write,
        Arc::clone(&total),
        max_bytes,
        bytes_per_second,
    );
    let downstream = copy_limited(
        right_read,
        left_write,
        Arc::clone(&total),
        max_bytes,
        bytes_per_second,
    );
    tokio::try_join!(upstream, downstream)?;
    Ok(total.load(Ordering::Acquire).min(max_bytes))
}

async fn copy_limited<R, W>(
    mut reader: R,
    mut writer: W,
    total: Arc<AtomicU64>,
    max_bytes: u64,
    bytes_per_second: u64,
) -> Result<(), IoError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let started = Instant::now();
    let mut direction_bytes = 0_u64;
    let mut buffer = Box::new([0_u8; 16 * 1024]);
    loop {
        let read = reader.read(&mut buffer[..]).await?;
        if read == 0 {
            writer.shutdown().await?;
            return Ok(());
        }
        let next = total.fetch_add(read as u64, Ordering::AcqRel) + read as u64;
        if next > max_bytes {
            return Err(IoError::new(
                ErrorKind::QuotaExceeded,
                "relay byte quota reached",
            ));
        }
        writer.write_all(&buffer[..read]).await?;
        direction_bytes += read as u64;
        if bytes_per_second > 0 {
            let expected = Duration::from_nanos(
                direction_bytes.saturating_mul(1_000_000_000) / bytes_per_second,
            );
            if expected > started.elapsed() {
                tokio::time::sleep(expected - started.elapsed()).await;
            }
        }
    }
}

fn compile_runtime(config: &Config) -> Result<Runtime, IoError> {
    if config.max_concurrent == 0 {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "maxConcurrent must be positive",
        ));
    }
    let mappings = config
        .mappings
        .iter()
        .map(|mapping| (mapping.name.clone(), mapping.clone()))
        .collect::<HashMap<_, _>>();
    if mappings.len() != config.mappings.len()
        || mappings.values().any(|mapping| {
            mapping.name.is_empty() || mapping.target_host.is_empty() || mapping.target_port == 0
        })
    {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "relay mappings are invalid",
        ));
    }
    let clients = config
        .clients
        .iter()
        .map(|client| {
            if client.id.is_empty()
                || client.max_concurrent == 0
                || client.max_duration_seconds == 0
                || client.max_bytes == 0
                || client
                    .allowed_mappings
                    .iter()
                    .any(|name| !mappings.contains_key(name))
            {
                return Err(IoError::new(
                    ErrorKind::InvalidInput,
                    "relay client policy is invalid",
                ));
            }
            Ok(Arc::new(ClientPolicy {
                id: client.id.clone(),
                token_hash: parse_hash(&client.token_sha256)?,
                allowed_mappings: client.allowed_mappings.iter().cloned().collect(),
                max_concurrent: client.max_concurrent,
                max_duration: Duration::from_secs(client.max_duration_seconds),
                max_bytes: client.max_bytes,
                bandwidth_bytes_per_second: client.bandwidth_bytes_per_second,
                active: AtomicUsize::new(0),
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Runtime {
        mappings,
        clients,
        global: Arc::new(Semaphore::new(config.max_concurrent)),
    })
}

fn parse_hash(value: &str) -> Result<[u8; 32], IoError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "tokenSha256 is invalid",
        ));
    }
    let mut output = [0_u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| IoError::new(ErrorKind::InvalidInput, "tokenSha256 is invalid"))?;
    }
    Ok(output)
}

fn load_identity(
    certificate_path: &Path,
    key_path: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), IoError> {
    let mut certificate = BufReader::new(std::fs::File::open(certificate_path)?);
    let certificates = rustls_pemfile::certs(&mut certificate).collect::<Result<Vec<_>, _>>()?;
    let mut key = BufReader::new(std::fs::File::open(key_path)?);
    let private_key = rustls_pemfile::private_key(&mut key)?
        .ok_or_else(|| IoError::new(ErrorKind::InvalidInput, "private key is missing"))?;
    Ok((certificates, private_key))
}

fn quic_server_config(
    certificates: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<quinn::ServerConfig, IoError> {
    quinn::ServerConfig::with_single_cert(certificates, key)
        .map_err(|_| IoError::new(ErrorKind::InvalidInput, "QUIC TLS configuration is invalid"))
}

fn tls_server_config(
    certificates: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<rustls::ServerConfig, IoError> {
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|_| IoError::new(ErrorKind::InvalidInput, "TLS protocol versions are invalid"))?
    .with_no_client_auth()
    .with_single_cert(certificates, key)
    .map_err(|_| IoError::new(ErrorKind::InvalidInput, "TLS identity is invalid"))?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}

fn config_argument() -> Result<PathBuf, IoError> {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() != Some("--config") {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "usage: firstshell-relayd --config <path>",
        ));
    }
    let path = arguments
        .next()
        .ok_or_else(|| IoError::new(ErrorKind::InvalidInput, "config path is missing"))?;
    if arguments.next().is_some() {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "unexpected relayd arguments",
        ));
    }
    Ok(PathBuf::from(path))
}

fn audit(
    session_id: &str,
    client_id: &str,
    mapping: &str,
    transport: &str,
    outcome: &str,
    bytes: u64,
) {
    let event = AuditEvent {
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        session_id,
        client_id,
        mapping,
        transport,
        outcome,
        bytes,
    };
    if let Ok(encoded) = serde_json::to_string(&event) {
        eprintln!("{encoded}");
    }
}

struct QuicIo {
    send: quinn::SendStream,
    receive: quinn::RecvStream,
}

impl AsyncRead for QuicIo {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<Result<(), IoError>> {
        std::pin::Pin::new(&mut self.receive).poll_read(context, buffer)
    }
}

impl AsyncWrite for QuicIo {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<Result<usize, IoError>> {
        AsyncWrite::poll_write(std::pin::Pin::new(&mut self.send), context, buffer)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), IoError>> {
        AsyncWrite::poll_flush(std::pin::Pin::new(&mut self.send), context)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), IoError>> {
        AsyncWrite::poll_shutdown(std::pin::Pin::new(&mut self.send), context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use firstshell_relay_client::{RelayConnectSpec, SelectedTransport, connect};
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zeroize::Zeroizing;

    #[test]
    fn token_comparison_should_match_only_exact_hash() {
        let expected: [u8; 32] = Sha256::digest(b"secret").into();
        let different: [u8; 32] = Sha256::digest(b"different").into();
        assert!(constant_time_equal(&expected, &expected));
        assert!(!constant_time_equal(&expected, &different));
    }

    #[test]
    fn policy_should_reject_unknown_mapping() {
        let config = Config {
            quic_listen: "127.0.0.1:9443".parse().expect("valid address"),
            tls_listen: "127.0.0.1:9444".parse().expect("valid address"),
            certificate_pem: "cert.pem".into(),
            private_key_pem: "key.pem".into(),
            max_concurrent: 1,
            mappings: vec![],
            clients: vec![ClientConfig {
                id: "client".to_owned(),
                token_sha256: "00".repeat(32),
                allowed_mappings: vec!["missing".to_owned()],
                max_concurrent: 1,
                max_duration_seconds: 60,
                max_bytes: 1024,
                bandwidth_bytes_per_second: 1024,
            }],
        };
        assert!(compile_runtime(&config).is_err());
    }

    #[tokio::test]
    async fn authorized_mapping_should_forward_opaque_bytes() {
        let target = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("target must bind");
        let target_address = target.local_addr().expect("target address must exist");
        tokio::spawn(async move {
            let (mut stream, _) = target.accept().await.expect("relay must connect");
            let mut buffer = [0_u8; 4];
            stream
                .read_exact(&mut buffer)
                .await
                .expect("payload must arrive");
            stream.write_all(&buffer).await.expect("payload must echo");
        });
        let token_hash: [u8; 32] = Sha256::digest(b"secret").into();
        let policy = Arc::new(ClientPolicy {
            id: "client".to_owned(),
            token_hash,
            allowed_mappings: HashSet::from(["echo".to_owned()]),
            max_concurrent: 1,
            max_duration: Duration::from_secs(2),
            max_bytes: 1024,
            bandwidth_bytes_per_second: 0,
            active: AtomicUsize::new(0),
        });
        let runtime = Arc::new(Runtime {
            mappings: HashMap::from([(
                "echo".to_owned(),
                MappingConfig {
                    name: "echo".to_owned(),
                    target_host: target_address.ip().to_string(),
                    target_port: target_address.port(),
                },
            )]),
            clients: vec![Arc::clone(&policy)],
            global: Arc::new(Semaphore::new(1)),
        });
        let (mut client, mut server) = tokio::io::duplex(4096);
        let relay = tokio::spawn(async move { handle(&mut server, runtime, "test").await });
        write_frame(
            &mut client,
            &ClientHello {
                version: PROTOCOL_VERSION,
                token: "secret".to_owned(),
                mapping: "echo".to_owned(),
            },
        )
        .await
        .expect("hello must write");
        let response: ServerHello = read_frame(&mut client).await.expect("response must read");
        assert!(response.accepted);
        client.write_all(b"ping").await.expect("payload must write");
        let mut echoed = [0_u8; 4];
        client
            .read_exact(&mut echoed)
            .await
            .expect("echo must read");
        assert_eq!(&echoed, b"ping");
        client.shutdown().await.expect("client must close");
        relay
            .await
            .expect("relay task must join")
            .expect("relay must finish");
        assert_eq!(policy.active.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn handler_should_reject_unauthorized_target_and_concurrent_quota() {
        let policy = Arc::new(ClientPolicy {
            id: "client".to_owned(),
            token_hash: Sha256::digest(b"secret").into(),
            allowed_mappings: HashSet::from(["echo".to_owned()]),
            max_concurrent: 1,
            max_duration: Duration::from_secs(2),
            max_bytes: 1024,
            bandwidth_bytes_per_second: 0,
            active: AtomicUsize::new(0),
        });
        let runtime = Arc::new(Runtime {
            mappings: HashMap::from([(
                "echo".to_owned(),
                MappingConfig {
                    name: "echo".to_owned(),
                    target_host: "127.0.0.1".to_owned(),
                    target_port: 9,
                },
            )]),
            clients: vec![Arc::clone(&policy)],
            global: Arc::new(Semaphore::new(1)),
        });
        let response = request_over_duplex(Arc::clone(&runtime), "unknown").await;
        assert!(!response.accepted);
        assert_eq!(response.code, "TARGET_DENIED");

        policy.active.store(1, Ordering::Release);
        let response = request_over_duplex(runtime, "echo").await;
        assert!(!response.accepted);
        assert_eq!(response.code, "QUOTA_CONCURRENT");
    }

    #[tokio::test]
    async fn client_should_use_tcp_tls_when_direct_and_quic_are_unavailable() {
        let (certificates, private_key, certificate_der) = test_identity();
        let runtime = test_runtime().await;
        let unavailable = unavailable_tcp_address().await;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("TLS listener must bind");
        let tls_address = listener.local_addr().expect("TLS address must exist");
        let tls_task = tokio::spawn(serve_tls(
            listener,
            TlsAcceptor::from(Arc::new(
                tls_server_config(certificates, private_key).expect("TLS config must build"),
            )),
            runtime,
        ));
        let mut tls_connection = connect(&RelayConnectSpec {
            direct_target: Some((unavailable.ip().to_string(), unavailable.port())),
            quic_address: unavailable,
            tls_address,
            server_name: "localhost".to_owned(),
            root_certificate_der: certificate_der,
            token: Zeroizing::new("secret".to_owned()),
            mapping: "echo".to_owned(),
            timeout: Duration::from_millis(250),
        })
        .await
        .expect("TCP/TLS fallback must connect");
        assert_eq!(tls_connection.transport, SelectedTransport::TcpTls);
        assert_echo(&mut tls_connection.stream).await;
        drop(tls_connection);
        tls_task.abort();
    }

    async fn request_over_duplex(runtime: Arc<Runtime>, mapping: &str) -> ServerHello {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let _ = handle(&mut server, runtime, "test").await;
        });
        write_frame(
            &mut client,
            &ClientHello {
                version: PROTOCOL_VERSION,
                token: "secret".to_owned(),
                mapping: mapping.to_owned(),
            },
        )
        .await
        .expect("hello must write");
        read_frame(&mut client).await.expect("response must read")
    }

    async fn test_runtime() -> Arc<Runtime> {
        let target = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("target must bind");
        let target_address = target.local_addr().expect("target address must exist");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = target.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut byte = [0_u8; 1];
                    if stream.read_exact(&mut byte).await.is_ok() {
                        let _ = stream.write_all(&byte).await;
                    }
                });
            }
        });
        let policy = Arc::new(ClientPolicy {
            id: "client".to_owned(),
            token_hash: Sha256::digest(b"secret").into(),
            allowed_mappings: HashSet::from(["echo".to_owned()]),
            max_concurrent: 2,
            max_duration: Duration::from_secs(5),
            max_bytes: 1024,
            bandwidth_bytes_per_second: 0,
            active: AtomicUsize::new(0),
        });
        Arc::new(Runtime {
            mappings: HashMap::from([(
                "echo".to_owned(),
                MappingConfig {
                    name: "echo".to_owned(),
                    target_host: target_address.ip().to_string(),
                    target_port: target_address.port(),
                },
            )]),
            clients: vec![policy],
            global: Arc::new(Semaphore::new(2)),
        })
    }

    fn test_identity() -> (
        Vec<CertificateDer<'static>>,
        PrivateKeyDer<'static>,
        Vec<u8>,
    ) {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_owned()])
                .expect("test certificate must generate");
        let certificate = cert.der().clone();
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
        (
            vec![certificate.clone()],
            key,
            certificate.as_ref().to_vec(),
        )
    }

    async fn unavailable_tcp_address() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("temporary listener must bind");
        listener.local_addr().expect("temporary address must exist")
    }

    async fn assert_echo<S>(stream: &mut S)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        stream.write_all(b"x").await.expect("payload must write");
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await.expect("echo must read");
        assert_eq!(byte, *b"x");
    }
}
