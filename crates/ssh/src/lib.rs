//! Secure `russh` terminal transport with strict host-key verification.

mod connector;
mod monitor;
mod sftp;
mod tunnel;

pub use connector::{
    BoxedStream, ConnectTarget, Connector, ConnectorError, ProxyDnsMode, ProxyEndpoint, ProxyRoute,
};
pub use monitor::{ProbeError, ProbeKind, ProbeMetric, ProbeResult, ProbeRow, ProbeSession};
pub use sftp::{
    ArchiveReceipt, ConflictPolicy, RemoteEditDocument, RemoteEntry, RemoteEntryKind, SftpError,
    SftpWorkspace, TransferOptions, TransferProgress, TransferReceipt, VerifyMode,
};
pub use tunnel::{RunningTunnel, TunnelError, TunnelKind, TunnelSnapshot, TunnelSpec};

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use encoding_rs::{BIG5, Encoding, GBK, UTF_8};
use firstshell_session::{
    ConnectedTransport, INTERACTIVE_QUEUE_CAPACITY, LocalTerminalFactory, SessionError,
    SessionEvent, SessionFactory, TerminalSize, TransportCommand,
};
use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentStream;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use thiserror::Error;
use tokio::sync::mpsc;
use tunnel::ForwardTargets;
use zeroize::Zeroizing;

const MAX_BANNER_BYTES: usize = 8 * 1024;
const KEEPALIVE_SECONDS: u64 = 20;
const INACTIVITY_SECONDS: u64 = 120;
const MAX_JUMP_HOSTS: usize = 4;

/// One capability exercised by the M0 `russh` spike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshSpikeCapability {
    /// Interactive PTY channel is available.
    Terminal,
    /// Server identity can be verified before authentication.
    HostKey,
    /// Password and public-key authentication hooks are available.
    Authentication,
    /// `SFTP` can run on a separately scheduled channel.
    Sftp,
}

const REQUIRED_CAPABILITIES: [SshSpikeCapability; 4] = [
    SshSpikeCapability::Terminal,
    SshSpikeCapability::HostKey,
    SshSpikeCapability::Authentication,
    SshSpikeCapability::Sftp,
];

/// Capability set verified by an `SSH` compatibility spike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshSpikeCapabilities {
    supported: &'static [SshSpikeCapability],
}

impl SshSpikeCapabilities {
    /// Returns the capability set required by the frozen architecture.
    #[must_use]
    pub const fn required() -> Self {
        Self {
            supported: &REQUIRED_CAPABILITIES,
        }
    }

    /// Returns true only when all critical capabilities are present.
    #[must_use]
    pub fn is_complete(self) -> bool {
        REQUIRED_CAPABILITIES
            .iter()
            .all(|required| self.supported.contains(required))
    }
}

/// Terminal byte encoding negotiated by a user profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEncoding {
    /// UTF-8, the default for modern systems.
    Utf8,
    /// GB18030-compatible Chinese encoding.
    Gb18030,
    /// Traditional Chinese Big5 encoding.
    Big5,
}

impl TerminalEncoding {
    fn codec(self) -> &'static Encoding {
        match self {
            Self::Utf8 => UTF_8,
            Self::Gb18030 => GBK,
            Self::Big5 => BIG5,
        }
    }
}

/// Authentication material held only inside the trusted Rust process.
pub enum SshAuthentication {
    /// Password authentication.
    Password(Zeroizing<String>),
    /// OpenSSH private key loaded from a local path.
    PrivateKey {
        /// Private key path selected by the user.
        path: PathBuf,
        /// Optional passphrase obtained from a native prompt.
        passphrase: Option<Zeroizing<String>>,
    },
    /// Use a local OpenSSH-compatible agent without forwarding it.
    Agent,
    /// Answer non-echoing keyboard-interactive prompts from native secret input.
    KeyboardInteractive(Zeroizing<String>),
}

/// One independently authenticated and verified SSH jump host.
pub struct JumpHostSpec {
    /// DNS name or IP address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Remote account.
    pub username: String,
    /// Authentication selected for this hop.
    pub authentication: SshAuthentication,
    /// Enables the bounded per-host legacy algorithm exception.
    pub allow_legacy_algorithms: bool,
}

impl std::fmt::Debug for JumpHostSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JumpHostSpec")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("authentication", &self.authentication)
            .field("allow_legacy_algorithms", &self.allow_legacy_algorithms)
            .finish()
    }
}

impl std::fmt::Debug for SshAuthentication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password(_) => formatter.write_str("Password(REDACTED)"),
            Self::PrivateKey { path, .. } => formatter
                .debug_struct("PrivateKey")
                .field("path", path)
                .field("passphrase", &"REDACTED")
                .finish(),
            Self::Agent => formatter.write_str("Agent"),
            Self::KeyboardInteractive(_) => formatter.write_str("KeyboardInteractive(REDACTED)"),
        }
    }
}

