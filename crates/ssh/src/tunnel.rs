//! Bounded local, remote, and dynamic SSH port forwarding.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use russh::Channel;
use russh::client::ChannelOpenHandle;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{AuthenticatedChain, ConnectTarget, SshError, SshSessionFactory};
use firstshell_session::{INTERACTIVE_QUEUE_CAPACITY, SessionEvent};

const MAX_TUNNEL_CONNECTIONS: usize = 128;
const MAX_SOCKS_METHODS: usize = 16;

/// Supported SSH forwarding modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    /// Listen locally and ask the SSH server to connect to a fixed target.
    Local,
    /// Ask the SSH server to listen and forward accepted connections to a local target.
    Remote,
    /// Listen locally as a bounded unauthenticated SOCKS5 CONNECT proxy.
    Dynamic,
}

/// Validated user intent for one independently cancellable tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelSpec {
    /// Forwarding mode.
    pub kind: TunnelKind,
    /// Local or remote bind address.
    pub bind_host: String,
    /// Requested bind port; zero asks the OS/server to choose one.
    pub bind_port: u16,
    /// Fixed target for local and remote forwarding.
    pub target_host: String,
    /// Fixed target port for local and remote forwarding.
    pub target_port: u16,
    /// Explicit acknowledgement required for a non-loopback bind.
    pub allow_non_loopback: bool,
}

/// Public state returned after a tunnel starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelSnapshot {
    /// Forwarding mode.
    pub kind: TunnelKind,
    /// Effective bind address.
    pub bind_host: String,
    /// Effective port, including an OS/server-assigned ephemeral port.
    pub bind_port: u16,
    /// Fixed target host when applicable.
    pub target_host: String,
    /// Fixed target port when applicable.
    pub target_port: u16,
}

/// One running tunnel. Dropping the value does not silently abandon cleanup; call [`Self::close`].
pub struct RunningTunnel {
    snapshot: TunnelSnapshot,
    cancellation: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for RunningTunnel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunningTunnel")
            .field("snapshot", &self.snapshot)
            .finish_non_exhaustive()
    }
}

impl RunningTunnel {
    /// Returns the immutable public tunnel state.
    #[must_use]
    pub fn snapshot(&self) -> &TunnelSnapshot {
        &self.snapshot
    }

    /// Cancels the listener/remote request and waits for SSH cleanup.
    pub async fn close(&self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ForwardTarget {
    host: String,
    port: u16,
    permits: Arc<Semaphore>,
}

pub(crate) type ForwardTargets = Arc<RwLock<HashMap<u32, ForwardTarget>>>;

impl SshSessionFactory {
    /// Starts one local, remote, or dynamic tunnel over an independent SSH connection.
    ///
    /// # Errors
    /// Rejects unsafe exposure without explicit confirmation and reports bounded SSH/listener
    /// startup failures without disclosing credentials.
    pub async fn start_tunnel(&self, spec: TunnelSpec) -> Result<RunningTunnel, TunnelError> {
        validate_spec(&spec)?;
        let targets = Arc::new(RwLock::new(HashMap::new()));
        let (events, _receiver) = mpsc::channel::<SessionEvent>(INTERACTIVE_QUEUE_CAPACITY);
        let chain = self
            .connect_authenticated_with_forwards(events, Some(Arc::clone(&targets)))
            .await?;
        match spec.kind {
            TunnelKind::Local | TunnelKind::Dynamic => start_local_listener(chain, spec).await,
            TunnelKind::Remote => start_remote_forward(chain, spec, targets).await,
        }
    }
}

async fn start_local_listener(
    chain: AuthenticatedChain,
    spec: TunnelSpec,
) -> Result<RunningTunnel, TunnelError> {
    let listener = TcpListener::bind((spec.bind_host.as_str(), spec.bind_port)).await?;
    let local = listener.local_addr()?;
    let snapshot = TunnelSnapshot {
        kind: spec.kind,
        bind_host: local.ip().to_string(),
        bind_port: local.port(),
        target_host: spec.target_host.clone(),
        target_port: spec.target_port,
    };
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let kind = spec.kind;
    let target_port = spec.target_port;
    let task = tokio::spawn(async move {
        let permits = Arc::new(Semaphore::new(MAX_TUNNEL_CONNECTIONS));
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                accepted = listener.accept() => {
                    let Ok((stream, peer)) = accepted else { break };
                    let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else { continue };
                    let client = Arc::clone(&chain.final_client);
                    let target_host = spec.target_host.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        match kind {
                            TunnelKind::Local => {
                                let _ = forward_fixed(stream, peer, client, target_host, target_port).await;
                            }
                            TunnelKind::Dynamic => {
                                let _ = forward_socks5(stream, peer, client).await;
                            }
                            TunnelKind::Remote => {}
                        }
                    });
                }
            }
        }
        chain.disconnect("tunnel closed").await;
    });
    Ok(RunningTunnel {
        snapshot,
        cancellation,
        task: Mutex::new(Some(task)),
    })
}

