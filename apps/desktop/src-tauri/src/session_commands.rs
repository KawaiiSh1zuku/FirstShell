//! Terminal session commands and native-only credential collection.

// Tauri commands deserialize owned arguments at the IPC boundary.
#![allow(clippy::needless_pass_by_value)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use firstshell_domain::{
    ConnectionId, ConnectionKind, ErrorCode, PublicError, SshAuthenticationKind,
    TerminalEncoding as ProfileEncoding,
};
use firstshell_session::{
    LocalShell, LocalTerminalFactory, SessionError, SessionEvent, SessionEventSink, SessionId,
    SessionRegistry, SessionSnapshot, TerminalSize,
};
use firstshell_ssh::{
    HostKeyEvidence, OpenSshFallbackSpec, SshAuthentication, SshCompatibilityFactory,
    SshConnectSpec, SshSessionFactory, TerminalEncoding, private_key_requires_passphrase,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use zeroize::{Zeroize, Zeroizing};

use crate::commands::DesktopState;

const CREDENTIAL_SERVICE: &str = "FirstShell SSH";
const LOG_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_SSH_SECRET_BYTES: usize = 4096;

type LogSenders = Arc<Mutex<HashMap<SessionId, mpsc::Sender<Vec<u8>>>>>;

/// Desktop-owned terminal registry and adapter state.
pub struct DesktopSessions {
    registry: SessionRegistry,
    ssh_factories: Mutex<HashMap<SessionId, Arc<SshSessionFactory>>>,
    log_senders: LogSenders,
    known_hosts: PathBuf,
    log_directory: PathBuf,
}

impl DesktopSessions {
    /// Creates a desktop session boundary rooted in controlled app-data paths.
    #[must_use]
    pub fn new(known_hosts: PathBuf, log_directory: PathBuf) -> Self {
        purge_old_logs(&log_directory);
        Self {
            registry: SessionRegistry::new(),
            ssh_factories: Mutex::new(HashMap::new()),
            log_senders: Arc::new(Mutex::new(HashMap::new())),
            known_hosts,
            log_directory,
        }
    }

    /// Closes all sessions and drops pending factories/log writers on vault lock.
    pub async fn close_all(&self) {
        self.registry.close_all().await;
        if let Ok(mut factories) = self.ssh_factories.lock() {
            factories.clear();
        }
        if let Ok(mut logs) = self.log_senders.lock() {
            logs.clear();
        }
    }
}

/// Session event envelope carried over one Tauri channel.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSessionEvent {
    /// Opaque session identifier.
    session_id: SessionId,
    /// Public actor event.
    event: SessionEvent,
}

/// Public host-key evidence for an explicit trust dialog.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopHostKeyEvidence {
    host: String,
    port: u16,
    algorithm: String,
    fingerprint: String,
    changed: bool,
    previous_fingerprint: Option<String>,
}

impl From<HostKeyEvidence> for DesktopHostKeyEvidence {
    fn from(value: HostKeyEvidence) -> Self {
        Self {
            host: value.host,
            port: value.port,
            algorithm: value.algorithm,
            fingerprint: value.fingerprint,
            changed: value.changed,
            previous_fingerprint: value.previous_fingerprint,
        }
    }
}

/// Session log activation receipt.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLogReceipt {
    /// Absolute controlled path shown to the user.
    path: String,
}

/// Non-persistent quick-connect request. It cannot contain credential bytes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickConnectInput {
    host: String,
    port: u16,
    username: String,
    authentication: SshAuthenticationKind,
    private_key_path: String,
    allow_legacy_algorithms: bool,
    openssh_fallback: bool,
}

fn authorize(window: &WebviewWindow) -> Result<(), PublicError> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(public(
            ErrorCode::InvalidState,
            "window is not authorized for terminal access",
        ))
    }
}

#[tauri::command]
pub async fn configure_connection_credential(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
) -> Result<bool, PublicError> {
    authorize(&window)?;
    let connection = find_connection(&state, connection_id).await?;
    if connection.kind != ConnectionKind::Ssh
        || !matches!(
            connection.authentication,
            SshAuthenticationKind::Password | SshAuthenticationKind::KeyboardInteractive
        )
    {
        return Err(public(
            ErrorCode::InvalidState,
            "connection does not use a stored SSH secret",
        ));
    }
    let account = credential_account(connection_id);
    tokio::task::spawn_blocking(move || {
        let Some(secret) = tinyfiledialogs::password_box(
            "FirstShell SSH credential",
            "Enter the SSH secret. It is sent directly to the OS credential store and never returned to the WebView.",
        ) else {
            return Ok(false);
        };
        let secret = Zeroizing::new(secret);
        if secret.is_empty() || secret.len() > 4096 {
            return Err(public(ErrorCode::InvalidInput, "SSH secret length is invalid"));
        }
        keyring::Entry::new(CREDENTIAL_SERVICE, &account)
            .and_then(|entry| entry.set_password(secret.as_str()))
            .map_err(|_| public(ErrorCode::Internal, "OS credential store rejected the SSH secret"))?;
        Ok(true)
    })
    .await
    .map_err(|_| public(ErrorCode::Internal, "native credential prompt stopped"))?
}

