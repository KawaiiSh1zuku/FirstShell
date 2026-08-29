//! Versioned, bounded messages shared with the isolated `Windows` RDP helper.

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Current helper IPC protocol version.
pub const PROTOCOL_VERSION: u16 = 2;
/// Maximum encoded control frame accepted on the local IPC channel.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Maximum password bytes accepted for one RDP launch.
pub const MAX_CREDENTIAL_BYTES: usize = 4096;

/// First authenticated request sent to a newly launched helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    /// Protocol version understood by the desktop process.
    pub version: u16,
    /// Per-launch random token, never persisted.
    pub token: String,
}

/// Desktop response after validating the helper protocol and launch token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// Protocol version selected by the desktop process.
    pub version: u16,
    /// Whether this helper instance is authorized to continue.
    pub accepted: bool,
}

/// How the native RDP window is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    /// A separately managed native window that follows the `FirstShell` session lifecycle.
    Companion,
    /// A native child window attached to a desktop-owned parent HWND.
    Embedded {
        /// Desktop-owned HWND used only as the native parent handle.
        parent_window: u64,
    },
}

/// Non-interactive certificate policy passed to `FreeRDP`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum CertificatePolicy {
    /// Reject certificates that are not already trusted.
    Deny,
    /// Trust the first observed certificate and reject later changes.
    Tofu,
    /// Accept exactly one SHA-256 certificate fingerprint.
    Fingerprint(String),
}

/// One RDP launch request. Its debug representation deliberately omits the password.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSession {
    /// Desktop-generated public session identifier.
    pub session_id: String,
    /// Target DNS name or IP address.
    pub host: String,
    /// Target TCP port.
    pub port: u16,
    /// Remote account name.
    pub username: String,
    /// Optional Windows domain.
    pub domain: Option<String>,
    /// One-shot password delivered only over authenticated local IPC.
    pub password: Option<String>,
    /// Initial logical-pixel width.
    pub width: u16,
    /// Initial logical-pixel height.
    pub height: u16,
    /// Native window presentation.
    pub window_mode: WindowMode,
    /// Non-interactive certificate validation policy.
    pub certificate: CertificatePolicy,
}

impl fmt::Debug for StartSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartSession")
            .field("session_id", &self.session_id)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("domain", &self.domain)
            .field("password", &self.password.as_ref().map(|_| "REDACTED"))
            .field("width", &self.width)
            .field("height", &self.height)
            .field("window_mode", &self.window_mode)
            .field("certificate", &self.certificate)
            .finish()
    }
}

impl StartSession {
    /// Validates all values before they can become `FreeRDP` arguments.
    ///
    /// # Errors
    /// Returns a stable validation error for unsafe, empty, or oversized fields.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_text(&self.session_id, 128, false)?;
        validate_text(&self.host, 253, false)?;
        validate_text(&self.username, 256, true)?;
        if self.port == 0 || self.width < 320 || self.height < 200 {
            return Err(ProtocolError::InvalidMessage);
        }
        if let Some(domain) = &self.domain {
            validate_text(domain, 256, true)?;
        }
        if self
            .password
            .as_ref()
            .is_some_and(|value| value.len() > MAX_CREDENTIAL_BYTES || contains_line_break(value))
        {
            return Err(ProtocolError::InvalidMessage);
        }
        if let CertificatePolicy::Fingerprint(value) = &self.certificate {
            validate_fingerprint(value)?;
        }
        if matches!(self.window_mode, WindowMode::Embedded { parent_window: 0 }) {
            return Err(ProtocolError::InvalidMessage);
        }
        Ok(())
    }
}

/// Desktop-to-helper control messages after authentication.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "payload")]
pub enum DesktopRequest {
    /// Starts the helper's single RDP backend.
    Start(StartSession),
    /// Requests graceful session termination.
    Stop,
    /// Verifies that the helper control channel is responsive.
    Ping,
}