/// Immutable endpoint and terminal behavior for a fresh connection.
#[derive(Debug)]
pub struct SshConnectSpec {
    /// DNS name or IP address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Remote account.
    pub username: String,
    /// Authentication selected for this profile.
    pub authentication: SshAuthentication,
    /// Application-owned OpenSSH `known_hosts` file.
    pub known_hosts: PathBuf,
    /// Output/input encoding.
    pub encoding: TerminalEncoding,
    /// Enables the bounded per-host legacy algorithm exception.
    pub allow_legacy_algorithms: bool,
    /// Network route resolved before the SSH handshake.
    pub route: ProxyRoute,
    /// Ordered SSH jump hosts. The route applies only to the first network hop.
    pub jump_hosts: Vec<JumpHostSpec>,
}

/// Public host-key evidence displayed before a trust decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyEvidence {
    /// Host from the immutable connection specification.
    pub host: String,
    /// Port from the immutable connection specification.
    pub port: u16,
    /// Negotiated host-key algorithm.
    pub algorithm: String,
    /// SHA-256 fingerprint.
    pub fingerprint: String,
    /// Whether a key with this algorithm changed from the stored value.
    pub changed: bool,
    /// Previous SHA-256 fingerprint for a changed key.
    pub previous_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
enum HostKeyApproval {
    TrustNew {
        host: String,
        port: u16,
        fingerprint: String,
    },
    ReplaceChanged {
        host: String,
        port: u16,
        old_fingerprint: String,
        new_fingerprint: String,
    },
}

/// Reusable factory used by the session actor for initial and explicit reconnect.
pub struct SshSessionFactory {
    spec: SshConnectSpec,
    pending: Arc<Mutex<Option<HostKeyEvidence>>>,
    approval: Arc<Mutex<Option<HostKeyApproval>>>,
}

/// Secret-free platform OpenSSH fallback configuration.
#[derive(Debug, Clone)]
pub struct OpenSshFallbackSpec {
    /// DNS name or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Remote user.
    pub username: String,
    /// Application-owned `known_hosts` path.
    pub known_hosts: PathBuf,
    /// Optional unencrypted private key path. Omit to use the agent.
    pub private_key: Option<PathBuf>,
}

/// Pure Rust primary transport with a narrowly gated platform fallback.
pub struct SshCompatibilityFactory {
    primary: Arc<SshSessionFactory>,
    fallback: Option<LocalTerminalFactory>,
}

impl SshCompatibilityFactory {
    /// Builds a compatibility wrapper. Invalid fallback arguments disable the fallback.
    #[must_use]
    pub fn new(primary: Arc<SshSessionFactory>, fallback: Option<OpenSshFallbackSpec>) -> Self {
        let fallback = fallback.and_then(|spec| {
            let mut arguments = vec![
                "-F".to_owned(),
                null_device().to_owned(),
                "-oBatchMode=yes".to_owned(),
                "-oStrictHostKeyChecking=yes".to_owned(),
                format!(
                    "-oUserKnownHostsFile={}",
                    spec.known_hosts.to_string_lossy()
                ),
                "-oConnectTimeout=30".to_owned(),
                "-oServerAliveInterval=20".to_owned(),
                "-oServerAliveCountMax=3".to_owned(),
                "-oLogLevel=ERROR".to_owned(),
                "-p".to_owned(),
                spec.port.to_string(),
            ];
            if let Some(path) = spec.private_key {
                arguments.push("-oIdentitiesOnly=yes".to_owned());
                arguments.push("-i".to_owned());
                arguments.push(path.to_string_lossy().into_owned());
            }
            arguments.push("--".to_owned());
            arguments.push(format!("{}@{}", spec.username, spec.host));
            LocalTerminalFactory::openssh(arguments).ok()
        });
        Self { primary, fallback }
    }

    /// Returns the strict primary factory for host-key approvals.
    #[must_use]
    pub fn primary(&self) -> &Arc<SshSessionFactory> {
        &self.primary
    }
}

#[async_trait]
impl SessionFactory for SshCompatibilityFactory {
    async fn connect(&self, size: TerminalSize) -> Result<ConnectedTransport, SessionError> {
        match self.primary.connect(size).await {
            Err(error) if should_use_openssh_fallback(&error) => match self.fallback.as_ref() {
                Some(fallback) => fallback.connect(size).await,
                None => Err(SessionError::transport(
                    "SSH_CONNECTION_FAILED",
                    "SSH transport failed",
                )),
            },
            result => result,
        }
    }
}

fn should_use_openssh_fallback(error: &SessionError) -> bool {
    matches!(
        error,
        SessionError::Transport {
            code: "SSH_CONNECTION_FAILED",
            ..
        }
    )
}

const fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

impl SshSessionFactory {
    /// Creates a strict SSH factory. Unknown and changed keys are rejected.
    #[must_use]
    pub fn new(spec: SshConnectSpec) -> Self {
        Self {
            spec,
            pending: Arc::new(Mutex::new(None)),
            approval: Arc::new(Mutex::new(None)),
        }
    }

