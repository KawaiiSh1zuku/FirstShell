//! Bounded authentication handshake shared by relay clients and `relayd`.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Current relay protocol version.
pub const PROTOCOL_VERSION: u16 = 1;
/// ALPN used by both QUIC and TCP/TLS relay transports.
pub const ALPN: &[u8] = b"firstshell-relay/1";
/// Maximum authentication frame size.
pub const MAX_FRAME_BYTES: usize = 16 * 1024;

/// Authenticated request for one server-defined target mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientHello {
    /// Protocol version offered by the client.
    pub version: u16,
    /// Opaque bearer token delivered only inside TLS.
    pub token: String,
    /// Server-configured mapping name, never an arbitrary client target.
    pub mapping: String,
}

impl ClientHello {
    /// Rejects malformed or injection-prone handshake fields.
    ///
    /// # Errors
    /// Returns [`ProtocolError::InvalidMessage`] for invalid values.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION
            || self.token.is_empty()
            || self.token.len() > 4096
            || self.mapping.is_empty()
            || self.mapping.len() > 128
            || self.token.contains(['\r', '\n', '\0'])
            || !self
                .mapping
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(ProtocolError::InvalidMessage);
        }
        Ok(())
    }
}

/// Relay response before opaque byte forwarding begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerHello {
    /// Selected protocol version.
    pub version: u16,
    /// Whether forwarding may begin.
    pub accepted: bool,
    /// Stable redacted result code.
    pub code: String,
    /// Public audit correlation identifier when accepted.
    pub session_id: Option<String>,
}

/// Relay control-plane framing error.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Transport read or write failed.
    #[error("relay transport failed")]
    Io(#[from] std::io::Error),
    /// Encoded payload was invalid.
    #[error("relay handshake payload is invalid")]
    Json(#[from] serde_json::Error),
    /// Frame length was zero or exceeded its fixed limit.
    #[error("relay handshake frame is invalid")]
    InvalidFrame,
    /// A handshake field violated protocol constraints.
    #[error("relay handshake message is invalid")]
    InvalidMessage,
}

/// Writes one bounded length-prefixed JSON frame.
///
/// # Errors
/// Returns a protocol error when serialization, bounds checks, or transport I/O fail.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value)?;
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::InvalidFrame);
    }
    writer
        .write_u32_le(u32::try_from(payload.len()).map_err(|_| ProtocolError::InvalidFrame)?)
        .await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one bounded length-prefixed JSON frame.
///
/// # Errors
/// Returns a protocol error for invalid lengths, malformed JSON, or transport I/O.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, ProtocolError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let length = reader.read_u32_le().await? as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(ProtocolError::InvalidFrame);
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_should_reject_arbitrary_target_syntax() {
        let value = ClientHello {
            version: PROTOCOL_VERSION,
            token: "secret".to_owned(),
            mapping: "host:22\nother".to_owned(),
        };
        assert!(matches!(
            value.validate(),
            Err(ProtocolError::InvalidMessage)
        ));
    }
}
