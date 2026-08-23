//! Application ports that keep infrastructure outside the domain boundary.

mod workspace;

pub use workspace::{
    ExportReceipt, ImportSummary, OsVaultKeyStore, RestoreReceipt, VaultKeyStore, VaultStatus,
    WorkspacePaths, WorkspaceService,
};

use async_trait::async_trait;
use firstshell_domain::{ConnectionId, ConnectionProfile, PublicError};

/// Persistence boundary for connection metadata.
#[async_trait]
pub trait ConnectionRepository: Send + Sync {
    /// Returns all visible connection records.
    async fn list(&self) -> Result<Vec<ConnectionProfile>, PublicError>;

    /// Returns one connection record when present.
    async fn get(&self, id: ConnectionId) -> Result<Option<ConnectionProfile>, PublicError>;
}
