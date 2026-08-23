//! Versioned messages shared with the isolated `Windows` RDP helper.

use serde::{Deserialize, Serialize};

/// Current helper IPC protocol version.
pub const PROTOCOL_VERSION: u16 = 1;

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