    /// Returns the last rejected host-key evidence.
    #[must_use]
    pub fn pending_host_key(&self) -> Option<HostKeyEvidence> {
        self.pending.lock().ok().and_then(|pending| pending.clone())
    }

    /// Allows exactly one subsequent connection to persist an unknown key.
    ///
    /// # Errors
    /// Rejects stale or altered fingerprints.
    pub fn approve_unknown(&self, fingerprint: &str) -> Result<(), SshError> {
        let pending = self
            .pending_host_key()
            .filter(|evidence| !evidence.changed)
            .ok_or(SshError::NoPendingHostKey)?;
        if pending.fingerprint != fingerprint {
            return Err(SshError::HostKeyApprovalMismatch);
        }
        *self
            .approval
            .lock()
            .map_err(|_| SshError::StateUnavailable)? = Some(HostKeyApproval::TrustNew {
            host: pending.host,
            port: pending.port,
            fingerprint: fingerprint.to_owned(),
        });
        Ok(())
    }

    /// Allows exactly one subsequent connection to replace a changed host key.
    ///
    /// # Errors
    /// Requires both fingerprints to match the current file and pending key.
    pub fn approve_replacement(
        &self,
        old_fingerprint: &str,
        new_fingerprint: &str,
    ) -> Result<(), SshError> {
        let pending = self
            .pending_host_key()
            .filter(|evidence| evidence.changed)
            .ok_or(SshError::NoPendingHostKey)?;
        if pending.fingerprint != new_fingerprint {
            return Err(SshError::HostKeyApprovalMismatch);
        }
        *self
            .approval
            .lock()
            .map_err(|_| SshError::StateUnavailable)? = Some(HostKeyApproval::ReplaceChanged {
            host: pending.host,
            port: pending.port,
            old_fingerprint: old_fingerprint.to_owned(),
            new_fingerprint: new_fingerprint.to_owned(),
        });
        Ok(())
    }
}

/// Detects whether a private key requires a passphrase without returning key material.
///
/// # Errors
/// Returns a redacted key error for unreadable or malformed files.
pub fn private_key_requires_passphrase(path: impl AsRef<Path>) -> Result<bool, SshError> {
    match russh::keys::load_secret_key(path, None) {
        Ok(_) => Ok(false),
        Err(russh::keys::Error::KeyIsEncrypted) => Ok(true),
        Err(error) => Err(SshError::Key(error)),
    }
}

#[async_trait]
impl SessionFactory for SshSessionFactory {
    async fn connect(&self, size: TerminalSize) -> Result<ConnectedTransport, SessionError> {
        self.connect_inner(size)
            .await
            .map_err(SshError::into_session)
    }
}

