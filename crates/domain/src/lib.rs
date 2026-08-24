//! Core `FirstShell` types and invariants with no infrastructure dependencies.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

macro_rules! entity_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a cryptographically random identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Returns the canonical hyphenated identifier.
            #[must_use]
            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

entity_id!(ConnectionId, "Stable identifier for a connection profile.");
entity_id!(FolderId, "Stable identifier for a connection folder.");
entity_id!(ProxyId, "Stable identifier for a proxy profile.");
entity_id!(
    TerminalProfileId,
    "Stable identifier for a terminal profile."
);

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

/// Non-secret SSH authentication selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthenticationKind {
    /// Use the platform SSH agent without forwarding it.
    #[default]
    Agent,
    /// Load a password from the native credential broker.
    Password,
    /// Load a local private key and obtain its passphrase from a native prompt.
    PrivateKey,
    /// Use a native secret response for keyboard-interactive authentication.
    KeyboardInteractive,
}

/// Supported terminal wire encodings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalEncoding {
    /// Modern UTF-8 terminals.
    #[default]
    Utf8,
    /// GB18030-compatible Chinese terminals.
    Gb18030,
    /// Traditional Chinese Big5 terminals.
    Big5,
}

/// Terminal keyboard compatibility profiles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKeymap {
    /// xterm-compatible defaults.
    #[default]
    Xterm,
    /// Windows-oriented word deletion and clipboard bindings.
    Windows,
    /// Emacs-style meta and control bindings.
    Emacs,
}

/// Supported proxy transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyKind {
    /// No proxy.
    Direct,
    /// `SOCKS5` proxy.
    Socks5,
    /// `HTTP CONNECT` proxy.
    HttpConnect,
    /// Resolve the system proxy once when opening the session.
    System,
}

/// Proxy-side target name resolution policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyDnsMode {
    /// Send the target hostname to the proxy.
    #[default]
    Remote,
    /// Resolve the target locally before proxy negotiation.
    Local,
}

/// Connection-tree folder with decrypted display fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// Stable record identifier.
    pub id: FolderId,
    /// Optional parent folder.
    pub parent_id: Option<FolderId>,
    /// Monotonic optimistic-concurrency revision.
    pub revision: u64,
    /// Stable ordering within the parent.
    pub sort_order: i64,
    /// User-visible name, available only while unlocked.
    pub name: String,
    /// Optional private notes.
    pub notes: String,
}

/// Input used to create or update a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInput {
    /// Optional parent folder.
    pub parent_id: Option<FolderId>,
    /// Stable ordering within the parent.
    pub sort_order: i64,
    /// User-visible name.
    pub name: String,
    /// Optional private notes.
    pub notes: String,
}

/// Decrypted connection profile. Authentication secrets are referenced, never embedded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    /// Stable record identifier.
    pub id: ConnectionId,
    /// Optional containing folder.
    pub folder_id: Option<FolderId>,
    /// Monotonic optimistic-concurrency revision.
    pub revision: u64,
    /// Stable ordering within the folder.
    pub sort_order: i64,
    /// Connection transport category.
    pub kind: ConnectionKind,
    /// User-visible name.
    pub name: String,
    /// Host name or address. Empty for local terminals.
    pub host: String,
    /// Remote port. Zero for local terminals.
    pub port: u16,
    /// Remote account name.
    pub username: String,
    /// Optional proxy profile reference.
    pub proxy_id: Option<ProxyId>,
    /// Ordered SSH jump hosts, with a maximum depth of four.
    #[serde(default)]
    pub jump_host_ids: Vec<ConnectionId>,
    /// Optional terminal profile reference.
    pub terminal_profile_id: Option<TerminalProfileId>,
    /// SSH authentication method without credential bytes.
    #[serde(default)]
    pub authentication: SshAuthenticationKind,
    /// Optional local private-key path; key contents never cross IPC.
    #[serde(default)]
    pub private_key_path: String,
    /// Per-host opt-in for the bounded legacy algorithm set.
    #[serde(default)]
    pub allow_legacy_algorithms: bool,
    /// Whether explicit compatibility fallback to platform OpenSSH is allowed.
    #[serde(default)]
    pub openssh_fallback: bool,
    /// Searchable private tags.
    pub tags: Vec<String>,
    /// Optional private notes.
    pub notes: String,
}