async fn start_remote_forward(
    chain: AuthenticatedChain,
    spec: TunnelSpec,
    targets: ForwardTargets,
) -> Result<RunningTunnel, TunnelError> {
    let target = ForwardTarget {
        host: spec.target_host.clone(),
        port: spec.target_port,
        permits: Arc::new(Semaphore::new(MAX_TUNNEL_CONNECTIONS)),
    };
    targets
        .write()
        .map_err(|_| TunnelError::StateUnavailable)?
        .insert(spec.bind_port.into(), target);
    let assigned = chain
        .final_client
        .tcpip_forward(spec.bind_host.clone(), spec.bind_port.into())
        .await
        .map_err(TunnelError::Russh)?;
    let effective_port = if spec.bind_port == 0 {
        u16::try_from(assigned).map_err(|_| TunnelError::InvalidEndpoint)?
    } else {
        spec.bind_port
    };
    let snapshot = TunnelSnapshot {
        kind: spec.kind,
        bind_host: spec.bind_host.clone(),
        bind_port: effective_port,
        target_host: spec.target_host,
        target_port: spec.target_port,
    };
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    let task = tokio::spawn(async move {
        stop.cancelled().await;
        let _ = chain
            .final_client
            .cancel_tcpip_forward(spec.bind_host, u32::from(effective_port))
            .await;
        chain.disconnect("remote tunnel closed").await;
    });
    Ok(RunningTunnel {
        snapshot,
        cancellation,
        task: Mutex::new(Some(task)),
    })
}

async fn forward_fixed(
    mut stream: TcpStream,
    peer: SocketAddr,
    client: Arc<russh::client::Handle<super::StrictClientHandler>>,
    host: String,
    port: u16,
) -> Result<(), TunnelError> {
    let channel = client
        .channel_open_direct_tcpip(
            host,
            u32::from(port),
            peer.ip().to_string(),
            peer.port().into(),
        )
        .await
        .map_err(TunnelError::Russh)?;
    let mut remote = channel.into_stream();
    tokio::io::copy_bidirectional(&mut stream, &mut remote).await?;
    remote.shutdown().await?;
    Ok(())
}

async fn forward_socks5(
    mut stream: TcpStream,
    peer: SocketAddr,
    client: Arc<russh::client::Handle<super::StrictClientHandler>>,
) -> Result<(), TunnelError> {
    let mut greeting = [0_u8; 2];
    stream.read_exact(&mut greeting).await?;
    if greeting[0] != 5 || usize::from(greeting[1]) > MAX_SOCKS_METHODS {
        return Err(TunnelError::SocksProtocol);
    }
    let mut methods = vec![0_u8; usize::from(greeting[1])];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        stream.write_all(&[5, 0xff]).await?;
        return Err(TunnelError::SocksAuthentication);
    }
    stream.write_all(&[5, 0]).await?;
    let target = read_socks_target(&mut stream).await?;
    let channel = match client
        .channel_open_direct_tcpip(
            target.host,
            u32::from(target.port),
            peer.ip().to_string(),
            peer.port().into(),
        )
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            let _ = stream.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            return Err(TunnelError::Russh(error));
        }
    };
    stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    let mut remote = channel.into_stream();
    tokio::io::copy_bidirectional(&mut stream, &mut remote).await?;
    remote.shutdown().await?;
    Ok(())
}

