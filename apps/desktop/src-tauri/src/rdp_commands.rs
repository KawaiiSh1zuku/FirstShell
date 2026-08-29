//! Isolated RDP helper lifecycle and native-only credential access.

// Tauri commands deserialize owned arguments at the IPC boundary.
#![allow(clippy::needless_pass_by_value)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use firstshell_domain::{ConnectionId, ConnectionKind, ErrorCode, PublicError};
use firstshell_rdp_protocol::{HelperEvent, HelperState};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};
use tokio::sync::{Mutex, Semaphore, mpsc};
use zeroize::{Zeroize, Zeroizing};

use crate::commands::DesktopState;

const CREDENTIAL_SERVICE: &str = "FirstShell RDP";
const MAX_RDP_SESSIONS: usize = 8;
const MAX_RDP_SECRET_BYTES: usize = firstshell_rdp_protocol::MAX_CREDENTIAL_BYTES;

/// Desktop-owned RDP helper registry.
pub struct DesktopRdp {
    sessions: Mutex<HashMap<String, RdpSessionControl>>,
    slots: Arc<Semaphore>,
}

struct RdpSessionControl {
    stop: mpsc::Sender<()>,
}

impl DesktopRdp {
    /// Creates an empty RDP registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(MAX_RDP_SESSIONS)),
        }
    }

    /// Stops all helper processes during Vault lock or application shutdown.
    pub async fn close_all(&self) {
        let senders = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .drain()
                .map(|(_, control)| control.stop)
                .collect::<Vec<_>>()
        };
        for sender in senders {
            let _ = sender.send(()).await;
        }
    }
}

impl Default for DesktopRdp {
    fn default() -> Self {
        Self::new()
    }
}

/// Initial native RDP viewport.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RdpOpenOptions {
    /// Initial window width in logical pixels.
    pub width: u16,
    /// Initial window height in logical pixels.
    pub height: u16,
}

/// Public RDP session handle.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RdpSessionSnapshot {
    /// Public session identifier.
    pub id: String,
    /// Persisted connection that owns the helper.
    pub connection_id: ConnectionId,
    /// Initial helper lifecycle state.
    pub state: HelperState,
}

/// One helper event scoped to a public session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopRdpEvent {
    /// Public session identifier.
    pub session_id: String,
    /// Redacted helper event.
    pub event: HelperEvent,
}

/// Stores or replaces one RDP password in the OS credential manager.
#[tauri::command]
pub async fn configure_rdp_credential(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
    secret: String,
) -> Result<(), PublicError> {
    authorize(&window)?;
    let connection = find_rdp_connection(&state, connection_id).await?;
    let secret = validated_secret(secret)?;
    let account = credential_account(connection.id);
    tokio::task::spawn_blocking(move || {
        keyring::Entry::new(CREDENTIAL_SERVICE, &account)
            .and_then(|entry| entry.set_password(secret.as_str()))
            .map_err(|_| public(ErrorCode::Internal, "RDP credential could not be stored"))
    })
    .await
    .map_err(|_| public(ErrorCode::Internal, "RDP credential storage stopped"))??;
    Ok(())
}

/// Reports whether a saved RDP connection has a credential without returning it.
#[tauri::command]
pub async fn rdp_credential_status(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
) -> Result<bool, PublicError> {
    authorize(&window)?;
    let connection = find_rdp_connection(&state, connection_id).await?;
    let account = credential_account(connection.id);
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
    .map_err(|_| public(ErrorCode::Internal, "RDP credential status check stopped"))?
}

/// Launches one authenticated helper and companion `FreeRDP` window.
#[tauri::command]
pub async fn open_rdp_session(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
    options: RdpOpenOptions,
    on_event: Channel<DesktopRdpEvent>,
) -> Result<RdpSessionSnapshot, PublicError> {
    authorize(&window)?;
    if options.width < 320 || options.height < 200 {
        return Err(public(ErrorCode::InvalidInput, "RDP viewport is invalid"));
    }
    let connection = find_rdp_connection(&state, connection_id).await?;
    let password = load_credential(connection.id).await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let slot = Arc::clone(&state.rdp.slots)
        .try_acquire_owned()
        .map_err(|_| public(ErrorCode::ResourceExhausted, "RDP session limit reached"))?;

    #[cfg(target_os = "windows")]
    platform::launch(
        Arc::clone(&state.rdp),
        slot,
        platform::LaunchRequest {
            session_id: session_id.clone(),
            host: connection.host,
            port: connection.port,
            username: connection.username,
            password,
            options,
            on_event,
        },
    )
    .await?;

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (connection, password, on_event, slot);
        return Err(public(
            ErrorCode::InvalidState,
            "RDP is supported only on Windows",
        ));
    };

    Ok(RdpSessionSnapshot {
        id: session_id,
        connection_id,
        state: HelperState::Starting,
    })
}