impl SshSessionFactory {
    async fn connect_inner(&self, size: TerminalSize) -> Result<ConnectedTransport, SshError> {
        let (event_tx, event_rx) = mpsc::channel(INTERACTIVE_QUEUE_CAPACITY);
        let AuthenticatedChain {
            final_client: client,
            jump_clients,
        } = self.connect_authenticated(event_tx.clone()).await?;
        let channel = client
            .channel_open_session()
            .await
            .map_err(SshError::Russh)?;
        channel
            .request_pty(
                true,
                "xterm-256color",
                u32::from(size.cols),
                u32::from(size.rows),
                u32::from(size.pixel_width),
                u32::from(size.pixel_height),
                &[],
            )
            .await
            .map_err(SshError::Russh)?;
        channel.request_shell(true).await.map_err(SshError::Russh)?;
        let (mut reader, writer) = channel.split();
        let (command_tx, mut command_rx) = mpsc::channel(INTERACTIVE_QUEUE_CAPACITY);
        let encoding = self.spec.encoding.codec();

        tokio::spawn(async move {
            let mut decoder = encoding.new_decoder();
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        let result = match command {
                            Some(TransportCommand::Input(data)) => {
                                let input = encode_terminal_input(encoding, &data);
                                writer.data_bytes(input).await
                            }
                            Some(TransportCommand::Resize(next)) => writer.window_change(
                                u32::from(next.cols),
                                u32::from(next.rows),
                                u32::from(next.pixel_width),
                                u32::from(next.pixel_height),
                            ).await,
                            Some(TransportCommand::Close) | None => {
                                let _ = writer.close().await;
                                let _ = client.disconnect(
                                    Disconnect::ByApplication,
                                    "session closed",
                                    "en-US",
                                ).await;
                                for jump in jump_clients.iter().rev() {
                                    let _ = jump.disconnect(
                                        Disconnect::ByApplication,
                                        "jump session closed",
                                        "en-US",
                                    ).await;
                                }
                                break;
                            }
                        };
                        if result.is_err() {
                            let _ = event_tx.send(SessionEvent::Error {
                                code: "SSH_WRITE_FAILED".to_owned(),
                                message: "SSH terminal write failed".to_owned(),
                            }).await;
                            break;
                        }
                    }
                    message = reader.wait() => {
                        match message {
                            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                                let output = decode_terminal_output(&mut decoder, &data);
                                if !output.is_empty()
                                    && event_tx.send(SessionEvent::Output { data: output }).await.is_err()
                                {
                                    break;
                                }
                            }
                            Some(ChannelMsg::ExitStatus { exit_status }) => {
                                let _ = event_tx.send(SessionEvent::Exit { code: Some(exit_status) }).await;
                                break;
                            }
                            Some(ChannelMsg::Close) | None => {
                                let _ = event_tx.send(SessionEvent::Exit { code: None }).await;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(ConnectedTransport {
            commands: command_tx,
            events: event_rx,
        })
    }

    async fn connect_authenticated(
        &self,
        event_tx: mpsc::Sender<SessionEvent>,
    ) -> Result<AuthenticatedChain, SshError> {
        self.connect_authenticated_with_forwards(event_tx, None)
            .await
    }

    async fn connect_authenticated_with_forwards(
        &self,
        event_tx: mpsc::Sender<SessionEvent>,
        forward_targets: Option<ForwardTargets>,
    ) -> Result<AuthenticatedChain, SshError> {
        self.validate_endpoints()?;
        if let Ok(mut pending) = self.pending.lock() {
            *pending = None;
        }
        let first = self
            .spec
            .jump_hosts
            .first()
            .map_or_else(|| EndpointRef::final_target(&self.spec), EndpointRef::jump);
        let target = ConnectTarget::new(first.host.to_owned(), first.port)?;
        let stream = Connector::new(self.spec.route.clone())
            .connect(&target)
            .await?;
        let first_forward_targets = self
            .spec
            .jump_hosts
            .is_empty()
            .then(|| forward_targets.clone())
            .flatten();
        let mut client = self
            .connect_endpoint(stream, first, event_tx.clone(), first_forward_targets)
            .await?;
        let mut jump_clients = Vec::with_capacity(self.spec.jump_hosts.len());
        for endpoint in self.spec.jump_hosts.iter().skip(1).map(EndpointRef::jump) {
            let channel = client
                .channel_open_direct_tcpip(endpoint.host, u32::from(endpoint.port), "127.0.0.1", 0)
                .await
                .map_err(SshError::Russh)?;
            let stream = BoxedStream::new(channel.into_stream());
            jump_clients.push(Arc::new(client));
            client = self
                .connect_endpoint(stream, endpoint, event_tx.clone(), None)
                .await?;
        }
        if !self.spec.jump_hosts.is_empty() {
            let endpoint = EndpointRef::final_target(&self.spec);
            let channel = client
                .channel_open_direct_tcpip(endpoint.host, u32::from(endpoint.port), "127.0.0.1", 0)
                .await
                .map_err(SshError::Russh)?;
            let stream = BoxedStream::new(channel.into_stream());
            jump_clients.push(Arc::new(client));
            client = self
                .connect_endpoint(stream, endpoint, event_tx, forward_targets)
                .await?;
        }
        Ok(AuthenticatedChain {
            final_client: Arc::new(client),
            jump_clients,
        })
    }

    async fn connect_endpoint(
        &self,
        stream: BoxedStream,
        endpoint: EndpointRef<'_>,
        events: mpsc::Sender<SessionEvent>,
        forward_targets: Option<ForwardTargets>,
    ) -> Result<client::Handle<StrictClientHandler>, SshError> {
        let handler = self.client_handler(endpoint.host, endpoint.port, events, forward_targets);
        let config = Arc::new(ssh_client_config(endpoint.allow_legacy_algorithms));
        let mut client = client::connect_stream(config, stream, handler)
            .await
            .map_err(|error| self.map_connect_error(error))?;
        authenticate(&mut client, endpoint.username, endpoint.authentication).await?;
        Ok(client)
    }

    fn validate_endpoints(&self) -> Result<(), SshError> {
        if self.spec.jump_hosts.len() > MAX_JUMP_HOSTS {
            return Err(SshError::InvalidEndpoint);
        }
        self.spec
            .jump_hosts
            .iter()
            .map(EndpointRef::jump)
            .chain(std::iter::once(EndpointRef::final_target(&self.spec)))
            .try_for_each(validate_endpoint)
    }

    fn client_handler(
        &self,
        host: &str,
        port: u16,
        events: mpsc::Sender<SessionEvent>,
        forward_targets: Option<ForwardTargets>,
    ) -> StrictClientHandler {
        StrictClientHandler {
            host: host.to_owned(),
            port,
            known_hosts: self.spec.known_hosts.clone(),
            pending: Arc::clone(&self.pending),
            approval: Arc::clone(&self.approval),
            events,
            forward_targets,
        }
    }

    fn map_connect_error(&self, error: russh::Error) -> SshError {
        if let Some(evidence) = self.pending_host_key() {
            if evidence.changed {
                SshError::HostKeyChanged {
                    algorithm: evidence.algorithm,
                    fingerprint: evidence.fingerprint,
                }
            } else {
                SshError::HostKeyUnknown {
                    algorithm: evidence.algorithm,
                    fingerprint: evidence.fingerprint,
                }
            }
        } else {
            SshError::Russh(error)
        }
    }
}

#[derive(Clone)]
struct AuthenticatedChain {
    final_client: Arc<client::Handle<StrictClientHandler>>,
    jump_clients: Vec<Arc<client::Handle<StrictClientHandler>>>,
}

impl AuthenticatedChain {
    async fn disconnect(&self, reason: &'static str) {
        let _ = self
            .final_client
            .disconnect(Disconnect::ByApplication, reason, "en-US")
            .await;
        for jump in self.jump_clients.iter().rev() {
            let _ = jump
                .disconnect(Disconnect::ByApplication, reason, "en-US")
                .await;
        }
    }
}

#[derive(Clone, Copy)]
struct EndpointRef<'a> {
    host: &'a str,
    port: u16,
    username: &'a str,
    authentication: &'a SshAuthentication,
    allow_legacy_algorithms: bool,
}

impl<'a> EndpointRef<'a> {
    fn jump(spec: &'a JumpHostSpec) -> Self {
        Self {
            host: &spec.host,
            port: spec.port,
            username: &spec.username,
            authentication: &spec.authentication,
            allow_legacy_algorithms: spec.allow_legacy_algorithms,
        }
    }

    fn final_target(spec: &'a SshConnectSpec) -> Self {
        Self {
            host: &spec.host,
            port: spec.port,
            username: &spec.username,
            authentication: &spec.authentication,
            allow_legacy_algorithms: spec.allow_legacy_algorithms,
        }
    }
}

fn validate_endpoint(endpoint: EndpointRef<'_>) -> Result<(), SshError> {
    if endpoint.host.is_empty()
        || endpoint.host.len() > 253
        || endpoint.host.starts_with('-')
        || endpoint
            .host
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        || endpoint.username.is_empty()
        || endpoint.username.len() > 256
        || endpoint
            .username
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        || endpoint.port == 0
    {
        return Err(SshError::InvalidEndpoint);
    }
    Ok(())
}

fn ssh_client_config(allow_legacy_algorithms: bool) -> client::Config {
    client::Config {
        preferred: preferred_algorithms(allow_legacy_algorithms),
        inactivity_timeout: Some(Duration::from_secs(INACTIVITY_SECONDS)),
        keepalive_interval: Some(Duration::from_secs(KEEPALIVE_SECONDS)),
        keepalive_max: 3,
        nodelay: true,
        ..client::Config::default()
    }
}

fn preferred_algorithms(allow_legacy: bool) -> russh::Preferred {
    let mut preferred = russh::Preferred::default();
    let mut keys = preferred
        .key
        .iter()
        .filter(|algorithm| !matches!(algorithm, russh::keys::Algorithm::Rsa { hash: None }))
        .cloned()
        .collect::<Vec<_>>();
    if allow_legacy {
        keys.push(russh::keys::Algorithm::Rsa { hash: None });
        let mut kex = preferred.kex.to_vec();
        kex.push(russh::kex::DH_G14_SHA1);
        preferred.kex = Cow::Owned(kex);
        let mut ciphers = preferred.cipher.to_vec();
        ciphers.push(russh::cipher::AES_256_CBC);
        preferred.cipher = Cow::Owned(ciphers);
        let mut macs = preferred.mac.to_vec();
        macs.push(russh::mac::HMAC_SHA1_ETM);
        preferred.mac = Cow::Owned(macs);
    }
    preferred.key = Cow::Owned(keys);
    preferred
}

async fn authenticate(
    client: &mut client::Handle<StrictClientHandler>,
    username: &str,
    authentication: &SshAuthentication,
) -> Result<(), SshError> {
    let result = match authentication {
        SshAuthentication::Password(password) => client
            .authenticate_password(username, password.as_str())
            .await
            .map_err(SshError::Russh)?,
        SshAuthentication::PrivateKey { path, passphrase } => {
            let path = path.clone();
            let passphrase = passphrase.clone();
            let key = tokio::task::spawn_blocking(move || {
                russh::keys::load_secret_key(path, passphrase.as_deref().map(String::as_str))
            })
            .await
            .map_err(|_| SshError::PrivateKeyUnavailable)?
            .map_err(SshError::Key)?;
            let hash_algorithm = if key.algorithm().is_rsa() {
                client
                    .best_supported_rsa_hash()
                    .await
                    .map_err(SshError::Russh)?
                    .flatten()
                    .or(Some(HashAlg::Sha256))
            } else {
                None
            };
            client
                .authenticate_publickey(
                    username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash_algorithm),
                )
                .await
                .map_err(SshError::Russh)?
        }
        SshAuthentication::Agent => authenticate_agent(client, username).await?,
        SshAuthentication::KeyboardInteractive(response) => {
            authenticate_keyboard_interactive(client, username, response).await?
        }
    };
    if result.success() {
        Ok(())
    } else {
        Err(SshError::AuthenticationFailed)
    }
}

async fn authenticate_keyboard_interactive(
    client: &mut client::Handle<StrictClientHandler>,
    username: &str,
    response: &Zeroizing<String>,
) -> Result<AuthResult, SshError> {
    let mut reply = client
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(SshError::Russh)?;
    for _ in 0..8 {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(AuthResult::Success),
            KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                return Ok(AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                });
            }
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                if prompts.iter().any(|prompt| prompt.echo) {
                    return Err(SshError::UnsupportedInteractivePrompt);
                }
                reply = client
                    .authenticate_keyboard_interactive_respond(
                        prompts
                            .iter()
                            .map(|_| response.as_str().to_owned())
                            .collect(),
                    )
                    .await
                    .map_err(SshError::Russh)?;
            }
        }
    }
    Err(SshError::TooManyInteractivePrompts)
}