/// Input used to create or update a connection profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    /// Optional containing folder.
    pub folder_id: Option<FolderId>,
    /// Stable ordering within the folder.
    pub sort_order: i64,
    /// Connection transport category.
    pub kind: ConnectionKind,
    /// User-visible name.
    pub name: String,
    /// Host name or address. Empty for local terminals.
    pub host: String,
    /// Remote port. Zero for local terminals.
    pub port: u16,
    /// Remote account name.
    pub username: String,
    /// Optional proxy profile reference.
    pub proxy_id: Option<ProxyId>,
    /// Ordered SSH jump hosts, with a maximum depth of four.
    #[serde(default)]
    pub jump_host_ids: Vec<ConnectionId>,
    /// Optional terminal profile reference.
    pub terminal_profile_id: Option<TerminalProfileId>,
    /// SSH authentication method without credential bytes.
    #[serde(default)]
    pub authentication: SshAuthenticationKind,
    /// Optional local private-key path.
    #[serde(default)]
    pub private_key_path: String,
    /// Per-host opt-in for the bounded legacy algorithm set.
    #[serde(default)]
    pub allow_legacy_algorithms: bool,
    /// Whether explicit compatibility fallback to platform OpenSSH is allowed.
    #[serde(default)]
    pub openssh_fallback: bool,
    /// Searchable private tags.
    pub tags: Vec<String>,
    /// Optional private notes.
    pub notes: String,
}

/// Decrypted proxy profile without stored credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyProfile {
    /// Stable record identifier.
    pub id: ProxyId,
    /// Monotonic optimistic-concurrency revision.
    pub revision: u64,
    /// Proxy transport category.
    pub kind: ProxyKind,
    /// User-visible name.
    pub name: String,
    /// Proxy host. Empty for direct/system profiles.
    pub host: String,
    /// Proxy port. Zero for direct/system profiles.
    pub port: u16,
    /// Optional proxy username; its password remains in the OS keyring.
    pub username: String,
    /// Whether the proxy or local client resolves target hostnames.
    #[serde(default)]
    pub dns_mode: ProxyDnsMode,
}

/// Input used to create or update a proxy profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyInput {
    /// Proxy transport category.
    pub kind: ProxyKind,
    /// User-visible name.
    pub name: String,
    /// Proxy host.
    pub host: String,
    /// Proxy port.
    pub port: u16,
    /// Optional proxy username.
    pub username: String,
    /// Whether the proxy or local client resolves target hostnames.
    #[serde(default)]
    pub dns_mode: ProxyDnsMode,
}

/// Decrypted terminal appearance and behavior profile.
/// Complete xterm color palette for a user-defined terminal theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPalette {
    /// Default text color.
    pub foreground: String,
    /// Terminal canvas color.
    pub background: String,
    /// Cursor color.
    pub cursor: String,
    /// Text color rendered inside a block cursor.
    pub cursor_accent: String,
    /// Selection highlight color.
    pub selection_background: String,
    /// ANSI black.
    pub black: String,
    /// ANSI red.
    pub red: String,
    /// ANSI green.
    pub green: String,
    /// ANSI yellow.
    pub yellow: String,
    /// ANSI blue.
    pub blue: String,
    /// ANSI magenta.
    pub magenta: String,
    /// ANSI cyan.
    pub cyan: String,
    /// ANSI white.
    pub white: String,
    /// ANSI bright black.
    pub bright_black: String,
    /// ANSI bright red.
    pub bright_red: String,
    /// ANSI bright green.
    pub bright_green: String,
    /// ANSI bright yellow.
    pub bright_yellow: String,
    /// ANSI bright blue.
    pub bright_blue: String,
    /// ANSI bright magenta.
    pub bright_magenta: String,
    /// ANSI bright cyan.
    pub bright_cyan: String,
    /// ANSI bright white.
    pub bright_white: String,
}

/// Decrypted terminal appearance and behavior profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalProfile {
    /// Stable record identifier.
    pub id: TerminalProfileId,
    /// Monotonic optimistic-concurrency revision.
    pub revision: u64,
    /// User-visible name.
    pub name: String,
    /// Terminal font family.
    pub font_family: String,
    /// Terminal font size in logical pixels.
    pub font_size: f32,
    /// Theme identifier interpreted by the terminal adapter.
    pub theme: String,
    /// Complete palette when `theme` is `custom`.
    #[serde(default)]
    pub palette: Option<TerminalPalette>,
    /// Maximum in-memory scrollback lines.
    pub scrollback: u32,
    /// Remote byte encoding.
    #[serde(default)]
    pub encoding: TerminalEncoding,
    /// Keyboard compatibility profile.
    #[serde(default)]
    pub keymap: TerminalKeymap,
}