#[tauri::command]
pub async fn connection_credential_status(
    window: WebviewWindow,
    connection_id: ConnectionId,
) -> Result<bool, PublicError> {
    authorize(&window)?;
    let account = credential_account(connection_id);
    tokio::task::spawn_blocking(move || {
        Ok(keyring::Entry::new(CREDENTIAL_SERVICE, &account)
            .and_then(|entry| entry.get_password())
            .map(|mut secret| {
                secret.zeroize();
                true
            })
            .unwrap_or(false))
    })
    .await
    .map_err(|_| public(ErrorCode::Internal, "credential status check stopped"))?
}

#[tauri::command]
pub async fn open_terminal_session(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
    size: TerminalSize,
    on_event: Channel<DesktopSessionEvent>,
) -> Result<SessionSnapshot, PublicError> {
    authorize(&window)?;
    let connection = find_connection(&state, connection_id).await?;
    let profile = state
        .workspace
        .snapshot()
        .await?
        .terminal_profiles
        .into_iter()
        .find(|profile| Some(profile.id) == connection.terminal_profile_id);
    let encoding = match profile.map(|profile| profile.encoding).unwrap_or_default() {
        ProfileEncoding::Utf8 => TerminalEncoding::Utf8,
        ProfileEncoding::Gb18030 => TerminalEncoding::Gb18030,
        ProfileEncoding::Big5 => TerminalEncoding::Big5,
    };
    let sink = desktop_session_sink(Arc::clone(&state.sessions.log_senders), on_event);

    match connection.kind {
        ConnectionKind::Ssh => {
            let authentication = resolve_authentication(&connection).await?;
            let factory = Arc::new(SshSessionFactory::new(SshConnectSpec {
                host: connection.host.clone(),
                port: connection.port,
                username: connection.username.clone(),
                authentication,
                known_hosts: state.sessions.known_hosts.clone(),
                encoding,
                allow_legacy_algorithms: connection.allow_legacy_algorithms,
            }));
            let fallback = connection.openssh_fallback.then(|| OpenSshFallbackSpec {
                host: connection.host,
                port: connection.port,
                username: connection.username,
                known_hosts: state.sessions.known_hosts.clone(),
                private_key: None,
            });
            let compatible = Arc::new(SshCompatibilityFactory::new(factory.clone(), fallback));
            let snapshot = state
                .sessions
                .registry
                .open(compatible, size, sink)
                .await
                .map_err(map_session)?;
            state
                .sessions
                .ssh_factories
                .lock()
                .map_err(|_| public(ErrorCode::Internal, "session state is unavailable"))?
                .insert(snapshot.id, factory);
            Ok(snapshot)
        }
        ConnectionKind::Local => state
            .sessions
            .registry
            .open(
                Arc::new(LocalTerminalFactory::new(default_local_shell(), None)),
                size,
                sink,
            )
            .await
            .map_err(map_session),
        ConnectionKind::Rdp => Err(public(
            ErrorCode::InvalidState,
            "RDP sessions are delivered in M6",
        )),
    }
}

#[tauri::command]
pub async fn open_quick_terminal(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    input: QuickConnectInput,
    size: TerminalSize,
    on_event: Channel<DesktopSessionEvent>,
) -> Result<SessionSnapshot, PublicError> {
    authorize(&window)?;
    validate_quick_input(&input)?;
    let authentication = resolve_quick_authentication(&input).await?;
    let sink = desktop_session_sink(Arc::clone(&state.sessions.log_senders), on_event);
    let primary = Arc::new(SshSessionFactory::new(SshConnectSpec {
        host: input.host.clone(),
        port: input.port,
        username: input.username.clone(),
        authentication,
        known_hosts: state.sessions.known_hosts.clone(),
        encoding: TerminalEncoding::Utf8,
        allow_legacy_algorithms: input.allow_legacy_algorithms,
    }));
    let fallback = input.openssh_fallback.then(|| OpenSshFallbackSpec {
        host: input.host,
        port: input.port,
        username: input.username,
        known_hosts: state.sessions.known_hosts.clone(),
        private_key: None,
    });
    let compatible = Arc::new(SshCompatibilityFactory::new(primary.clone(), fallback));
    let snapshot = state
        .sessions
        .registry
        .open(compatible, size, sink)
        .await
        .map_err(map_session)?;
    state
        .sessions
        .ssh_factories
        .lock()
        .map_err(|_| public(ErrorCode::Internal, "session state is unavailable"))?
        .insert(snapshot.id, primary);
    Ok(snapshot)
}