#[cfg(windows)]
async fn authenticate_agent(
    client: &mut client::Handle<StrictClientHandler>,
    username: &str,
) -> Result<AuthResult, SshError> {
    if let Ok(mut agent) =
        russh::keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
    {
        return authenticate_agent_identities(client, username, &mut agent).await;
    }
    let mut agent = russh::keys::agent::client::AgentClient::connect_pageant()
        .await
        .map_err(SshError::Key)?;
    authenticate_agent_identities(client, username, &mut agent).await
}

#[cfg(unix)]
async fn authenticate_agent(
    client: &mut client::Handle<StrictClientHandler>,
    username: &str,
) -> Result<AuthResult, SshError> {
    let mut agent = russh::keys::agent::client::AgentClient::connect_env()
        .await
        .map_err(SshError::Key)?;
    authenticate_agent_identities(client, username, &mut agent).await
}

async fn authenticate_agent_identities<S>(
    client: &mut client::Handle<StrictClientHandler>,
    username: &str,
    agent: &mut russh::keys::agent::client::AgentClient<S>,
) -> Result<AuthResult, SshError>
where
    S: AgentStream + Send + Unpin,
{
    let identities = agent.request_identities().await.map_err(SshError::Key)?;
    for identity in identities {
        if let AgentIdentity::PublicKey { key, .. } = identity {
            let hash_algorithm = if key.algorithm().is_rsa() {
                client
                    .best_supported_rsa_hash()
                    .await
                    .map_err(SshError::Russh)?
                    .flatten()
                    .or(Some(HashAlg::Sha256))
            } else {
                None
            };
            let result = client
                .authenticate_publickey_with(username, key, hash_algorithm, agent)
                .await
                .map_err(SshError::AgentAuth)?;
            if result.success() {
                return Ok(result);
            }
        }
    }
    Err(SshError::AgentRejected)
}