/// Input used to create or update a terminal profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalProfileInput {
    /// User-visible name.
    pub name: String,
    /// Terminal font family.
    pub font_family: String,
    /// Terminal font size in logical pixels.
    pub font_size: f32,
    /// Theme identifier interpreted by the terminal adapter.
    pub theme: String,
    /// Complete palette when `theme` is `custom`.
    #[serde(default)]
    pub palette: Option<TerminalPalette>,
    /// Maximum in-memory scrollback lines.
    pub scrollback: u32,
    /// Remote byte encoding.
    #[serde(default)]
    pub encoding: TerminalEncoding,
    /// Keyboard compatibility profile.
    #[serde(default)]
    pub keymap: TerminalKeymap,
}

/// Decrypted entities sent to the workspace after unlock.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    /// Ordered visible folders.
    pub folders: Vec<Folder>,
    /// Ordered visible connections.
    pub connections: Vec<ConnectionProfile>,
    /// Visible proxy profiles.
    pub proxies: Vec<ProxyProfile>,
    /// Visible terminal profiles.
    pub terminal_profiles: Vec<TerminalProfile>,
}

/// Persisted workspace panel geometry with bounded logical-pixel values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLayout {
    /// Left connection-tree width.
    pub left_width: u16,
    /// Right inspector width.
    pub right_width: u16,
    /// Bottom activity panel height.
    pub bottom_height: u16,
    /// Whether the bottom activity panel is expanded.
    pub bottom_open: bool,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            left_width: 256,
            right_width: 288,
            bottom_height: 260,
            bottom_open: true,
        }
    }
}

/// Search result category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchResultKind {
    /// Folder match.
    Folder,
    /// Connection match.
    Connection,
    /// Proxy match.
    Proxy,
    /// Terminal profile match.
    TerminalProfile,
}

/// One bounded in-memory search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Result category.
    pub kind: SearchResultKind,
    /// Stable entity identifier.
    pub id: String,
    /// Primary display label.
    pub label: String,
    /// Secondary match context.
    pub detail: String,
}

/// Stable error codes safe to cross the desktop IPC boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Caller supplied malformed or out-of-range data.
    InvalidInput,
    /// Requested operation is invalid for the current state.
    InvalidState,
    /// Requested object does not exist.
    NotFound,
    /// Optimistic revision or unique constraint conflicted.
    Conflict,
    /// A bounded queue cannot accept more work.
    ResourceExhausted,
    /// The operation exceeded its deadline.
    Timeout,
    /// A lower-level component failed without safe public details.
    Internal,
}

/// Error payload intended for UI display and IPC serialization.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    fn typed_ids_should_be_unique_for_new_values() {
        assert_ne!(ConnectionId::new(), ConnectionId::new());
        assert_ne!(FolderId::new(), FolderId::new());
    }

    #[test]
    fn public_error_should_assign_a_non_nil_correlation_id() {
        let error = PublicError::new(ErrorCode::InvalidInput, "invalid port");
        assert_ne!(error.correlation_id, Uuid::nil());
    }

    #[test]
    fn m1_payloads_should_decode_with_m2_security_defaults() {
        let connection: ConnectionProfile = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(),
            "folderId": null,
            "revision": 1,
            "sortOrder": 0,
            "kind": "ssh",
            "name": "Legacy",
            "host": "legacy.test",
            "port": 22,
            "username": "operator",
            "proxyId": null,
            "terminalProfileId": null,
            "tags": [],
            "notes": ""
        }))
        .expect("M1 connection payload should remain readable");
        assert_eq!(connection.authentication, SshAuthenticationKind::Agent);
        assert!(!connection.allow_legacy_algorithms);
        assert!(!connection.openssh_fallback);

        let terminal: TerminalProfile = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(),
            "revision": 1,
            "name": "Legacy",
            "fontFamily": "Consolas",
            "fontSize": 14.0,
            "theme": "firstshell-dark",
            "scrollback": 10000
        }))
        .expect("M1 terminal payload should remain readable");
        assert_eq!(terminal.encoding, TerminalEncoding::Utf8);
        assert_eq!(terminal.keymap, TerminalKeymap::Xterm);
        assert!(terminal.palette.is_none());
    }
}