/// Public helper lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperState {
    /// `FreeRDP` is being located and launched.
    Starting,
    /// The supervised `FreeRDP` process is running.
    Running,
    /// The session ended normally.
    Stopped,
    /// The backend or helper failed.
    Failed,
}

/// Helper-to-desktop events. Messages contain no credential material or raw backend output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "payload")]
pub enum HelperEvent {
    /// One lifecycle transition.
    State {
        /// Current helper lifecycle state.
        state: HelperState,
    },
    /// Response to [`DesktopRequest::Ping`].
    Pong,
    /// `FreeRDP` exited.
    Exited {
        /// Native backend exit code, when Windows returned one.
        code: Option<i32>,
        /// Whether the desktop explicitly requested termination.
        requested: bool,
    },
    /// Stable, redacted failure.
    Error {
        /// Stable public diagnostic code.
        code: String,
        /// Redacted user-facing message.
        message: String,
    },
}

/// Bounded framing or message validation failure.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Underlying local IPC failed.
    #[error("RDP helper IPC failed")]
    Io(#[from] std::io::Error),
    /// JSON serialization failed.
    #[error("RDP helper IPC payload is invalid")]
    Json(#[from] serde_json::Error),
    /// A frame was empty or exceeded the fixed control-plane limit.
    #[error("RDP helper IPC frame exceeds the allowed size")]
    InvalidFrame,
    /// A message field failed strict validation.
    #[error("RDP helper IPC message is invalid")]
    InvalidMessage,
}

/// Writes one length-prefixed JSON frame.
///
/// # Errors
/// Returns a bounded protocol error when serialization or transport I/O fails.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value)?;
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::InvalidFrame);
    }
    let length = u32::try_from(payload.len()).map_err(|_| ProtocolError::InvalidFrame)?;
    writer.write_u32_le(length).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one length-prefixed JSON frame without allocating above the fixed limit.
///
/// # Errors
/// Returns a bounded protocol error for invalid lengths, malformed JSON, or transport I/O.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, ProtocolError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = reader.read_u32_le().await? as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(ProtocolError::InvalidFrame);
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
}

fn validate_text(value: &str, maximum: usize, allow_empty: bool) -> Result<(), ProtocolError> {
    if (!allow_empty && value.is_empty()) || value.len() > maximum || contains_line_break(value) {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(())
}

fn contains_line_break(value: &str) -> bool {
    value.contains(['\r', '\n', '\0'])
}

fn validate_fingerprint(value: &str) -> Result<(), ProtocolError> {
    let mut parts = value.split(':');
    let algorithm = parts.next();
    let digest = parts.next();
    if parts.next().is_some()
        || algorithm != Some("sha256")
        || digest.is_none_or(|value| {
            value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(ProtocolError::InvalidMessage);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_start() -> StartSession {
        StartSession {
            session_id: "session-1".to_owned(),
            host: "server.example".to_owned(),
            port: 3389,
            username: "operator".to_owned(),
            domain: None,
            password: Some("secret".to_owned()),
            width: 1280,
            height: 720,
            window_mode: WindowMode::Companion,
            certificate: CertificatePolicy::Tofu,
        }
    }

    #[test]
    fn start_debug_should_redact_password() {
        let output = format!("{:?}", valid_start());
        assert!(!output.contains("secret"));
        assert!(output.contains("REDACTED"));
    }

    #[test]
    fn start_should_reject_argument_injection() {
        let mut request = valid_start();
        request.host = "server.example\n/cert:ignore".to_owned();
        assert!(matches!(
            request.validate(),
            Err(ProtocolError::InvalidMessage)
        ));
    }

    #[test]
    fn fingerprint_should_require_sha256_hex() {
        let mut request = valid_start();
        request.certificate = CertificatePolicy::Fingerprint(format!("sha256:{}", "a5".repeat(32)));
        assert!(request.validate().is_ok());
        request.certificate = CertificatePolicy::Fingerprint("md5:abcd".to_owned());
        assert!(matches!(
            request.validate(),
            Err(ProtocolError::InvalidMessage)
        ));
    }
}