fn encode_terminal_input(encoding: &'static Encoding, input: &[u8]) -> Vec<u8> {
    if encoding == UTF_8 {
        return input.to_vec();
    }
    let text = String::from_utf8_lossy(input);
    let (encoded, _, _) = encoding.encode(&text);
    encoded.into_owned()
}

fn decode_terminal_output(decoder: &mut encoding_rs::Decoder, input: &[u8]) -> Vec<u8> {
    let capacity = decoder
        .max_utf8_buffer_length(input.len())
        .unwrap_or(input.len().saturating_mul(4).saturating_add(16));
    let mut output = String::with_capacity(capacity);
    let _ = decoder.decode_to_string(input, &mut output, false);
    output.into_bytes()
}

struct StrictClientHandler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    pending: Arc<Mutex<Option<HostKeyEvidence>>>,
    approval: Arc<Mutex<Option<HostKeyApproval>>>,
    events: mpsc::Sender<SessionEvent>,
    forward_targets: Option<ForwardTargets>,
}

impl client::Handler for StrictClientHandler {
    type Error = russh::Error;

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let text = sanitize_banner(banner);
        if !text.is_empty() {
            let _ = self.events.try_send(SessionEvent::Banner { text });
        }
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = server_public_key.public_key();
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let algorithm = key.algorithm().to_string();
        match russh::keys::check_known_hosts_path(&self.host, self.port, &key, &self.known_hosts) {
            Ok(true) => Ok(true),
            Ok(false) => {
                let approved = self
                    .approval
                    .lock()
                    .ok()
                    .and_then(|mut approval| approval.take());
                if matches!(approved, Some(HostKeyApproval::TrustNew { ref host, port, fingerprint: ref expected }) if host == &self.host && port == self.port && expected == &fingerprint)
                {
                    russh::keys::known_hosts::learn_known_hosts_path(
                        &self.host,
                        self.port,
                        &key,
                        &self.known_hosts,
                    )
                    .map_err(russh::Error::Keys)?;
                    return Ok(true);
                }
                self.record_pending(algorithm, fingerprint, false, None);
                Ok(false)
            }
            Err(russh::keys::Error::KeyChanged { line }) => {
                let approved = self
                    .approval
                    .lock()
                    .ok()
                    .and_then(|mut approval| approval.take());
                if let Some(HostKeyApproval::ReplaceChanged {
                    host,
                    port,
                    old_fingerprint,
                    new_fingerprint,
                }) = approved
                    && host == self.host
                    && port == self.port
                    && new_fingerprint == fingerprint
                    && replace_changed_key(
                        &self.known_hosts,
                        line,
                        &old_fingerprint,
                        &self.host,
                        self.port,
                        &key,
                    )
                    .is_ok()
                {
                    return Ok(true);
                }
                let previous_fingerprint = stored_fingerprint(&self.known_hosts, line);
                self.record_pending(algorithm, fingerprint, true, previous_fingerprint);
                Ok(false)
            }
            Err(_) => Ok(false),
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let target = self
            .forward_targets
            .as_ref()
            .and_then(|targets| targets.read().ok())
            .and_then(|targets| {
                targets
                    .get(&connected_port)
                    .or_else(|| targets.get(&0))
                    .cloned()
            });
        if let Some(target) = target {
            tunnel::accept_remote_channel(channel, reply, target).await;
        }
        Ok(())
    }
}