/// Stops one RDP helper and its native child process.
#[tauri::command]
pub async fn close_rdp_session(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    session_id: String,
) -> Result<(), PublicError> {
    authorize(&window)?;
    let control = state.rdp.sessions.lock().await.remove(&session_id);
    if let Some(control) = control {
        control
            .stop
            .send(())
            .await
            .map_err(|_| public(ErrorCode::InvalidState, "RDP helper already stopped"))?;
    }
    Ok(())
}

async fn find_rdp_connection(
    state: &State<'_, DesktopState>,
    connection_id: ConnectionId,
) -> Result<firstshell_domain::ConnectionProfile, PublicError> {
    let connection = state
        .workspace
        .snapshot()
        .await?
        .connections
        .into_iter()
        .find(|connection| connection.id == connection_id)
        .ok_or_else(|| public(ErrorCode::NotFound, "RDP connection not found"))?;
    if connection.kind != ConnectionKind::Rdp {
        return Err(public(
            ErrorCode::InvalidInput,
            "connection is not an RDP profile",
        ));
    }
    Ok(connection)
}

async fn load_credential(connection_id: ConnectionId) -> Result<Zeroizing<String>, PublicError> {
    let account = credential_account(connection_id);
    tokio::task::spawn_blocking(move || {
        keyring::Entry::new(CREDENTIAL_SERVICE, &account)
            .and_then(|entry| entry.get_password())
            .map(Zeroizing::new)
            .map_err(|_| public(ErrorCode::InvalidState, "RDP credential is required"))
    })
    .await
    .map_err(|_| public(ErrorCode::Internal, "RDP credential lookup stopped"))?
}

fn validated_secret(secret: String) -> Result<Zeroizing<String>, PublicError> {
    if secret.is_empty()
        || secret.len() > MAX_RDP_SECRET_BYTES
        || secret.contains(['\r', '\n', '\0'])
    {
        return Err(public(ErrorCode::InvalidInput, "RDP credential is invalid"));
    }
    Ok(Zeroizing::new(secret))
}

fn credential_account(connection_id: ConnectionId) -> String {
    format!("connection:{connection_id}")
}

fn authorize(window: &WebviewWindow) -> Result<(), PublicError> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(public(ErrorCode::InvalidState, "window is not authorized"))
    }
}

fn public(code: ErrorCode, message: impl Into<String>) -> PublicError {
    PublicError::new(code, message)
}

#[cfg(target_os = "windows")]
mod platform {
    use std::path::PathBuf;
    use std::process::Stdio;

    use firstshell_rdp_protocol::{
        CertificatePolicy, DesktopRequest, Handshake, HandshakeResponse, HelperEvent,
        PROTOCOL_VERSION, StartSession, WindowMode, read_frame, write_frame,
    };
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ServerOptions;
    use tokio::process::{Child, Command};
    use tokio::sync::OwnedSemaphorePermit;
    use uuid::Uuid;

    use super::{
        Arc, Channel, DesktopRdp, DesktopRdpEvent, Duration, ErrorCode, PublicError,
        RdpOpenOptions, RdpSessionControl, Zeroize, Zeroizing, mpsc, public,
    };

    pub struct LaunchRequest {
        pub session_id: String,
        pub host: String,
        pub port: u16,
        pub username: String,
        pub password: Zeroizing<String>,
        pub options: RdpOpenOptions,
        pub on_event: Channel<DesktopRdpEvent>,
    }