async fn read_socks_target(stream: &mut TcpStream) -> Result<ConnectTarget, TunnelError> {
    let mut request = [0_u8; 4];
    stream.read_exact(&mut request).await?;
    if request[0] != 5 || request[1] != 1 || request[2] != 0 {
        let _ = stream.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await;
        return Err(TunnelError::SocksProtocol);
    }
    let host = match request[3] {
        1 => {
            let mut value = [0_u8; 4];
            stream.read_exact(&mut value).await?;
            IpAddr::from(value).to_string()
        }
        3 => {
            let length = stream.read_u8().await?;
            let mut value = vec![0_u8; usize::from(length)];
            stream.read_exact(&mut value).await?;
            String::from_utf8(value).map_err(|_| TunnelError::SocksProtocol)?
        }
        4 => {
            let mut value = [0_u8; 16];
            stream.read_exact(&mut value).await?;
            IpAddr::from(value).to_string()
        }
        _ => return Err(TunnelError::SocksProtocol),
    };
    let port = stream.read_u16().await?;
    ConnectTarget::new(host, port).map_err(|_| TunnelError::InvalidEndpoint)
}

pub(crate) async fn accept_remote_channel(
    channel: Channel<russh::client::Msg>,
    reply: ChannelOpenHandle,
    target: ForwardTarget,
) {
    let Ok(permit) = target.permits.try_acquire_owned() else {
        return;
    };
    let Ok(mut local) = TcpStream::connect((target.host.as_str(), target.port)).await else {
        return;
    };
    reply.accept().await;
    tokio::spawn(async move {
        let _permit = permit;
        let mut remote = channel.into_stream();
        let _ = tokio::io::copy_bidirectional(&mut remote, &mut local).await;
        let _ = remote.shutdown().await;
    });
}

fn validate_spec(spec: &TunnelSpec) -> Result<(), TunnelError> {
    let bind_ip = parse_bind_host(&spec.bind_host)?;
    if !bind_ip.is_loopback() && !spec.allow_non_loopback {
        return Err(TunnelError::PublicBindRequiresConfirmation);
    }
    match spec.kind {
        TunnelKind::Dynamic => {
            if !spec.target_host.is_empty() || spec.target_port != 0 {
                return Err(TunnelError::InvalidEndpoint);
            }
        }
        TunnelKind::Local | TunnelKind::Remote => {
            ConnectTarget::new(spec.target_host.clone(), spec.target_port)
                .map_err(|_| TunnelError::InvalidEndpoint)?;
        }
    }
    Ok(())
}

fn parse_bind_host(host: &str) -> Result<IpAddr, TunnelError> {
    if host.eq_ignore_ascii_case("localhost") {
        Ok(IpAddr::from([127, 0, 0, 1]))
    } else {
        IpAddr::from_str(host).map_err(|_| TunnelError::InvalidEndpoint)
    }
}

/// Redacted tunnel startup/runtime failure.
#[derive(Debug, Error)]
pub enum TunnelError {
    /// SSH setup failed.
    #[error("SSH tunnel setup failed")]
    Ssh(#[from] SshError),
    /// Listener or forwarding I/O failed.
    #[error("tunnel I/O failed")]
    Io(#[from] std::io::Error),
    /// SSH channel or request failed.
    #[error("SSH forwarding request failed")]
    Russh(#[source] russh::Error),
    /// Endpoint fields are invalid or unsupported.
    #[error("invalid tunnel endpoint")]
    InvalidEndpoint,
    /// Non-loopback exposure requires explicit confirmation.
    #[error("non-loopback tunnel bind requires explicit confirmation")]
    PublicBindRequiresConfirmation,
    /// SOCKS5 request is malformed or unsupported.
    #[error("unsupported SOCKS5 request")]
    SocksProtocol,
    /// Dynamic forwarding supports only the no-auth SOCKS5 method on its local listener.
    #[error("SOCKS5 authentication method is unsupported")]
    SocksAuthentication,
    /// Internal remote-forward target state was unavailable.
    #[error("remote tunnel state is unavailable")]
    StateUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_loopback_bind_requires_confirmation() {
        let error = validate_spec(&TunnelSpec {
            kind: TunnelKind::Dynamic,
            bind_host: "0.0.0.0".to_owned(),
            bind_port: 1080,
            target_host: String::new(),
            target_port: 0,
            allow_non_loopback: false,
        })
        .expect_err("public bind must require confirmation");
        assert!(matches!(error, TunnelError::PublicBindRequiresConfirmation));
    }

    #[test]
    fn dynamic_forward_rejects_a_fixed_target() {
        let error = validate_spec(&TunnelSpec {
            kind: TunnelKind::Dynamic,
            bind_host: "127.0.0.1".to_owned(),
            bind_port: 0,
            target_host: "example.com".to_owned(),
            target_port: 443,
            allow_non_loopback: false,
        })
        .expect_err("dynamic tunnel target must come from SOCKS");
        assert!(matches!(error, TunnelError::InvalidEndpoint));
    }
}