impl StrictClientHandler {
    fn record_pending(
        &self,
        algorithm: String,
        fingerprint: String,
        changed: bool,
        previous_fingerprint: Option<String>,
    ) {
        if let Ok(mut pending) = self.pending.lock() {
            *pending = Some(HostKeyEvidence {
                host: self.host.clone(),
                port: self.port,
                algorithm,
                fingerprint,
                changed,
                previous_fingerprint,
            });
        }
    }
}

fn stored_fingerprint(path: &Path, line_number: usize) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let line = content.lines().nth(line_number.checked_sub(1)?)?;
    let key = russh::keys::parse_public_key_base64(line.split_whitespace().nth(2)?).ok()?;
    Some(key.fingerprint(HashAlg::Sha256).to_string())
}

fn replace_changed_key(
    path: &Path,
    line_number: usize,
    old_fingerprint: &str,
    host: &str,
    port: u16,
    new_key: &russh::keys::PublicKey,
) -> Result<(), SshError> {
    let content = std::fs::read_to_string(path).map_err(SshError::Io)?;
    let mut lines = content.lines().map(str::to_owned).collect::<Vec<_>>();
    let index = line_number
        .checked_sub(1)
        .ok_or(SshError::HostKeyApprovalMismatch)?;
    let old_line = lines.get(index).ok_or(SshError::HostKeyApprovalMismatch)?;
    let old_key_text = old_line
        .split_whitespace()
        .nth(2)
        .ok_or(SshError::HostKeyApprovalMismatch)?;
    let old_key = russh::keys::parse_public_key_base64(old_key_text).map_err(SshError::Key)?;
    if old_key.fingerprint(HashAlg::Sha256).to_string() != old_fingerprint {
        return Err(SshError::HostKeyApprovalMismatch);
    }
    let host_field = if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    };
    lines[index] = format!(
        "{host_field} {}",
        new_key
            .to_openssh()
            .map_err(|error| SshError::Key(error.into()))?
    );
    let replacement = format!("{}\n", lines.join("\n"));
    std::fs::write(path, replacement).map_err(SshError::Io)
}

fn sanitize_banner(banner: &str) -> String {
    banner
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        .take(MAX_BANNER_BYTES)
        .collect()
}

