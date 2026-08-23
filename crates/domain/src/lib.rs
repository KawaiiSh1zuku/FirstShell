//! Core `FirstShell` types and invariants with no infrastructure dependencies.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Stable identifier for a connection profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionId(Uuid);

impl ConnectionId {
    /// Creates a cryptographically random connection identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable, non-secret connection metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    /// Stable record identifier.
    pub id: ConnectionId,
    /// Monotonic optimistic-concurrency revision.
    pub revision: u64,
    /// Connection transport category.
    pub kind: ConnectionKind,
}

/// Supported top-level connection transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    /// Secure Shell connection.
    Ssh,
    /// `Windows` Remote Desktop connection.
    Rdp,
    /// Local terminal session.
    Local,
}

/// Stable error codes safe to cross the desktop IPC boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Caller supplied malformed or out-of-range data.
    InvalidInput,
    /// Requested operation is invalid for the current state.
    InvalidState,
    /// A bounded queue cannot accept more work.
    ResourceExhausted,
    /// The operation exceeded its deadline.
    Timeout,
    /// A lower-level component failed without safe public details.
    Internal,
}

/// Error payload intended for UI display and IPC serialization.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[error("{code:?}: {message}")]
pub struct PublicError {
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// Redacted human-readable summary.
    pub message: String,
    /// Correlation identifier for internal diagnostics.
    pub correlation_id: Uuid,
}

impl PublicError {
    /// Creates a redacted public error with a fresh correlation identifier.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            correlation_id: Uuid::new_v4(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_id_should_be_unique_for_new_values() {
        assert_ne!(ConnectionId::new(), ConnectionId::new());
    }

    #[test]
    fn public_error_should_assign_a_non_nil_correlation_id() {
        let error = PublicError::new(ErrorCode::InvalidInput, "invalid port");
        assert_ne!(error.correlation_id, Uuid::nil());
    }
}