#[tauri::command]
pub async fn list_terminal_sessions(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<SessionSnapshot>, PublicError> {
    authorize(&window)?;
    Ok(state.sessions.registry.list().await)
}

#[tauri::command]
pub async fn send_terminal_input(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
    data: Vec<u8>,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state
        .sessions
        .registry
        .send_input(session_id, data)
        .await
        .map_err(map_session)
}

#[tauri::command]
pub async fn resize_terminal(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
    size: TerminalSize,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state
        .sessions
        .registry
        .resize(session_id, size)
        .await
        .map_err(map_session)
}

#[tauri::command]
pub async fn reconnect_terminal(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state
        .sessions
        .registry
        .reconnect(session_id)
        .await
        .map_err(map_session)
}

#[tauri::command]
pub async fn close_terminal(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
) -> Result<(), PublicError> {
    authorize(&window)?;
    if let Ok(mut factories) = state.sessions.ssh_factories.lock() {
        factories.remove(&session_id);
    }
    if let Ok(mut logs) = state.sessions.log_senders.lock() {
        logs.remove(&session_id);
    }
    state
        .sessions
        .registry
        .close(session_id)
        .await
        .map_err(map_session)
}

#[tauri::command]
pub fn pending_host_key(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
) -> Result<Option<DesktopHostKeyEvidence>, PublicError> {
    authorize(&window)?;
    let factories = state
        .sessions
        .ssh_factories
        .lock()
        .map_err(|_| public(ErrorCode::Internal, "session state is unavailable"))?;
    Ok(factories
        .get(&session_id)
        .and_then(|factory| factory.pending_host_key())
        .map(Into::into))
}

#[tauri::command]
pub async fn approve_host_key(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
    fingerprint: String,
    previous_fingerprint: Option<String>,
) -> Result<(), PublicError> {
    authorize(&window)?;
    if fingerprint.len() > 256
        || previous_fingerprint
            .as_ref()
            .is_some_and(|value| value.len() > 256)
    {
        return Err(public(
            ErrorCode::InvalidInput,
            "host-key fingerprint is invalid",
        ));
    }
    {
        let factories = state
            .sessions
            .ssh_factories
            .lock()
            .map_err(|_| public(ErrorCode::Internal, "session state is unavailable"))?;
        let factory = factories
            .get(&session_id)
            .ok_or_else(|| public(ErrorCode::NotFound, "SSH session not found"))?;
        if let Some(previous) = previous_fingerprint {
            factory
                .approve_replacement(&previous, &fingerprint)
                .map_err(|_| public(ErrorCode::Conflict, "host-key evidence changed"))?;
        } else {
            factory
                .approve_unknown(&fingerprint)
                .map_err(|_| public(ErrorCode::Conflict, "host-key evidence changed"))?;
        }
    }
    state
        .sessions
        .registry
        .reconnect(session_id)
        .await
        .map_err(map_session)
}

#[tauri::command]
pub async fn start_session_log(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
) -> Result<SessionLogReceipt, PublicError> {
    authorize(&window)?;
    if !state
        .sessions
        .registry
        .list()
        .await
        .iter()
        .any(|snapshot| snapshot.id == session_id)
    {
        return Err(public(ErrorCode::NotFound, "terminal session not found"));
    }
    tokio::fs::create_dir_all(&state.sessions.log_directory)
        .await
        .map_err(|_| public(ErrorCode::Internal, "session log directory is unavailable"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let path = state
        .sessions
        .log_directory
        .join(format!("{timestamp}-{session_id}.log"));
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(|_| public(ErrorCode::Internal, "session log could not be created"))?;
    let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(64);
    state
        .sessions
        .log_senders
        .lock()
        .map_err(|_| public(ErrorCode::Internal, "session log state is unavailable"))?
        .insert(session_id, sender);
    tokio::spawn(async move {
        let mut file = file;
        while let Some(data) = receiver.recv().await {
            if file.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = file.flush().await;
    });
    Ok(SessionLogReceipt {
        path: path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub fn stop_session_log(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: SessionId,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state
        .sessions
        .log_senders
        .lock()
        .map_err(|_| public(ErrorCode::Internal, "session log state is unavailable"))?
        .remove(&session_id);
    Ok(())
}

async fn find_connection(
    state: &State<'_, DesktopState>,
    id: ConnectionId,
) -> Result<firstshell_domain::ConnectionProfile, PublicError> {
    state
        .workspace
        .snapshot()
        .await?
        .connections
        .into_iter()
        .find(|connection| connection.id == id)
        .ok_or_else(|| public(ErrorCode::NotFound, "connection not found"))
}

async fn resolve_authentication(
    connection: &firstshell_domain::ConnectionProfile,
) -> Result<SshAuthentication, PublicError> {
    match connection.authentication {
        SshAuthenticationKind::Agent => Ok(SshAuthentication::Agent),
        SshAuthenticationKind::Password | SshAuthenticationKind::KeyboardInteractive => {
            let account = credential_account(connection.id);
            let secret = tokio::task::spawn_blocking(move || {
                keyring::Entry::new(CREDENTIAL_SERVICE, &account)
                    .and_then(|entry| entry.get_password())
            })
            .await
            .map_err(|_| public(ErrorCode::Internal, "credential lookup stopped"))?
            .map_err(|_| {
                public(
                    ErrorCode::InvalidState,
                    "configure the native SSH credential before connecting",
                )
            })?;
            let secret = validated_native_secret(secret)?;
            if connection.authentication == SshAuthenticationKind::Password {
                Ok(SshAuthentication::Password(secret))
            } else {
                Ok(SshAuthentication::KeyboardInteractive(secret))
            }
        }
        SshAuthenticationKind::PrivateKey => {
            let path = PathBuf::from(&connection.private_key_path);
            let check_path = path.clone();
            let encrypted =
                tokio::task::spawn_blocking(move || private_key_requires_passphrase(check_path))
                    .await
                    .map_err(|_| public(ErrorCode::Internal, "private-key inspection stopped"))?
                    .map_err(|_| {
                        public(
                            ErrorCode::InvalidInput,
                            "private key is unavailable or invalid",
                        )
                    })?;
            let passphrase = if encrypted {
                Some(
                    tokio::task::spawn_blocking(|| {
                        tinyfiledialogs::password_box(
                            "FirstShell private key",
                            "Enter the private-key passphrase. It stays in the Rust process and is not stored.",
                        )
                    })
                    .await
                    .map_err(|_| public(ErrorCode::Internal, "native passphrase prompt stopped"))?
                    .ok_or_else(|| public(ErrorCode::InvalidState, "private-key prompt was cancelled"))
                    .and_then(validated_native_secret)?,
                )
            } else {
                None
            };
            Ok(SshAuthentication::PrivateKey { path, passphrase })
        }
    }
}

async fn resolve_quick_authentication(
    input: &QuickConnectInput,
) -> Result<SshAuthentication, PublicError> {
    match input.authentication {
        SshAuthenticationKind::Agent => Ok(SshAuthentication::Agent),
        SshAuthenticationKind::Password | SshAuthenticationKind::KeyboardInteractive => {
            let secret = tokio::task::spawn_blocking(|| {
                tinyfiledialogs::password_box(
                    "FirstShell temporary SSH credential",
                    "Enter the one-time SSH secret. It stays in the Rust process and is not stored.",
                )
            })
            .await
            .map_err(|_| public(ErrorCode::Internal, "native credential prompt stopped"))?
            .ok_or_else(|| public(ErrorCode::InvalidState, "credential prompt was cancelled"))?;
            let secret = validated_native_secret(secret)?;
            if input.authentication == SshAuthenticationKind::Password {
                Ok(SshAuthentication::Password(secret))
            } else {
                Ok(SshAuthentication::KeyboardInteractive(secret))
            }
        }
        SshAuthenticationKind::PrivateKey => {
            let path = PathBuf::from(&input.private_key_path);
            let check_path = path.clone();
            let encrypted =
                tokio::task::spawn_blocking(move || private_key_requires_passphrase(check_path))
                    .await
                    .map_err(|_| public(ErrorCode::Internal, "private-key inspection stopped"))?
                    .map_err(|_| {
                        public(
                            ErrorCode::InvalidInput,
                            "private key is unavailable or invalid",
                        )
                    })?;
            let passphrase = if encrypted {
                Some(
                    tokio::task::spawn_blocking(|| {
                        tinyfiledialogs::password_box(
                            "FirstShell private key",
                            "Enter the one-time private-key passphrase. It is not stored.",
                        )
                    })
                    .await
                    .map_err(|_| public(ErrorCode::Internal, "native passphrase prompt stopped"))?
                    .ok_or_else(|| {
                        public(ErrorCode::InvalidState, "private-key prompt was cancelled")
                    })
                    .and_then(validated_native_secret)?,
                )
            } else {
                None
            };
            Ok(SshAuthentication::PrivateKey { path, passphrase })
        }
    }
}

fn validate_quick_input(input: &QuickConnectInput) -> Result<(), PublicError> {
    if input.host.is_empty()
        || input.host.len() > 253
        || input.host.starts_with('-')
        || input
            .host
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        || input.port == 0
        || input.username.is_empty()
        || input.username.len() > 128
        || input
            .username
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        || input.private_key_path.len() > 4096
    {
        return Err(public(
            ErrorCode::InvalidInput,
            "quick connection is invalid",
        ));
    }
    if input.authentication == SshAuthenticationKind::PrivateKey
        && input.private_key_path.is_empty()
    {
        return Err(public(
            ErrorCode::InvalidInput,
            "private key path is required",
        ));
    }
    if input.openssh_fallback && input.authentication != SshAuthenticationKind::Agent {
        return Err(public(
            ErrorCode::InvalidInput,
            "OpenSSH fallback requires agent authentication",
        ));
    }
    Ok(())
}

fn validated_native_secret(secret: String) -> Result<Zeroizing<String>, PublicError> {
    let secret = Zeroizing::new(secret);
    if secret.is_empty() || secret.len() > MAX_SSH_SECRET_BYTES {
        return Err(public(
            ErrorCode::InvalidInput,
            "SSH secret length is invalid",
        ));
    }
    Ok(secret)
}

fn credential_account(id: ConnectionId) -> String {
    format!("ssh:{id}")
}

fn desktop_session_sink(
    logs: LogSenders,
    on_event: Channel<DesktopSessionEvent>,
) -> Arc<dyn SessionEventSink> {
    Arc::new(move |session_id, event: SessionEvent| {
        if let SessionEvent::Output { data } = &event
            && let Ok(logs) = logs.lock()
            && let Some(sender) = logs.get(&session_id)
        {
            let _ = sender.try_send(data.clone());
        }
        let _ = on_event.send(DesktopSessionEvent { session_id, event });
    })
}

#[cfg(windows)]
const fn default_local_shell() -> LocalShell {
    LocalShell::WindowsPowerShell
}

#[cfg(not(windows))]
const fn default_local_shell() -> LocalShell {
    LocalShell::Bash
}

fn map_session(error: SessionError) -> PublicError {
    match error {
        SessionError::ResourceExhausted => public(ErrorCode::ResourceExhausted, error.to_string()),
        SessionError::InputTooLarge | SessionError::InvalidSize => {
            public(ErrorCode::InvalidInput, error.to_string())
        }
        SessionError::NotFound => public(ErrorCode::NotFound, error.to_string()),
        SessionError::Closed => public(ErrorCode::InvalidState, error.to_string()),
        SessionError::Transport { .. } | SessionError::Local(_) => {
            public(ErrorCode::Internal, error.to_string())
        }
    }
}

fn public(code: ErrorCode, message: impl Into<String>) -> PublicError {
    PublicError::new(code, message)
}

fn purge_old_logs(directory: &PathBuf) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("log") {
            continue;
        }
        let expired = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > LOG_RETENTION);
        if expired {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_accounts_should_be_scoped_by_connection() {
        assert_eq!(CREDENTIAL_SERVICE, "FirstShell SSH");
        assert!(credential_account(ConnectionId::new()).starts_with("ssh:"));
    }

    #[test]
    fn quick_connections_should_reject_unsafe_or_secret_bearing_fallbacks() {
        let mut input = QuickConnectInput {
            host: "server.internal".to_owned(),
            port: 22,
            username: "operator".to_owned(),
            authentication: SshAuthenticationKind::Agent,
            private_key_path: String::new(),
            allow_legacy_algorithms: false,
            openssh_fallback: true,
        };
        assert!(validate_quick_input(&input).is_ok());

        input.username = "operator\nother".to_owned();
        assert!(validate_quick_input(&input).is_err());
        input.username = "operator".to_owned();
        input.authentication = SshAuthenticationKind::Password;
        assert!(validate_quick_input(&input).is_err());
    }

    #[test]
    fn native_ssh_secrets_should_be_bounded() {
        assert!(validated_native_secret("secret".to_owned()).is_ok());
        assert!(validated_native_secret(String::new()).is_err());
        assert!(validated_native_secret("x".repeat(MAX_SSH_SECRET_BYTES + 1)).is_err());
    }
}