/// Redacted SSH failure classification.
#[derive(Debug, Error)]
pub enum SshError {
    /// Endpoint fields were empty or out of bounds.
    #[error("invalid SSH endpoint")]
    InvalidEndpoint,
    /// First-use key requires explicit confirmation.
    #[error("unknown host key: {algorithm} {fingerprint}")]
    HostKeyUnknown {
        /// Negotiated algorithm.
        algorithm: String,
        /// SHA-256 fingerprint.
        fingerprint: String,
    },
    /// Stored and presented host keys differ.
    #[error("host key changed: {algorithm} {fingerprint}")]
    HostKeyChanged {
        /// Negotiated algorithm.
        algorithm: String,
        /// New SHA-256 fingerprint.
        fingerprint: String,
    },
    /// Authentication was rejected without exposing credentials.
    #[error("SSH authentication failed")]
    AuthenticationFailed,
    /// Agent was unavailable or every identity was rejected.
    #[error("SSH agent did not provide an accepted identity")]
    AgentRejected,
    /// Echoing interactive prompts require a separately designed safe flow.
    #[error("server requested an unsupported echoing authentication prompt")]
    UnsupportedInteractivePrompt,
    /// Server exceeded the bounded keyboard-interactive exchange.
    #[error("server sent too many authentication prompts")]
    TooManyInteractivePrompts,
    /// Private key loading task could not complete.
    #[error("private key could not be loaded")]
    PrivateKeyUnavailable,
    /// No rejected key is available for approval.
    #[error("no pending host key")]
    NoPendingHostKey,
    /// Approval does not match the pending evidence.
    #[error("host-key approval did not match current evidence")]
    HostKeyApprovalMismatch,
    /// Internal synchronized state was unavailable.
    #[error("SSH verifier state is unavailable")]
    StateUnavailable,
    /// `russh` protocol error.
    #[error("SSH transport failed")]
    Russh(#[source] russh::Error),
    /// Direct or proxy stream creation failed.
    #[error("SSH connector failed")]
    Connector(#[from] ConnectorError),
    /// SSH key or agent error.
    #[error("SSH key operation failed")]
    Key(#[source] russh::keys::Error),
    /// SSH agent signing exchange failed.
    #[error("SSH agent authentication exchange failed")]
    AgentAuth(#[source] russh::AgentAuthError),
    /// Known-host file operation failed.
    #[error("known-host storage failed")]
    Io(#[source] std::io::Error),
}

impl SshError {
    fn into_session(self) -> SessionError {
        match self {
            Self::HostKeyUnknown {
                algorithm,
                fingerprint,
            } => SessionError::transport(
                "HOST_KEY_UNKNOWN",
                format!("Unknown host key {algorithm} {fingerprint}"),
            ),
            Self::HostKeyChanged {
                algorithm,
                fingerprint,
            } => SessionError::transport(
                "HOST_KEY_CHANGED",
                format!("Host key changed to {algorithm} {fingerprint}"),
            ),
            Self::AuthenticationFailed => {
                SessionError::transport("AUTHENTICATION_FAILED", self.to_string())
            }
            Self::AgentRejected | Self::AgentAuth(_) => {
                SessionError::transport("SSH_AGENT_REJECTED", self.to_string())
            }
            Self::InvalidEndpoint => SessionError::transport("INVALID_ENDPOINT", self.to_string()),
            Self::UnsupportedInteractivePrompt | Self::TooManyInteractivePrompts => {
                SessionError::transport("INTERACTIVE_AUTH_REJECTED", self.to_string())
            }
            Self::PrivateKeyUnavailable | Self::Key(_) => {
                SessionError::transport("PRIVATE_KEY_UNAVAILABLE", self.to_string())
            }
            Self::NoPendingHostKey
            | Self::HostKeyApprovalMismatch
            | Self::StateUnavailable
            | Self::Io(_) => SessionError::transport("HOST_KEY_STATE_FAILED", self.to_string()),
            Self::Connector(ConnectorError::ProxyAuthentication) => {
                SessionError::transport("PROXY_AUTHENTICATION_FAILED", self.to_string())
            }
            Self::Connector(ConnectorError::Timeout) => {
                SessionError::transport("PROXY_TIMEOUT", self.to_string())
            }
            Self::Connector(_) => {
                SessionError::transport("SSH_CONNECTION_FAILED", self.to_string())
            }
            Self::Russh(_) => SessionError::transport("SSH_CONNECTION_FAILED", self.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_spike_capabilities_should_be_complete() {
        assert!(SshSpikeCapabilities::required().is_complete());
    }

    #[test]
    fn banner_should_be_bounded_and_strip_terminal_controls() {
        let banner = format!("safe\u{1b}[31m{}", "x".repeat(MAX_BANNER_BYTES * 2));
        let sanitized = sanitize_banner(&banner);
        assert!(!sanitized.contains('\u{1b}'));
        assert_eq!(sanitized.chars().count(), MAX_BANNER_BYTES);
    }

    #[test]
    fn legacy_encoding_should_round_trip_cjk_input() {
        let encoded = encode_terminal_input(GBK, "中文输入".as_bytes());
        let mut decoder = GBK.new_decoder();
        assert_eq!(
            String::from_utf8(decode_terminal_output(&mut decoder, &encoded))
                .expect("decoder output is UTF-8"),
            "中文输入"
        );
    }

    #[test]
    fn weak_algorithms_should_require_per_host_exception() {
        let strict = preferred_algorithms(false);
        assert!(
            !strict
                .key
                .contains(&russh::keys::Algorithm::Rsa { hash: None })
        );
        assert!(!strict.kex.contains(&russh::kex::DH_G14_SHA1));
        let legacy = preferred_algorithms(true);
        assert!(
            legacy
                .key
                .contains(&russh::keys::Algorithm::Rsa { hash: None })
        );
        assert_eq!(
            legacy.key.last(),
            Some(&russh::keys::Algorithm::Rsa { hash: None })
        );
        assert_eq!(legacy.kex.last(), Some(&russh::kex::DH_G14_SHA1));
    }

    #[test]
    fn authentication_debug_output_should_redact_secrets() {
        let authentication =
            SshAuthentication::Password(Zeroizing::new("never-print-this-password".to_owned()));
        let output = format!("{authentication:?}");
        assert!(output.contains("REDACTED"));
        assert!(!output.contains("never-print-this-password"));
    }

    #[test]
    fn openssh_fallback_should_only_handle_connection_failures() {
        assert!(should_use_openssh_fallback(&SessionError::transport(
            "SSH_CONNECTION_FAILED",
            "redacted"
        )));
        assert!(!should_use_openssh_fallback(&SessionError::transport(
            "AUTHENTICATION_FAILED",
            "redacted"
        )));
        assert!(!should_use_openssh_fallback(&SessionError::transport(
            "HOST_KEY_CHANGED",
            "redacted"
        )));
    }
}