    pub async fn launch(
        registry: Arc<DesktopRdp>,
        slot: OwnedSemaphorePermit,
        request: LaunchRequest,
    ) -> Result<(), PublicError> {
        let LaunchRequest {
            session_id,
            host,
            port,
            username,
            password,
            options,
            on_event,
        } = request;
        let pipe_name = format!(r"\\.\pipe\firstshell-rdp-{}", Uuid::new_v4());
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&pipe_name)
            .map_err(|_| public(ErrorCode::Internal, "RDP helper pipe could not be created"))?;
        let token = Zeroizing::new(Uuid::new_v4().to_string());
        let mut child = spawn_helper(&pipe_name, token.as_str()).await?;
        tokio::time::timeout(Duration::from_secs(5), server.connect())
            .await
            .map_err(|_| public(ErrorCode::Timeout, "RDP helper connection timed out"))?
            .map_err(|_| public(ErrorCode::Internal, "RDP helper connection failed"))?;
        let mut handshake: Handshake =
            tokio::time::timeout(Duration::from_secs(5), read_frame(&mut server))
                .await
                .map_err(|_| public(ErrorCode::Timeout, "RDP helper handshake timed out"))?
                .map_err(|_| public(ErrorCode::InvalidState, "RDP helper handshake is invalid"))?;
        let accepted = handshake.version == PROTOCOL_VERSION && handshake.token == token.as_str();
        handshake.token.zeroize();
        write_frame(
            &mut server,
            &HandshakeResponse {
                version: PROTOCOL_VERSION,
                accepted,
            },
        )
        .await
        .map_err(|_| public(ErrorCode::Internal, "RDP helper handshake reply failed"))?;
        if !accepted {
            let _ = child.kill().await;
            return Err(public(
                ErrorCode::InvalidState,
                "RDP helper authentication failed",
            ));
        }
        let mut request = DesktopRequest::Start(StartSession {
            session_id: session_id.clone(),
            host,
            port,
            username,
            domain: None,
            password: Some(password.to_string()),
            width: options.width,
            height: options.height,
            window_mode: WindowMode::Companion,
            certificate: CertificatePolicy::Tofu,
        });
        let start_result = write_frame(&mut server, &request).await;
        if let DesktopRequest::Start(start) = &mut request {
            start
                .password
                .as_mut()
                .iter_mut()
                .for_each(|value| value.zeroize());
        }
        start_result.map_err(|_| public(ErrorCode::Internal, "RDP helper start request failed"))?;

        let (stop_tx, stop_rx) = mpsc::channel(1);
        registry
            .sessions
            .lock()
            .await
            .insert(session_id.clone(), RdpSessionControl { stop: stop_tx });
        tauri::async_runtime::spawn(supervise(
            registry, slot, session_id, server, child, stop_rx, on_event,
        ));
        Ok(())
    }

    async fn spawn_helper(pipe_name: &str, token: &str) -> Result<Child, PublicError> {
        let helper = helper_executable()?;
        let mut child = Command::new(helper)
            .arg("--pipe")
            .arg(pipe_name)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| public(ErrorCode::NotFound, "RDP helper executable is unavailable"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| public(ErrorCode::Internal, "RDP helper stdin is unavailable"))?;
        stdin
            .write_all(token.as_bytes())
            .await
            .map_err(|_| public(ErrorCode::Internal, "RDP helper token delivery failed"))?;
        stdin
            .shutdown()
            .await
            .map_err(|_| public(ErrorCode::Internal, "RDP helper token delivery failed"))?;
        Ok(child)
    }

    async fn supervise(
        registry: Arc<DesktopRdp>,
        _slot: OwnedSemaphorePermit,
        session_id: String,
        mut pipe: tokio::net::windows::named_pipe::NamedPipeServer,
        mut child: Child,
        mut stop: mpsc::Receiver<()>,
        on_event: Channel<DesktopRdpEvent>,
    ) {
        loop {
            tokio::select! {
                event = read_frame::<_, HelperEvent>(&mut pipe) => {
                    let Ok(event) = event else { break; };
                    let terminal = matches!(event, HelperEvent::Exited { .. } | HelperEvent::Error { .. });
                    let _ = on_event.send(DesktopRdpEvent { session_id: session_id.clone(), event });
                    if terminal { break; }
                }
                requested = stop.recv() => {
                    if requested.is_some() {
                        let _ = write_frame(&mut pipe, &DesktopRequest::Stop).await;
                    }
                    break;
                }
            }
        }
        if tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        registry.sessions.lock().await.remove(&session_id);
    }

    fn helper_executable() -> Result<PathBuf, PublicError> {
        let current = std::env::current_exe()
            .map_err(|_| public(ErrorCode::Internal, "application path is unavailable"))?;
        let directory = current
            .parent()
            .ok_or_else(|| public(ErrorCode::Internal, "application directory is unavailable"))?;
        let candidate = directory.join("firstshell-rdp-helper.exe");
        if !candidate.is_file() {
            return Err(public(ErrorCode::NotFound, "RDP helper is not installed"));
        }
        Ok(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_validation_should_reject_injection_and_oversize() {
        assert!(validated_secret("secret".to_owned()).is_ok());
        assert!(validated_secret("line\nbreak".to_owned()).is_err());
        assert!(validated_secret("x".repeat(MAX_RDP_SECRET_BYTES + 1)).is_err());
    }
}
