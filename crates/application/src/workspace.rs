//! Secure workspace use cases shared by the desktop adapter and tests.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use firstshell_domain::{
    CommandHistoryEntry, ConnectionId, ConnectionInput, ConnectionKind, ConnectionProfile,
    ErrorCode, Folder, FolderId, FolderInput, ProxyId, ProxyInput, ProxyKind, ProxyProfile,
    PublicError, SearchResult, SearchResultKind, Snippet, SnippetId, SnippetInput,
    SnippetParameter, SnippetParameterKind, SshAuthenticationKind, TerminalEncoding,
    TerminalKeymap, TerminalProfile, TerminalProfileId, TerminalProfileInput, WorkspaceLayout,
    WorkspaceSnapshot,
};
use firstshell_security::{
    ExportEnvelope, OsKeyring, RecordContext, RecoveryKeySlot, SealedPayload, SecurityError,
    VaultKey, VaultSession, open_export, open_record, seal_export, seal_record, unwrap_vault_key,
    wrap_vault_key,
};
use firstshell_storage::{
    DbActor, RecordKind, StorageError, StoredRecord, VaultKeySlot, VaultMetadata,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

const SCHEMA_VERSION: u32 = 1;
const KEY_VERSION: u32 = 1;
const WORKSPACE_ID: &str = "default";
const VAULT_VERIFIER: &[u8] = b"firstshell-vault-verifier-v1";
const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024 + 1024 * 1024;
const MAX_HISTORY_ENTRIES: usize = 500;

/// Files owned by one local workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePaths {
    /// Primary `SQLite` database.
    pub database: PathBuf,
    /// Password-encrypted portable exports.
    pub exports: PathBuf,
    /// Automatic online backups created before imports.
    pub recovery: PathBuf,
}

impl WorkspacePaths {
    /// Creates the standard paths under an application data root.
    #[must_use]
    pub fn under(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            database: root.join("firstshell.db"),
            exports: root.join("exports"),
            recovery: root.join("recovery"),
        }
    }
}

/// Key store port used to test vault behavior without touching real credentials.
pub trait VaultKeyStore: Send + Sync {
    /// Loads one exact vault key.
    ///
    /// # Errors
    /// Returns a key-store error when the requested key is unavailable or invalid.
    fn load(&self, vault_id: &str) -> Result<VaultKey, SecurityError>;
    /// Stores or replaces one vault key.
    ///
    /// # Errors
    /// Returns a key-store error when the platform credential store rejects the write.
    fn store(&self, vault_id: &str, key: &VaultKey) -> Result<(), SecurityError>;
}

/// Production key store backed by the operating system credential manager.
#[derive(Debug, Clone)]
pub struct OsVaultKeyStore {
    service: String,
}

impl OsVaultKeyStore {
    /// Creates the platform adapter with a stable service namespace.
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl VaultKeyStore for OsVaultKeyStore {
    fn load(&self, vault_id: &str) -> Result<VaultKey, SecurityError> {
        OsKeyring::new(&self.service, vault_id).load()
    }

    fn store(&self, vault_id: &str, key: &VaultKey) -> Result<(), SecurityError> {
        OsKeyring::new(&self.service, vault_id).store(key)
    }
}

/// Public vault state that contains no secret material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct VaultStatus {
    /// Whether mandatory initialization has completed.
    pub initialized: bool,
    /// Whether decrypted operations are currently allowed.
    pub unlocked: bool,
    /// Whether this process initialized a new vault.
    pub initialized_this_run: bool,
    /// Whether the current vault key was successfully enrolled in the OS keyring.
    pub keyring_available: bool,
    /// Configured idle timeout in seconds.
    pub idle_timeout_seconds: u64,
}

/// Result of writing a portable encrypted export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReceipt {
    /// Basename inside the controlled exports directory.
    pub file_name: String,
    /// Encoded encrypted container size.
    pub bytes: u64,
}

/// Counts returned after fully decrypting and validating an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    /// Folder count.
    pub folders: usize,
    /// Connection count.
    pub connections: usize,
    /// Proxy count.
    pub proxies: usize,
    /// Terminal profile count.
    pub terminal_profiles: usize,
    /// Encrypted command snippet count.
    pub snippets: usize,
}

/// Result of an atomic import replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReceipt {
    /// Imported object counts.
    pub imported: ImportSummary,
    /// Online backup created before replacement.
    pub recovery_file_name: String,
}

#[derive(Debug, Clone)]
struct IndexedResult {
    result: SearchResult,
    normalized: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortableWorkspace {
    version: u16,
    snapshot: WorkspaceSnapshot,
    layout: WorkspaceLayout,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EfficiencyState {
    command_history: Vec<CommandHistoryEntry>,
}

#[derive(Debug, Clone)]
struct VaultDescriptor {
    vault_id: String,
    key_version: u32,
}

#[derive(Debug, Clone)]
struct VaultRuntimeState {
    descriptor: Option<VaultDescriptor>,
    initialized_this_run: bool,
    keyring_available: bool,
}

/// Application use cases for one encrypted local workspace.
pub struct WorkspaceService {
    db: DbActor,
    paths: WorkspacePaths,
    runtime: RwLock<VaultRuntimeState>,
    key_store: Arc<dyn VaultKeyStore>,
    session: VaultSession,
    idle_timeout: Duration,
    search_index: RwLock<Vec<IndexedResult>>,
}

impl WorkspaceService {
    /// Opens a production workspace using the operating-system keyring.
    ///
    /// # Errors
    /// Returns a redacted public error when directories, the database, keyring,
    /// or initial vault creation fails.
    pub async fn open_default(
        paths: WorkspacePaths,
        idle_timeout: Duration,
    ) -> Result<Self, PublicError> {
        Self::open(
            paths,
            idle_timeout,
            Arc::new(OsVaultKeyStore::new("dev.firstshell.desktop")),
        )
        .await
    }

    /// Opens a workspace with an injected key store.
    ///
    /// # Errors
    /// Returns a redacted public error when storage or key initialization fails.
    pub async fn open(
        paths: WorkspacePaths,
        idle_timeout: Duration,
        key_store: Arc<dyn VaultKeyStore>,
    ) -> Result<Self, PublicError> {
        create_workspace_directories(&paths).await?;
        let db = DbActor::open(&paths.database).await.map_err(map_storage)?;
        let session = VaultSession::new(idle_timeout);
        let descriptor = db
            .vault_metadata()
            .await
            .map_err(map_storage)?
            .map(|metadata| VaultDescriptor {
                vault_id: metadata.vault_id,
                key_version: metadata.key_version,
            });
        let keyring_available = descriptor
            .as_ref()
            .is_some_and(|value| key_store.load(&value.vault_id).is_ok());

        Ok(Self {
            db,
            paths,
            runtime: RwLock::new(VaultRuntimeState {
                descriptor,
                initialized_this_run: false,
                keyring_available,
            }),
            key_store,
            session,
            idle_timeout,
            search_index: RwLock::new(Vec::new()),
        })
    }

    /// Returns public lock state.
    ///
    /// # Errors
    /// Returns an internal error if the in-memory session is unavailable.
    pub fn vault_status(&self) -> Result<VaultStatus, PublicError> {
        let runtime = self
            .runtime
            .read()
            .map_err(|_| internal("vault runtime state is unavailable"))?;
        Ok(VaultStatus {
            initialized: runtime.descriptor.is_some(),
            unlocked: self.session.is_unlocked().map_err(map_security)?,
            initialized_this_run: runtime.initialized_this_run,
            keyring_available: runtime.keyring_available,
            idle_timeout_seconds: self.idle_timeout.as_secs(),
        })
    }

    /// Creates a vault only after a mandatory recovery password can wrap its key.
    ///
    /// # Errors
    /// Returns a validation, randomness, encryption, keyring, or storage error.
    pub async fn initialize(
        &self,
        recovery_password: String,
    ) -> Result<WorkspaceSnapshot, PublicError> {
        if self.vault_descriptor_optional()?.is_some() {
            return Err(PublicError::new(
                ErrorCode::InvalidState,
                "vault is already initialized",
            ));
        }
        let recovery_password = Zeroizing::new(recovery_password.into_bytes());
        validate_recovery_password(recovery_password.as_slice())?;
        let vault_id = Uuid::new_v4().to_string();
        let key = VaultKey::generate().map_err(map_security)?;
        let recovery_slot =
            wrap_vault_key(recovery_password.as_slice(), &vault_id, &key).map_err(map_security)?;
        let verifier = encode_sealed(
            &seal_record(
                &key,
                record_context("vault_meta", &vault_id, 1, KEY_VERSION),
                VAULT_VERIFIER,
            )
            .map_err(map_security)?,
        )?;
        let metadata = VaultMetadata {
            vault_id: vault_id.clone(),
            key_version: KEY_VERSION,
            verifier,
        };
        let slot = VaultKeySlot {
            slot_id: Uuid::new_v4().to_string(),
            vault_id: vault_id.clone(),
            kind: "recovery_password".to_owned(),
            payload: encode_recovery_slot(&recovery_slot)?,
        };
        self.db
            .initialize_vault(metadata, slot)
            .await
            .map_err(map_storage)?;

        let keyring_available = self.key_store.store(&vault_id, &key).is_ok();
        self.session.unlock(key).map_err(map_security)?;
        {
            let mut runtime = self
                .runtime
                .write()
                .map_err(|_| internal("vault runtime state is unavailable"))?;
            runtime.descriptor = Some(VaultDescriptor {
                vault_id,
                key_version: KEY_VERSION,
            });
            runtime.initialized_this_run = true;
            runtime.keyring_available = keyring_available;
        }
        self.seed_defaults().await?;
        let snapshot = self.snapshot().await?;
        self.replace_search_index(&snapshot)?;
        Ok(snapshot)
    }

    /// Loads the vault key from the OS keyring and authenticates its verifier.
    ///
    /// # Errors
    /// Returns a redacted error when the key is unavailable or invalid.
    pub async fn unlock(&self) -> Result<WorkspaceSnapshot, PublicError> {
        let descriptor = self.vault_descriptor()?;
        let metadata = self
            .db
            .vault_metadata()
            .await
            .map_err(map_storage)?
            .ok_or_else(|| internal("vault metadata is unavailable"))?;
        let key = self.key_store.load(&descriptor.vault_id).map_err(|_| {
            PublicError::new(
                ErrorCode::InvalidState,
                "OS keyring key is unavailable; use recovery password",
            )
        })?;
        let sealed = decode_sealed(&metadata.verifier)?;
        let opened = open_record(
            &key,
            record_context(
                "vault_meta",
                &descriptor.vault_id,
                1,
                descriptor.key_version,
            ),
            &sealed,
        )
        .map_err(map_security)?;
        if opened.as_slice() != VAULT_VERIFIER {
            return Err(internal("vault verification failed"));
        }
        self.session.unlock(key).map_err(map_security)?;
        self.set_keyring_available(true)?;
        let snapshot = self.snapshot().await?;
        self.replace_search_index(&snapshot)?;
        Ok(snapshot)
    }

    /// Recovers the vault key from the mandatory password slot and re-enrolls it.
    ///
    /// # Errors
    /// Returns a validation, authentication, storage, or keyring error.
    pub async fn recover(
        &self,
        recovery_password: String,
    ) -> Result<WorkspaceSnapshot, PublicError> {
        let descriptor = self.vault_descriptor()?;
        let recovery_password = Zeroizing::new(recovery_password.into_bytes());
        validate_recovery_password(recovery_password.as_slice())?;
        let stored = self
            .db
            .recovery_slot(descriptor.vault_id.clone())
            .await
            .map_err(map_storage)?
            .ok_or_else(|| internal("mandatory recovery slot is unavailable"))?;
        let slot = decode_recovery_slot(&stored.payload)?;
        let key = unwrap_vault_key(recovery_password.as_slice(), &descriptor.vault_id, &slot)
            .map_err(map_security)?;
        self.verify_vault_key(&descriptor, &key).await?;
        self.key_store
            .store(&descriptor.vault_id, &key)
            .map_err(map_security)?;
        self.session.unlock(key).map_err(map_security)?;
        self.set_keyring_available(true)?;
        let snapshot = self.snapshot().await?;
        self.replace_search_index(&snapshot)?;
        Ok(snapshot)
    }

    /// Removes the in-memory key and search index.
    ///
    /// # Errors
    /// Returns an internal error if the session state is unavailable.
    pub fn lock(&self) -> Result<(), PublicError> {
        self.session.lock().map_err(map_security)?;
        self.search_index
            .write()
            .map_err(|_| internal("search index is unavailable"))?
            .clear();
        Ok(())
    }

    /// Applies automatic locking after the configured idle timeout.
    ///
    /// Returns true when this call removed a resident key.
    ///
    /// # Errors
    /// Returns an internal error if the session state is unavailable.
    pub fn enforce_idle_lock(&self) -> Result<bool, PublicError> {
        let locked = self.session.lock_if_idle().map_err(map_security)?;
        if locked {
            self.search_index
                .write()
                .map_err(|_| internal("search index is unavailable"))?
                .clear();
        }
        Ok(locked)
    }

    /// Returns all decrypted visible entities.
    ///
    /// # Errors
    /// Returns a locked, storage, authentication, or payload error.
    pub async fn snapshot(&self) -> Result<WorkspaceSnapshot, PublicError> {
        ensure_unlocked(&self.session)?;
        let (folders, connections, proxies, terminals, snippets) = tokio::try_join!(
            self.db.list(RecordKind::Folder),
            self.db.list(RecordKind::Connection),
            self.db.list(RecordKind::Proxy),
            self.db.list(RecordKind::TerminalProfile),
            self.db.list(RecordKind::Snippet),
        )
        .map_err(map_storage)?;

        let snapshot = WorkspaceSnapshot {
            folders: folders
                .iter()
                .map(|record| self.open_folder(record))
                .collect::<Result<_, _>>()?,
            connections: connections
                .iter()
                .map(|record| self.open_connection(record))
                .collect::<Result<_, _>>()?,
            proxies: proxies
                .iter()
                .map(|record| self.open_proxy(record))
                .collect::<Result<_, _>>()?,
            terminal_profiles: terminals
                .iter()
                .map(|record| self.open_terminal(record))
                .collect::<Result<_, _>>()?,
            snippets: snippets
                .iter()
                .map(|record| self.open_snippet(record))
                .collect::<Result<_, _>>()?,
        };
        validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    /// Creates a folder.
    ///
    /// # Errors
    /// Returns a validation, locked, reference, encryption, or storage error.
    pub async fn create_folder(&self, input: FolderInput) -> Result<Folder, PublicError> {
        validate_folder_input(&input)?;
        let snapshot = self.snapshot().await?;
        validate_parent(input.parent_id, None, &snapshot.folders)?;
        let folder = Folder {
            id: FolderId::new(),
            parent_id: input.parent_id,
            revision: 1,
            sort_order: input.sort_order,
            name: input.name.trim().to_owned(),
            notes: input.notes,
        };
        self.db
            .save(self.seal_folder(&folder)?, None)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(folder)
    }

    /// Updates a folder with optimistic concurrency.
    ///
    /// # Errors
    /// Returns a validation, conflict, reference, encryption, or storage error.
    pub async fn update_folder(
        &self,
        id: FolderId,
        expected_revision: u64,
        input: FolderInput,
    ) -> Result<Folder, PublicError> {
        validate_folder_input(&input)?;
        let snapshot = self.snapshot().await?;
        let current = snapshot
            .folders
            .iter()
            .find(|folder| folder.id == id)
            .ok_or_else(not_found)?;
        if current.revision != expected_revision {
            return Err(conflict());
        }
        validate_parent(input.parent_id, Some(id), &snapshot.folders)?;
        let folder = Folder {
            id,
            parent_id: input.parent_id,
            revision: expected_revision.saturating_add(1),
            sort_order: input.sort_order,
            name: input.name.trim().to_owned(),
            notes: input.notes,
        };
        self.db
            .save(self.seal_folder(&folder)?, Some(expected_revision))
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(folder)
    }

    /// Deletes an empty folder with optimistic concurrency.
    ///
    /// # Errors
    /// Returns a conflict when the folder has children or connections.
    pub async fn delete_folder(
        &self,
        id: FolderId,
        expected_revision: u64,
    ) -> Result<(), PublicError> {
        let snapshot = self.snapshot().await?;
        if snapshot
            .folders
            .iter()
            .any(|folder| folder.parent_id == Some(id))
            || snapshot
                .connections
                .iter()
                .any(|connection| connection.folder_id == Some(id))
        {
            return Err(PublicError::new(
                ErrorCode::Conflict,
                "folder must be empty before deletion",
            ));
        }
        self.db
            .delete(RecordKind::Folder, id.to_string(), expected_revision)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await
    }

    /// Restores a recently deleted folder with optimistic concurrency.
    ///
    /// # Errors
    /// Returns a locked, conflict, storage, or payload error.
    pub async fn restore_folder(
        &self,
        id: FolderId,
        expected_revision: u64,
    ) -> Result<Folder, PublicError> {
        ensure_unlocked(&self.session)?;
        let mut tombstone = self
            .db
            .deleted(RecordKind::Folder, id.to_string())
            .await
            .map_err(map_storage)?
            .ok_or_else(conflict)?;
        require_revision(Some(tombstone.revision()), expected_revision)?;
        let prior_revision = expected_revision
            .checked_sub(1)
            .ok_or_else(|| invalid("deleted revision is invalid"))?;
        tombstone.set_revision(prior_revision);
        let mut folder = self.open_folder(&tombstone)?;
        let snapshot = self.snapshot().await?;
        validate_parent(folder.parent_id, Some(id), &snapshot.folders)?;
        folder.revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| invalid("record revision exceeds limits"))?;
        self.db
            .restore(self.seal_folder(&folder)?, expected_revision)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(folder)
    }

    /// Creates a connection profile without embedding authentication secrets.
    ///
    /// # Errors
    /// Returns a validation, reference, encryption, or storage error.
    pub async fn create_connection(
        &self,
        input: ConnectionInput,
    ) -> Result<ConnectionProfile, PublicError> {
        validate_connection_input(&input)?;
        let snapshot = self.snapshot().await?;
        validate_connection_references(&input, &snapshot, None)?;
        let connection = ConnectionProfile {
            id: ConnectionId::new(),
            folder_id: input.folder_id,
            revision: 1,
            sort_order: input.sort_order,
            kind: input.kind,
            name: input.name.trim().to_owned(),
            host: input.host.trim().to_owned(),
            port: input.port,
            username: input.username.trim().to_owned(),
            proxy_id: input.proxy_id,
            jump_host_ids: input.jump_host_ids,
            terminal_profile_id: input.terminal_profile_id,
            authentication: input.authentication,
            private_key_path: input.private_key_path.trim().to_owned(),
            allow_legacy_algorithms: input.allow_legacy_algorithms,
            openssh_fallback: input.openssh_fallback,
            tags: normalized_tags(input.tags),
            notes: input.notes,
        };
        self.db
            .save(self.seal_connection(&connection)?, None)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(connection)
    }

    /// Updates a connection profile with optimistic concurrency.
    ///
    /// # Errors
    /// Returns a validation, conflict, reference, encryption, or storage error.
    pub async fn update_connection(
        &self,
        id: ConnectionId,
        expected_revision: u64,
        input: ConnectionInput,
    ) -> Result<ConnectionProfile, PublicError> {
        validate_connection_input(&input)?;
        let snapshot = self.snapshot().await?;
        require_revision(
            snapshot
                .connections
                .iter()
                .find(|connection| connection.id == id)
                .map(|connection| connection.revision),
            expected_revision,
        )?;
        validate_connection_references(&input, &snapshot, Some(id))?;
        let connection = ConnectionProfile {
            id,
            folder_id: input.folder_id,
            revision: expected_revision.saturating_add(1),
            sort_order: input.sort_order,
            kind: input.kind,
            name: input.name.trim().to_owned(),
            host: input.host.trim().to_owned(),
            port: input.port,
            username: input.username.trim().to_owned(),
            proxy_id: input.proxy_id,
            jump_host_ids: input.jump_host_ids,
            terminal_profile_id: input.terminal_profile_id,
            authentication: input.authentication,
            private_key_path: input.private_key_path.trim().to_owned(),
            allow_legacy_algorithms: input.allow_legacy_algorithms,
            openssh_fallback: input.openssh_fallback,
            tags: normalized_tags(input.tags),
            notes: input.notes,
        };
        self.db
            .save(self.seal_connection(&connection)?, Some(expected_revision))
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(connection)
    }

    /// Deletes a connection profile.
    ///
    /// # Errors
    /// Returns a locked, conflict, or storage error.
    pub async fn delete_connection(
        &self,
        id: ConnectionId,
        expected_revision: u64,
    ) -> Result<(), PublicError> {
        ensure_unlocked(&self.session)?;
        self.db
            .delete(RecordKind::Connection, id.to_string(), expected_revision)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await
    }

    /// Restores a recently deleted connection with optimistic concurrency.
    ///
    /// # Errors
    /// Returns a locked, conflict, storage, or payload error.
    pub async fn restore_connection(
        &self,
        id: ConnectionId,
        expected_revision: u64,
    ) -> Result<ConnectionProfile, PublicError> {
        ensure_unlocked(&self.session)?;
        let mut tombstone = self
            .db
            .deleted(RecordKind::Connection, id.to_string())
            .await
            .map_err(map_storage)?
            .ok_or_else(conflict)?;
        require_revision(Some(tombstone.revision()), expected_revision)?;
        let prior_revision = expected_revision
            .checked_sub(1)
            .ok_or_else(|| invalid("deleted revision is invalid"))?;
        tombstone.set_revision(prior_revision);
        let mut connection = self.open_connection(&tombstone)?;
        let snapshot = self.snapshot().await?;
        validate_connection_references(
            &ConnectionInput {
                folder_id: connection.folder_id,
                sort_order: connection.sort_order,
                kind: connection.kind,
                name: connection.name.clone(),
                host: connection.host.clone(),
                port: connection.port,
                username: connection.username.clone(),
                proxy_id: connection.proxy_id,
                jump_host_ids: connection.jump_host_ids.clone(),
                terminal_profile_id: connection.terminal_profile_id,
                authentication: connection.authentication,
                private_key_path: connection.private_key_path.clone(),
                allow_legacy_algorithms: connection.allow_legacy_algorithms,
                openssh_fallback: connection.openssh_fallback,
                tags: connection.tags.clone(),
                notes: connection.notes.clone(),
            },
            &snapshot,
            Some(id),
        )?;
        connection.revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| invalid("record revision exceeds limits"))?;
        self.db
            .restore(self.seal_connection(&connection)?, expected_revision)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(connection)
    }

    /// Creates a proxy profile.
    ///
    /// # Errors
    /// Returns a validation, encryption, or storage error.
    pub async fn create_proxy(&self, input: ProxyInput) -> Result<ProxyProfile, PublicError> {
        validate_proxy_input(&input)?;
        ensure_unlocked(&self.session)?;
        let proxy = ProxyProfile {
            id: ProxyId::new(),
            revision: 1,
            kind: input.kind,
            name: input.name.trim().to_owned(),
            host: input.host.trim().to_owned(),
            port: input.port,
            username: input.username.trim().to_owned(),
            dns_mode: input.dns_mode,
            fallback_port: input.fallback_port,
            relay_server_name: input.relay_server_name.trim().to_owned(),
            relay_mapping: input.relay_mapping.trim().to_owned(),
            relay_certificate_path: input.relay_certificate_path.trim().to_owned(),
        };
        self.db
            .save(self.seal_proxy(&proxy)?, None)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(proxy)
    }

    /// Updates a proxy profile.
    ///
    /// # Errors
    /// Returns a validation, conflict, encryption, or storage error.
    pub async fn update_proxy(
        &self,
        id: ProxyId,
        expected_revision: u64,
        input: ProxyInput,
    ) -> Result<ProxyProfile, PublicError> {
        validate_proxy_input(&input)?;
        let snapshot = self.snapshot().await?;
        require_revision(
            snapshot
                .proxies
                .iter()
                .find(|proxy| proxy.id == id)
                .map(|proxy| proxy.revision),
            expected_revision,
        )?;
        let proxy = ProxyProfile {
            id,
            revision: expected_revision.saturating_add(1),
            kind: input.kind,
            name: input.name.trim().to_owned(),
            host: input.host.trim().to_owned(),
            port: input.port,
            username: input.username.trim().to_owned(),
            dns_mode: input.dns_mode,
            fallback_port: input.fallback_port,
            relay_server_name: input.relay_server_name.trim().to_owned(),
            relay_mapping: input.relay_mapping.trim().to_owned(),
            relay_certificate_path: input.relay_certificate_path.trim().to_owned(),
        };
        self.db
            .save(self.seal_proxy(&proxy)?, Some(expected_revision))
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(proxy)
    }

    /// Deletes an unreferenced proxy profile.
    ///
    /// # Errors
    /// Returns a conflict while any connection references the profile.
    pub async fn delete_proxy(
        &self,
        id: ProxyId,
        expected_revision: u64,
    ) -> Result<(), PublicError> {
        let snapshot = self.snapshot().await?;
        if snapshot
            .connections
            .iter()
            .any(|connection| connection.proxy_id == Some(id))
        {
            return Err(PublicError::new(
                ErrorCode::Conflict,
                "proxy is referenced by a connection",
            ));
        }
        self.db
            .delete(RecordKind::Proxy, id.to_string(), expected_revision)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await
    }

    /// Creates a terminal profile.
    ///
    /// # Errors
    /// Returns a validation, encryption, or storage error.
    pub async fn create_terminal_profile(
        &self,
        input: TerminalProfileInput,
    ) -> Result<TerminalProfile, PublicError> {
        validate_terminal_input(&input)?;
        ensure_unlocked(&self.session)?;
        let profile = TerminalProfile {
            id: TerminalProfileId::new(),
            revision: 1,
            name: input.name.trim().to_owned(),
            font_family: input.font_family.trim().to_owned(),
            font_size: input.font_size,
            theme: input.theme.trim().to_owned(),
            palette: input.palette,
            scrollback: input.scrollback,
            encoding: input.encoding,
            keymap: input.keymap,
        };
        self.db
            .save(self.seal_terminal(&profile)?, None)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(profile)
    }

    /// Updates a terminal profile.
    ///
    /// # Errors
    /// Returns a validation, conflict, encryption, or storage error.
    pub async fn update_terminal_profile(
        &self,
        id: TerminalProfileId,
        expected_revision: u64,
        input: TerminalProfileInput,
    ) -> Result<TerminalProfile, PublicError> {
        validate_terminal_input(&input)?;
        let snapshot = self.snapshot().await?;
        require_revision(
            snapshot
                .terminal_profiles
                .iter()
                .find(|profile| profile.id == id)
                .map(|profile| profile.revision),
            expected_revision,
        )?;
        let profile = TerminalProfile {
            id,
            revision: expected_revision.saturating_add(1),
            name: input.name.trim().to_owned(),
            font_family: input.font_family.trim().to_owned(),
            font_size: input.font_size,
            theme: input.theme.trim().to_owned(),
            palette: input.palette,
            scrollback: input.scrollback,
            encoding: input.encoding,
            keymap: input.keymap,
        };
        self.db
            .save(self.seal_terminal(&profile)?, Some(expected_revision))
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(profile)
    }

    /// Deletes an unreferenced terminal profile.
    ///
    /// # Errors
    /// Returns a conflict while any connection references the profile.
    pub async fn delete_terminal_profile(
        &self,
        id: TerminalProfileId,
        expected_revision: u64,
    ) -> Result<(), PublicError> {
        let snapshot = self.snapshot().await?;
        if snapshot
            .connections
            .iter()
            .any(|connection| connection.terminal_profile_id == Some(id))
        {
            return Err(PublicError::new(
                ErrorCode::Conflict,
                "terminal profile is referenced by a connection",
            ));
        }
        self.db
            .delete(
                RecordKind::TerminalProfile,
                id.to_string(),
                expected_revision,
            )
            .await
            .map_err(map_storage)?;
        self.refresh_search().await
    }

    /// Creates an encrypted typed command snippet.
    ///
    /// # Errors
    /// Returns a validation, encryption, or storage error.
    pub async fn create_snippet(&self, input: SnippetInput) -> Result<Snippet, PublicError> {
        validate_snippet_input(&input)?;
        ensure_unlocked(&self.session)?;
        let snippet = Snippet {
            id: SnippetId::new(),
            revision: 1,
            name: input.name.trim().to_owned(),
            tags: normalized_tags(input.tags),
            template: input.template,
            parameters: input.parameters,
        };
        self.db
            .save(self.seal_snippet(&snippet)?, None)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(snippet)
    }

    /// Updates an encrypted typed command snippet with optimistic concurrency.
    ///
    /// # Errors
    /// Returns a validation, conflict, encryption, or storage error.
    pub async fn update_snippet(
        &self,
        id: SnippetId,
        expected_revision: u64,
        input: SnippetInput,
    ) -> Result<Snippet, PublicError> {
        validate_snippet_input(&input)?;
        let snapshot = self.snapshot().await?;
        require_revision(
            snapshot
                .snippets
                .iter()
                .find(|snippet| snippet.id == id)
                .map(|snippet| snippet.revision),
            expected_revision,
        )?;
        let snippet = Snippet {
            id,
            revision: expected_revision.saturating_add(1),
            name: input.name.trim().to_owned(),
            tags: normalized_tags(input.tags),
            template: input.template,
            parameters: input.parameters,
        };
        self.db
            .save(self.seal_snippet(&snippet)?, Some(expected_revision))
            .await
            .map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(snippet)
    }

    /// Deletes an encrypted command snippet.
    ///
    /// # Errors
    /// Returns a locked, conflict, or storage error.
    pub async fn delete_snippet(
        &self,
        id: SnippetId,
        expected_revision: u64,
    ) -> Result<(), PublicError> {
        ensure_unlocked(&self.session)?;
        self.db
            .delete(RecordKind::Snippet, id.to_string(), expected_revision)
            .await
            .map_err(map_storage)?;
        self.refresh_search().await
    }

    /// Renders one snippet after validating every declared typed parameter in Rust.
    ///
    /// The returned text never receives a trailing carriage return or newline.
    ///
    /// # Errors
    /// Returns a locked, not-found, missing-parameter, or type-validation error.
    pub async fn render_snippet(
        &self,
        id: SnippetId,
        values: BTreeMap<String, String>,
    ) -> Result<String, PublicError> {
        let snapshot = self.snapshot().await?;
        let snippet = snapshot
            .snippets
            .iter()
            .find(|snippet| snippet.id == id)
            .ok_or_else(not_found)?;
        if values.len() != snippet.parameters.len() {
            return Err(invalid(
                "snippet parameter set does not match its declaration",
            ));
        }
        let mut rendered = snippet.template.clone();
        for parameter in &snippet.parameters {
            let supplied = values
                .get(&parameter.name)
                .map_or(parameter.default_value.as_str(), String::as_str);
            validate_snippet_value(parameter, supplied)?;
            rendered = rendered.replace(&format!("{{{{{}}}}}", parameter.name), supplied);
        }
        if rendered.contains("{{") || rendered.contains("}}") {
            return Err(invalid("snippet contains an undeclared placeholder"));
        }
        Ok(rendered)
    }

    /// Records one encrypted command and inferred path history item.
    ///
    /// # Errors
    /// Returns a validation, locked, reference, encryption, or storage error.
    pub async fn record_command(
        &self,
        connection_id: ConnectionId,
        command: String,
    ) -> Result<CommandHistoryEntry, PublicError> {
        validate_history_command(&command)?;
        if !self
            .snapshot()
            .await?
            .connections
            .iter()
            .any(|connection| connection.id == connection_id)
        {
            return Err(not_found());
        }
        let (mut state, expected) = self.load_efficiency_state().await?;
        let command = command.trim().to_owned();
        let entry = CommandHistoryEntry {
            connection_id,
            path: infer_path(&command),
            command,
            recorded_at_ms: unix_millis(),
        };
        if !state.command_history.last().is_some_and(|last| {
            last.connection_id == entry.connection_id && last.command == entry.command
        }) {
            state.command_history.push(entry.clone());
        }
        if state.command_history.len() > MAX_HISTORY_ENTRIES {
            let remove = state.command_history.len() - MAX_HISTORY_ENTRIES;
            state.command_history.drain(..remove);
        }
        self.save_efficiency_state(&state, expected).await?;
        Ok(entry)
    }

    /// Lists newest encrypted command history for one connection or all connections.
    ///
    /// # Errors
    /// Returns a locked, validation, storage, authentication, or payload error.
    pub async fn command_history(
        &self,
        connection_id: Option<ConnectionId>,
        limit: usize,
    ) -> Result<Vec<CommandHistoryEntry>, PublicError> {
        if !(1..=MAX_HISTORY_ENTRIES).contains(&limit) {
            return Err(invalid("history limit is invalid"));
        }
        let (state, _) = self.load_efficiency_state().await?;
        Ok(state
            .command_history
            .into_iter()
            .rev()
            .filter(|entry| connection_id.is_none_or(|id| entry.connection_id == id))
            .take(limit)
            .collect())
    }

    /// Clears encrypted command and path history.
    ///
    /// # Errors
    /// Returns a locked, storage, encryption, or payload error.
    pub async fn clear_command_history(&self) -> Result<(), PublicError> {
        let (_, expected) = self.load_efficiency_state().await?;
        self.save_efficiency_state(&EfficiencyState::default(), expected)
            .await
    }

    /// Performs a bounded search over decrypted in-memory fields.
    ///
    /// # Errors
    /// Returns a locked or invalid-input error.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, PublicError> {
        ensure_unlocked(&self.session)?;
        let query = query.trim().to_lowercase();
        if query.len() > 128 || !(1..=100).contains(&limit) {
            return Err(invalid("invalid search query or result limit"));
        }
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let index = self
            .search_index
            .read()
            .map_err(|_| internal("search index is unavailable"))?;
        Ok(index
            .iter()
            .filter(|entry| entry.normalized.contains(&query))
            .take(limit)
            .map(|entry| entry.result.clone())
            .collect())
    }

    /// Loads decrypted workspace layout or a bounded default.
    ///
    /// # Errors
    /// Returns a locked, storage, authentication, or payload error.
    pub async fn load_layout(&self) -> Result<WorkspaceLayout, PublicError> {
        ensure_unlocked(&self.session)?;
        let record = self
            .db
            .list(RecordKind::UiState)
            .await
            .map_err(map_storage)?;
        record.first().map_or_else(
            || Ok(WorkspaceLayout::default()),
            |record| self.open_layout(record),
        )
    }

    /// Saves bounded workspace panel geometry.
    ///
    /// # Errors
    /// Returns a validation, locked, conflict, encryption, or storage error.
    pub async fn save_layout(&self, layout: WorkspaceLayout) -> Result<(), PublicError> {
        validate_layout(layout)?;
        ensure_unlocked(&self.session)?;
        let existing = self
            .db
            .list(RecordKind::UiState)
            .await
            .map_err(map_storage)?;
        let expected = existing.first().map(StoredRecord::revision);
        let revision = expected.map_or(1, |value| value.saturating_add(1));
        self.db
            .save(self.seal_layout(layout, revision)?, expected)
            .await
            .map_err(map_storage)
    }

    /// Writes a password-encrypted portable export to the controlled directory.
    ///
    /// # Errors
    /// Returns a validation, locked, serialization, encryption, or file error.
    pub async fn export_workspace(&self, password: String) -> Result<ExportReceipt, PublicError> {
        let password = Zeroizing::new(password.into_bytes());
        if password.len() < 12 || password.len() > 1024 {
            return Err(invalid("export password must contain 12 to 1024 bytes"));
        }
        let portable = PortableWorkspace {
            version: 1,
            snapshot: self.snapshot().await?,
            layout: self.load_layout().await?,
        };
        let plaintext = Zeroizing::new(
            rmp_serde::to_vec_named(&portable)
                .map_err(|_| internal("failed to encode workspace export"))?,
        );
        let envelope =
            seal_export(password.as_slice(), plaintext.as_slice()).map_err(map_security)?;
        let encoded = rmp_serde::to_vec_named(&envelope)
            .map_err(|_| internal("failed to encode encrypted export"))?;
        let file_name = format!("firstshell-{}.firstshell-backup", Uuid::new_v4());
        let path = self.paths.exports.join(&file_name);
        tokio::fs::write(path, &encoded)
            .await
            .map_err(|_| internal("failed to write encrypted export"))?;
        Ok(ExportReceipt {
            file_name,
            bytes: u64::try_from(encoded.len()).map_err(|_| internal("export size overflow"))?,
        })
    }

    /// Fully decrypts and validates a portable export without modifying storage.
    ///
    /// # Errors
    /// Returns a validation, authentication, container, or file error.
    pub async fn validate_import(
        &self,
        file_name: &str,
        password: String,
    ) -> Result<ImportSummary, PublicError> {
        let portable = self.read_import(file_name, password).await?;
        validate_snapshot(&portable.snapshot)?;
        validate_layout(portable.layout)?;
        Ok(import_summary(&portable.snapshot))
    }

    /// Creates an online backup, then atomically replaces all user records.
    ///
    /// # Errors
    /// Returns without partial replacement if validation, backup, encryption, or
    /// the database transaction fails.
    pub async fn restore_import(
        &self,
        file_name: &str,
        password: String,
    ) -> Result<RestoreReceipt, PublicError> {
        ensure_unlocked(&self.session)?;
        let portable = self.read_import(file_name, password).await?;
        validate_snapshot(&portable.snapshot)?;
        validate_layout(portable.layout)?;
        let records = self.seal_portable(&portable)?;
        let recovery_file_name = format!("before-import-{}.db", Uuid::new_v4());
        self.db
            .backup(self.paths.recovery.join(&recovery_file_name))
            .await
            .map_err(map_storage)?;
        self.db.replace_all(records).await.map_err(map_storage)?;
        self.refresh_search().await?;
        Ok(RestoreReceipt {
            imported: import_summary(&portable.snapshot),
            recovery_file_name,
        })
    }

    async fn read_import(
        &self,
        file_name: &str,
        password: String,
    ) -> Result<PortableWorkspace, PublicError> {
        validate_import_name(file_name)?;
        let password = Zeroizing::new(password.into_bytes());
        if password.is_empty() || password.len() > 1024 {
            return Err(invalid("invalid import password"));
        }
        let path = self.paths.exports.join(file_name);
        let metadata = tokio::fs::metadata(&path).await.map_err(|_| not_found())?;
        if metadata.len() > MAX_IMPORT_BYTES {
            return Err(invalid("import container exceeds size limit"));
        }
        let encoded = tokio::fs::read(path)
            .await
            .map_err(|_| internal("failed to read import container"))?;
        let envelope: ExportEnvelope =
            rmp_serde::from_slice(&encoded).map_err(|_| invalid("invalid import container"))?;
        let plaintext = open_export(password.as_slice(), &envelope).map_err(map_security)?;
        let portable: PortableWorkspace = rmp_serde::from_slice(plaintext.as_slice())
            .map_err(|_| invalid("invalid workspace payload"))?;
        if portable.version != 1 {
            return Err(invalid("unsupported workspace export version"));
        }
        Ok(portable)
    }

    fn seal_portable(
        &self,
        portable: &PortableWorkspace,
    ) -> Result<Vec<StoredRecord>, PublicError> {
        let mut records = Vec::with_capacity(
            portable.snapshot.folders.len()
                + portable.snapshot.connections.len()
                + portable.snapshot.proxies.len()
                + portable.snapshot.terminal_profiles.len()
                + portable.snapshot.snippets.len()
                + 1,
        );
        for source in &portable.snapshot.folders {
            let mut value = source.clone();
            value.revision = 1;
            records.push(self.seal_folder(&value)?);
        }
        for source in &portable.snapshot.connections {
            let mut value = source.clone();
            value.revision = 1;
            records.push(self.seal_connection(&value)?);
        }
        for source in &portable.snapshot.proxies {
            let mut value = source.clone();
            value.revision = 1;
            records.push(self.seal_proxy(&value)?);
        }
        for source in &portable.snapshot.terminal_profiles {
            let mut value = source.clone();
            value.revision = 1;
            records.push(self.seal_terminal(&value)?);
        }
        for source in &portable.snapshot.snippets {
            let mut value = source.clone();
            value.revision = 1;
            records.push(self.seal_snippet(&value)?);
        }
        records.push(self.seal_layout(portable.layout, 1)?);
        Ok(records)
    }

    async fn seed_defaults(&self) -> Result<(), PublicError> {
        self.create_terminal_profile(TerminalProfileInput {
            name: "Default".to_owned(),
            font_family: "Cascadia Mono".to_owned(),
            font_size: 14.0,
            theme: "firstshell-dark".to_owned(),
            palette: None,
            scrollback: 10_000,
            encoding: TerminalEncoding::Utf8,
            keymap: TerminalKeymap::Xterm,
        })
        .await?;
        Ok(())
    }

    async fn verify_vault_key(
        &self,
        descriptor: &VaultDescriptor,
        key: &VaultKey,
    ) -> Result<(), PublicError> {
        let metadata = self
            .db
            .vault_metadata()
            .await
            .map_err(map_storage)?
            .ok_or_else(|| internal("vault metadata is unavailable"))?;
        let sealed = decode_sealed(&metadata.verifier)?;
        let opened = open_record(
            key,
            record_context(
                "vault_meta",
                &descriptor.vault_id,
                1,
                descriptor.key_version,
            ),
            &sealed,
        )
        .map_err(map_security)?;
        if opened.as_slice() == VAULT_VERIFIER {
            Ok(())
        } else {
            Err(internal("vault verification failed"))
        }
    }

    fn vault_descriptor_optional(&self) -> Result<Option<VaultDescriptor>, PublicError> {
        self.runtime
            .read()
            .map(|runtime| runtime.descriptor.clone())
            .map_err(|_| internal("vault runtime state is unavailable"))
    }

    fn vault_descriptor(&self) -> Result<VaultDescriptor, PublicError> {
        self.vault_descriptor_optional()?.ok_or_else(|| {
            PublicError::new(ErrorCode::InvalidState, "vault must be initialized first")
        })
    }

    fn set_keyring_available(&self, available: bool) -> Result<(), PublicError> {
        self.runtime
            .write()
            .map_err(|_| internal("vault runtime state is unavailable"))?
            .keyring_available = available;
        Ok(())
    }

    async fn refresh_search(&self) -> Result<(), PublicError> {
        let snapshot = self.snapshot().await?;
        self.replace_search_index(&snapshot)
    }

    fn replace_search_index(&self, snapshot: &WorkspaceSnapshot) -> Result<(), PublicError> {
        let mut index = Vec::new();
        index.extend(snapshot.folders.iter().map(|folder| IndexedResult {
            result: SearchResult {
                kind: SearchResultKind::Folder,
                id: folder.id.to_string(),
                label: folder.name.clone(),
                detail: folder.notes.clone(),
            },
            normalized: format!("{} {}", folder.name, folder.notes).to_lowercase(),
        }));
        index.extend(snapshot.connections.iter().map(|connection| {
            IndexedResult {
                result: SearchResult {
                    kind: SearchResultKind::Connection,
                    id: connection.id.to_string(),
                    label: connection.name.clone(),
                    detail: connection.host.clone(),
                },
                normalized: format!(
                    "{} {} {} {} {}",
                    connection.name,
                    connection.host,
                    connection.username,
                    connection.tags.join(" "),
                    connection.notes
                )
                .to_lowercase(),
            }
        }));
        index.extend(snapshot.proxies.iter().map(|proxy| IndexedResult {
            result: SearchResult {
                kind: SearchResultKind::Proxy,
                id: proxy.id.to_string(),
                label: proxy.name.clone(),
                detail: proxy.host.clone(),
            },
            normalized: format!("{} {} {}", proxy.name, proxy.host, proxy.username).to_lowercase(),
        }));
        index.extend(
            snapshot
                .terminal_profiles
                .iter()
                .map(|profile| IndexedResult {
                    result: SearchResult {
                        kind: SearchResultKind::TerminalProfile,
                        id: profile.id.to_string(),
                        label: profile.name.clone(),
                        detail: profile.font_family.clone(),
                    },
                    normalized: format!(
                        "{} {} {}",
                        profile.name, profile.font_family, profile.theme
                    )
                    .to_lowercase(),
                }),
        );
        index.extend(snapshot.snippets.iter().map(|snippet| {
            IndexedResult {
                result: SearchResult {
                    kind: SearchResultKind::Snippet,
                    id: snippet.id.to_string(),
                    label: snippet.name.clone(),
                    detail: snippet.tags.join(" · "),
                },
                normalized: format!(
                    "{} {} {}",
                    snippet.name,
                    snippet.tags.join(" "),
                    snippet.template
                )
                .to_lowercase(),
            }
        }));
        *self
            .search_index
            .write()
            .map_err(|_| internal("search index is unavailable"))? = index;
        Ok(())
    }

    fn seal_folder(&self, folder: &Folder) -> Result<StoredRecord, PublicError> {
        Ok(StoredRecord::Folder {
            id: folder.id.to_string(),
            parent_id: folder.parent_id.map(|id| id.to_string()),
            sort_order: folder.sort_order,
            revision: folder.revision,
            payload: self.seal_value("folders", &folder.id.to_string(), folder.revision, folder)?,
        })
    }

    fn seal_connection(&self, connection: &ConnectionProfile) -> Result<StoredRecord, PublicError> {
        Ok(StoredRecord::Connection {
            id: connection.id.to_string(),
            folder_id: connection.folder_id.map(|id| id.to_string()),
            kind: connection_kind_name(connection.kind).to_owned(),
            sort_order: connection.sort_order,
            revision: connection.revision,
            payload: self.seal_value(
                "connection_profiles",
                &connection.id.to_string(),
                connection.revision,
                connection,
            )?,
        })
    }

    fn seal_proxy(&self, proxy: &ProxyProfile) -> Result<StoredRecord, PublicError> {
        Ok(StoredRecord::Proxy {
            id: proxy.id.to_string(),
            kind: proxy_kind_name(proxy.kind).to_owned(),
            revision: proxy.revision,
            payload: self.seal_value(
                "proxy_profiles",
                &proxy.id.to_string(),
                proxy.revision,
                proxy,
            )?,
        })
    }

    fn seal_terminal(&self, profile: &TerminalProfile) -> Result<StoredRecord, PublicError> {
        Ok(StoredRecord::TerminalProfile {
            id: profile.id.to_string(),
            revision: profile.revision,
            payload: self.seal_value(
                "terminal_profiles",
                &profile.id.to_string(),
                profile.revision,
                profile,
            )?,
        })
    }

    fn seal_snippet(&self, snippet: &Snippet) -> Result<StoredRecord, PublicError> {
        Ok(StoredRecord::Snippet {
            id: snippet.id.to_string(),
            revision: snippet.revision,
            payload: self.seal_value(
                "snippets",
                &snippet.id.to_string(),
                snippet.revision,
                snippet,
            )?,
        })
    }

    fn seal_layout(
        &self,
        layout: WorkspaceLayout,
        revision: u64,
    ) -> Result<StoredRecord, PublicError> {
        Ok(StoredRecord::UiState {
            workspace_id: WORKSPACE_ID.to_owned(),
            revision,
            payload: self.seal_value("ui_state", WORKSPACE_ID, revision, &layout)?,
        })
    }

    fn seal_efficiency_state(
        &self,
        state: &EfficiencyState,
        revision: u64,
    ) -> Result<StoredRecord, PublicError> {
        Ok(StoredRecord::EfficiencyState {
            workspace_id: WORKSPACE_ID.to_owned(),
            revision,
            payload: self.seal_value("efficiency_state", WORKSPACE_ID, revision, state)?,
        })
    }

    fn seal_value<T: Serialize>(
        &self,
        table: &str,
        id: &str,
        revision: u64,
        value: &T,
    ) -> Result<Vec<u8>, PublicError> {
        let key_version = self.vault_descriptor()?.key_version;
        let plaintext = Zeroizing::new(
            rmp_serde::to_vec_named(value)
                .map_err(|_| internal("failed to encode encrypted record"))?,
        );
        let sealed = self
            .session
            .with_key(|key| {
                seal_record(
                    key,
                    record_context(table, id, revision, key_version),
                    plaintext.as_slice(),
                )
            })
            .map_err(map_security)?;
        encode_sealed(&sealed)
    }

    fn open_value<T: DeserializeOwned>(
        &self,
        table: &str,
        id: &str,
        revision: u64,
        payload: &[u8],
    ) -> Result<T, PublicError> {
        let key_version = self.vault_descriptor()?.key_version;
        let sealed = decode_sealed(payload)?;
        let plaintext = self
            .session
            .with_key(|key| {
                open_record(
                    key,
                    record_context(table, id, revision, key_version),
                    &sealed,
                )
            })
            .map_err(map_security)?;
        rmp_serde::from_slice(plaintext.as_slice())
            .map_err(|_| internal("encrypted record schema is invalid"))
    }

    fn open_folder(&self, record: &StoredRecord) -> Result<Folder, PublicError> {
        let StoredRecord::Folder {
            id,
            parent_id,
            sort_order,
            revision,
            payload,
        } = record
        else {
            return Err(internal("storage returned the wrong record kind"));
        };
        let folder: Folder = self.open_value("folders", id, *revision, payload)?;
        if folder.id.to_string() != *id
            || folder.parent_id.map(|value| value.to_string()) != *parent_id
            || folder.sort_order != *sort_order
            || folder.revision != *revision
        {
            return Err(internal("folder metadata authentication failed"));
        }
        Ok(folder)
    }

    fn open_connection(&self, record: &StoredRecord) -> Result<ConnectionProfile, PublicError> {
        let StoredRecord::Connection {
            id,
            folder_id,
            kind,
            sort_order,
            revision,
            payload,
        } = record
        else {
            return Err(internal("storage returned the wrong record kind"));
        };
        let connection: ConnectionProfile =
            self.open_value("connection_profiles", id, *revision, payload)?;
        if connection.id.to_string() != *id
            || connection.folder_id.map(|value| value.to_string()) != *folder_id
            || connection_kind_name(connection.kind) != kind
            || connection.sort_order != *sort_order
            || connection.revision != *revision
        {
            return Err(internal("connection metadata authentication failed"));
        }
        Ok(connection)
    }

    fn open_proxy(&self, record: &StoredRecord) -> Result<ProxyProfile, PublicError> {
        let StoredRecord::Proxy {
            id,
            kind,
            revision,
            payload,
        } = record
        else {
            return Err(internal("storage returned the wrong record kind"));
        };
        let proxy: ProxyProfile = self.open_value("proxy_profiles", id, *revision, payload)?;
        if proxy.id.to_string() != *id
            || proxy_kind_name(proxy.kind) != kind
            || proxy.revision != *revision
        {
            return Err(internal("proxy metadata authentication failed"));
        }
        Ok(proxy)
    }

    fn open_terminal(&self, record: &StoredRecord) -> Result<TerminalProfile, PublicError> {
        let StoredRecord::TerminalProfile {
            id,
            revision,
            payload,
        } = record
        else {
            return Err(internal("storage returned the wrong record kind"));
        };
        let profile: TerminalProfile =
            self.open_value("terminal_profiles", id, *revision, payload)?;
        if profile.id.to_string() != *id || profile.revision != *revision {
            return Err(internal("terminal metadata authentication failed"));
        }
        Ok(profile)
    }

    fn open_snippet(&self, record: &StoredRecord) -> Result<Snippet, PublicError> {
        let StoredRecord::Snippet {
            id,
            revision,
            payload,
        } = record
        else {
            return Err(internal("storage returned the wrong record kind"));
        };
        let snippet: Snippet = self.open_value("snippets", id, *revision, payload)?;
        if snippet.id.to_string() != *id || snippet.revision != *revision {
            return Err(internal("snippet metadata authentication failed"));
        }
        Ok(snippet)
    }

    fn open_layout(&self, record: &StoredRecord) -> Result<WorkspaceLayout, PublicError> {
        let StoredRecord::UiState {
            workspace_id,
            revision,
            payload,
        } = record
        else {
            return Err(internal("storage returned the wrong record kind"));
        };
        if workspace_id != WORKSPACE_ID {
            return Err(internal("unexpected workspace identifier"));
        }
        let layout = self.open_value("ui_state", workspace_id, *revision, payload)?;
        validate_layout(layout)?;
        Ok(layout)
    }

    async fn load_efficiency_state(&self) -> Result<(EfficiencyState, Option<u64>), PublicError> {
        ensure_unlocked(&self.session)?;
        let records = self
            .db
            .list(RecordKind::EfficiencyState)
            .await
            .map_err(map_storage)?;
        let Some(record) = records.first() else {
            return Ok((EfficiencyState::default(), None));
        };
        let StoredRecord::EfficiencyState {
            workspace_id,
            revision,
            payload,
        } = record
        else {
            return Err(internal("storage returned the wrong record kind"));
        };
        if workspace_id != WORKSPACE_ID {
            return Err(internal("unexpected efficiency workspace identifier"));
        }
        let state = self.open_value("efficiency_state", workspace_id, *revision, payload)?;
        Ok((state, Some(*revision)))
    }

    async fn save_efficiency_state(
        &self,
        state: &EfficiencyState,
        expected: Option<u64>,
    ) -> Result<(), PublicError> {
        let revision = expected.map_or(1, |value| value.saturating_add(1));
        self.db
            .save(self.seal_efficiency_state(state, revision)?, expected)
            .await
            .map_err(map_storage)
    }
}

async fn create_workspace_directories(paths: &WorkspacePaths) -> Result<(), PublicError> {
    let database_parent = paths
        .database
        .parent()
        .ok_or_else(|| internal("invalid database path"))?;
    for directory in [
        database_parent,
        paths.exports.as_path(),
        paths.recovery.as_path(),
    ] {
        tokio::fs::create_dir_all(directory)
            .await
            .map_err(|_| internal("failed to create workspace directory"))?;
    }
    Ok(())
}

fn record_context<'a>(
    table: &'a str,
    id: &'a str,
    revision: u64,
    key_version: u32,
) -> RecordContext<'a> {
    RecordContext {
        table,
        record_id: id,
        schema_version: SCHEMA_VERSION,
        revision,
        key_version,
    }
}

fn encode_sealed(sealed: &SealedPayload) -> Result<Vec<u8>, PublicError> {
    rmp_serde::to_vec_named(sealed).map_err(|_| internal("failed to encode sealed payload"))
}

fn decode_sealed(payload: &[u8]) -> Result<SealedPayload, PublicError> {
    rmp_serde::from_slice(payload).map_err(|_| internal("invalid sealed payload"))
}

fn encode_recovery_slot(slot: &RecoveryKeySlot) -> Result<Vec<u8>, PublicError> {
    rmp_serde::to_vec_named(slot).map_err(|_| internal("failed to encode recovery key slot"))
}

fn decode_recovery_slot(payload: &[u8]) -> Result<RecoveryKeySlot, PublicError> {
    rmp_serde::from_slice(payload).map_err(|_| internal("invalid recovery key slot"))
}

fn validate_recovery_password(password: &[u8]) -> Result<(), PublicError> {
    if !(12..=1024).contains(&password.len()) {
        return Err(invalid("recovery password must contain 12 to 1024 bytes"));
    }
    Ok(())
}

fn validate_folder_input(input: &FolderInput) -> Result<(), PublicError> {
    validate_text("folder name", &input.name, 1, 128)?;
    validate_text("folder notes", &input.notes, 0, 4096)
}

fn validate_connection_input(input: &ConnectionInput) -> Result<(), PublicError> {
    validate_text("connection name", &input.name, 1, 128)?;
    validate_text("host", &input.host, 0, 253)?;
    validate_text("username", &input.username, 0, 128)?;
    validate_text("private key path", &input.private_key_path, 0, 4096)?;
    validate_text("notes", &input.notes, 0, 4096)?;
    if matches!(input.kind, ConnectionKind::Ssh | ConnectionKind::Rdp)
        && (input.host.starts_with('-')
            || input
                .host
                .chars()
                .any(|character| character.is_whitespace() || character.is_control()))
    {
        return Err(invalid("remote host contains unsupported characters"));
    }
    if input.kind == ConnectionKind::Ssh
        && input
            .username
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(invalid("SSH username contains unsupported characters"));
    }
    if input.tags.len() > 32
        || input
            .tags
            .iter()
            .any(|tag| tag.trim().is_empty() || tag.len() > 64)
    {
        return Err(invalid("connection tags exceed limits"));
    }
    if input.kind == ConnectionKind::Ssh
        && input.authentication == SshAuthenticationKind::PrivateKey
        && input.private_key_path.trim().is_empty()
    {
        return Err(invalid("private-key authentication requires a key path"));
    }
    if input.openssh_fallback && input.authentication != SshAuthenticationKind::Agent {
        return Err(invalid(
            "OpenSSH fallback requires agent authentication to keep secrets out of the terminal",
        ));
    }
    if input.openssh_fallback && (input.proxy_id.is_some() || !input.jump_host_ids.is_empty()) {
        return Err(invalid(
            "OpenSSH fallback cannot bypass a configured proxy or jump chain",
        ));
    }
    if input.jump_host_ids.len() > 4 {
        return Err(invalid("SSH jump chain exceeds the maximum depth of four"));
    }
    if input.kind != ConnectionKind::Ssh && !input.jump_host_ids.is_empty() {
        return Err(invalid("only SSH connections can use jump hosts"));
    }
    match input.kind {
        ConnectionKind::Ssh | ConnectionKind::Rdp
            if input.host.trim().is_empty() || input.port == 0 =>
        {
            Err(invalid("remote connections require a host and port"))
        }
        ConnectionKind::Local if !input.host.trim().is_empty() || input.port != 0 => Err(invalid(
            "local connections cannot contain a remote endpoint",
        )),
        _ => Ok(()),
    }
}

fn validate_proxy_input(input: &ProxyInput) -> Result<(), PublicError> {
    validate_text("proxy name", &input.name, 1, 128)?;
    validate_text("proxy host", &input.host, 0, 253)?;
    validate_text("proxy username", &input.username, 0, 128)?;
    validate_text("relay server name", &input.relay_server_name, 0, 253)?;
    validate_text("relay mapping", &input.relay_mapping, 0, 128)?;
    validate_text(
        "relay certificate path",
        &input.relay_certificate_path,
        0,
        4096,
    )?;
    match input.kind {
        ProxyKind::Socks5 | ProxyKind::HttpConnect
            if input.host.trim().is_empty() || input.port == 0 =>
        {
            Err(invalid("network proxies require a host and port"))
        }
        ProxyKind::Relay
            if input.host.trim().is_empty()
                || input.port == 0
                || input.fallback_port == 0
                || input.relay_server_name.trim().is_empty()
                || input.relay_mapping.trim().is_empty()
                || input.relay_certificate_path.trim().is_empty()
                || !input.username.trim().is_empty() =>
        {
            Err(invalid(
                "relay proxies require endpoints, TLS identity, certificate, and mapping without a username",
            ))
        }
        ProxyKind::Direct | ProxyKind::System
            if !input.host.trim().is_empty()
                || input.port != 0
                || input.fallback_port != 0
                || !input.relay_server_name.trim().is_empty()
                || !input.relay_mapping.trim().is_empty()
                || !input.relay_certificate_path.trim().is_empty() =>
        {
            Err(invalid(
                "direct and system proxies cannot contain an endpoint",
            ))
        }
        ProxyKind::Socks5 | ProxyKind::HttpConnect
            if input.fallback_port != 0
                || !input.relay_server_name.trim().is_empty()
                || !input.relay_mapping.trim().is_empty()
                || !input.relay_certificate_path.trim().is_empty() =>
        {
            Err(invalid("network proxies cannot contain relay settings"))
        }
        _ => Ok(()),
    }
}

fn validate_terminal_input(input: &TerminalProfileInput) -> Result<(), PublicError> {
    validate_text("terminal profile name", &input.name, 1, 128)?;
    validate_text("font family", &input.font_family, 1, 128)?;
    validate_text("terminal theme", &input.theme, 1, 128)?;
    if !(8.0..=32.0).contains(&input.font_size) || !(100..=200_000).contains(&input.scrollback) {
        return Err(invalid("terminal font size or scrollback exceeds limits"));
    }
    match (&*input.theme, &input.palette) {
        ("custom", Some(palette)) => {
            let colors = [
                &palette.foreground,
                &palette.background,
                &palette.cursor,
                &palette.cursor_accent,
                &palette.selection_background,
                &palette.black,
                &palette.red,
                &palette.green,
                &palette.yellow,
                &palette.blue,
                &palette.magenta,
                &palette.cyan,
                &palette.white,
                &palette.bright_black,
                &palette.bright_red,
                &palette.bright_green,
                &palette.bright_yellow,
                &palette.bright_blue,
                &palette.bright_magenta,
                &palette.bright_cyan,
                &palette.bright_white,
            ];
            if colors.iter().any(|color| !valid_terminal_color(color)) {
                return Err(invalid("custom terminal palette contains an invalid color"));
            }
        }
        ("custom", None) => return Err(invalid("custom terminal theme requires a palette")),
        (_, Some(_)) => return Err(invalid("terminal palette is only valid for a custom theme")),
        (_, None) => {}
    }
    Ok(())
}

fn validate_snippet_input(input: &SnippetInput) -> Result<(), PublicError> {
    validate_text("snippet name", &input.name, 1, 128)?;
    validate_text("snippet template", &input.template, 1, 16 * 1024)?;
    if input.template.contains(['\r', '\n']) {
        return Err(invalid(
            "terminal snippets must be single-line to prevent implicit execution",
        ));
    }
    if input.tags.len() > 32
        || input
            .tags
            .iter()
            .any(|tag| tag.trim().is_empty() || tag.len() > 64)
        || input.parameters.len() > 32
    {
        return Err(invalid("snippet tags or parameters exceed limits"));
    }
    let mut names = HashSet::new();
    for parameter in &input.parameters {
        validate_snippet_parameter(parameter)?;
        if !names.insert(parameter.name.as_str()) {
            return Err(invalid("snippet parameter names must be unique"));
        }
    }
    let placeholders = snippet_placeholders(&input.template)?;
    if placeholders != names {
        return Err(invalid(
            "snippet placeholders must exactly match declared parameters",
        ));
    }
    Ok(())
}

fn validate_snippet_parameter(parameter: &SnippetParameter) -> Result<(), PublicError> {
    if parameter.name.is_empty()
        || parameter.name.len() > 64
        || !parameter
            .name
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_alphanumeric() || byte == b'_' && index > 0)
        || parameter.name.as_bytes()[0].is_ascii_digit()
    {
        return Err(invalid("snippet parameter name is invalid"));
    }
    validate_text("snippet parameter label", &parameter.label, 1, 128)?;
    validate_text(
        "snippet parameter default",
        &parameter.default_value,
        0,
        1024,
    )?;
    if parameter.kind == SnippetParameterKind::Choice {
        if parameter.choices.is_empty()
            || parameter.choices.len() > 32
            || parameter
                .choices
                .iter()
                .any(|choice| choice.is_empty() || choice.len() > 256)
        {
            return Err(invalid("snippet choice parameter is invalid"));
        }
    } else if !parameter.choices.is_empty() {
        return Err(invalid("only choice parameters may declare choices"));
    }
    if !parameter.default_value.is_empty() {
        validate_snippet_value(parameter, &parameter.default_value)?;
    }
    Ok(())
}

fn validate_snippet_value(parameter: &SnippetParameter, value: &str) -> Result<(), PublicError> {
    if value.len() > 1024
        || value.contains(['\r', '\n', '\0'])
        || value.chars().any(char::is_control)
        || parameter.required && value.is_empty()
    {
        return Err(invalid("snippet parameter value is invalid"));
    }
    match parameter.kind {
        SnippetParameterKind::Text | SnippetParameterKind::Path => Ok(()),
        SnippetParameterKind::Integer if value.parse::<i64>().is_ok() => Ok(()),
        SnippetParameterKind::Choice if parameter.choices.iter().any(|item| item == value) => {
            Ok(())
        }
        SnippetParameterKind::Host
            if !value.is_empty()
                && !value.starts_with('-')
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'_')
                }) =>
        {
            Ok(())
        }
        _ => Err(invalid("snippet parameter value does not match its type")),
    }
}

fn snippet_placeholders(template: &str) -> Result<HashSet<&str>, PublicError> {
    let mut placeholders = HashSet::new();
    let mut remaining = template;
    while let Some(start) = remaining.find("{{") {
        let after_start = &remaining[start + 2..];
        let end = after_start
            .find("}}")
            .ok_or_else(|| invalid("snippet placeholder is not closed"))?;
        let name = &after_start[..end];
        if name.is_empty() || name.contains(['{', '}']) {
            return Err(invalid("snippet placeholder is invalid"));
        }
        placeholders.insert(name);
        remaining = &after_start[end + 2..];
    }
    if remaining.contains("}}") {
        return Err(invalid("snippet placeholder is not opened"));
    }
    Ok(placeholders)
}

fn validate_history_command(command: &str) -> Result<(), PublicError> {
    if command.trim().is_empty()
        || command.len() > 4096
        || command.contains(['\r', '\n', '\0'])
        || command.chars().any(char::is_control)
    {
        Err(invalid("command history entry is invalid"))
    } else {
        Ok(())
    }
}

fn infer_path(command: &str) -> Option<String> {
    let path = command.strip_prefix("cd ")?.trim();
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        None
    } else {
        Some(path.to_owned())
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn valid_terminal_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

fn validate_text(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), PublicError> {
    let length = value.trim().len();
    if (minimum..=maximum).contains(&length) {
        Ok(())
    } else {
        Err(invalid(format!("{field} exceeds length limits")))
    }
}

fn validate_layout(layout: WorkspaceLayout) -> Result<(), PublicError> {
    if !(180..=480).contains(&layout.left_width)
        || !(220..=520).contains(&layout.right_width)
        || !(100..=420).contains(&layout.bottom_height)
    {
        return Err(invalid("workspace panel size exceeds limits"));
    }
    Ok(())
}

fn validate_parent(
    parent_id: Option<FolderId>,
    moving: Option<FolderId>,
    folders: &[Folder],
) -> Result<(), PublicError> {
    let Some(mut cursor) = parent_id else {
        return Ok(());
    };
    let parents: HashMap<_, _> = folders
        .iter()
        .map(|folder| (folder.id, folder.parent_id))
        .collect();
    let mut visited = HashSet::new();
    loop {
        if Some(cursor) == moving || !visited.insert(cursor) {
            return Err(invalid("folder hierarchy contains a cycle"));
        }
        match parents.get(&cursor) {
            Some(Some(parent)) => cursor = *parent,
            Some(None) => return Ok(()),
            None => return Err(invalid("parent folder does not exist")),
        }
    }
}

fn validate_connection_references(
    input: &ConnectionInput,
    snapshot: &WorkspaceSnapshot,
    current_id: Option<ConnectionId>,
) -> Result<(), PublicError> {
    if input
        .folder_id
        .is_some_and(|id| !snapshot.folders.iter().any(|value| value.id == id))
        || input
            .proxy_id
            .is_some_and(|id| !snapshot.proxies.iter().any(|value| value.id == id))
        || input.terminal_profile_id.is_some_and(|id| {
            !snapshot
                .terminal_profiles
                .iter()
                .any(|value| value.id == id)
        })
    {
        return Err(invalid("connection references a missing profile"));
    }
    let mut jumps = HashSet::new();
    for jump_id in &input.jump_host_ids {
        if Some(*jump_id) == current_id || !jumps.insert(*jump_id) {
            return Err(invalid("SSH jump chain contains a loop"));
        }
        let Some(jump) = snapshot
            .connections
            .iter()
            .find(|value| value.id == *jump_id)
        else {
            return Err(invalid("SSH jump chain references a missing connection"));
        };
        if jump.kind != ConnectionKind::Ssh {
            return Err(invalid("SSH jump chain contains a non-SSH connection"));
        }
    }
    validate_jump_graph(input, snapshot, current_id)?;
    Ok(())
}

fn validate_jump_graph(
    input: &ConnectionInput,
    snapshot: &WorkspaceSnapshot,
    current_id: Option<ConnectionId>,
) -> Result<(), PublicError> {
    let mut edges = snapshot
        .connections
        .iter()
        .map(|connection| (connection.id, connection.jump_host_ids.clone()))
        .collect::<HashMap<_, _>>();
    if let Some(id) = current_id {
        edges.insert(id, input.jump_host_ids.clone());
    }
    let mut indegree = edges
        .keys()
        .map(|id| (*id, 0_usize))
        .collect::<HashMap<_, _>>();
    for targets in edges.values() {
        for target in targets {
            if let Some(value) = indegree.get_mut(target) {
                *value += 1;
            }
        }
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(*id))
        .collect::<VecDeque<_>>();
    let mut visited = 0_usize;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        for target in edges.get(&id).into_iter().flatten() {
            if let Some(count) = indegree.get_mut(target) {
                *count -= 1;
                if *count == 0 {
                    ready.push_back(*target);
                }
            }
        }
    }
    if visited == edges.len() {
        Ok(())
    } else {
        Err(invalid("SSH jump chain contains a transitive loop"))
    }
}

fn validate_snapshot(snapshot: &WorkspaceSnapshot) -> Result<(), PublicError> {
    let folder_ids: HashSet<_> = snapshot.folders.iter().map(|value| value.id).collect();
    let connection_ids: HashSet<_> = snapshot.connections.iter().map(|value| value.id).collect();
    let proxy_ids: HashSet<_> = snapshot.proxies.iter().map(|value| value.id).collect();
    let terminal_ids: HashSet<_> = snapshot
        .terminal_profiles
        .iter()
        .map(|value| value.id)
        .collect();
    let snippet_ids: HashSet<_> = snapshot.snippets.iter().map(|value| value.id).collect();
    if folder_ids.len() != snapshot.folders.len()
        || connection_ids.len() != snapshot.connections.len()
        || proxy_ids.len() != snapshot.proxies.len()
        || terminal_ids.len() != snapshot.terminal_profiles.len()
        || snippet_ids.len() != snapshot.snippets.len()
    {
        return Err(invalid("workspace contains duplicate identifiers"));
    }
    for folder in &snapshot.folders {
        validate_folder_input(&FolderInput {
            parent_id: folder.parent_id,
            sort_order: folder.sort_order,
            name: folder.name.clone(),
            notes: folder.notes.clone(),
        })?;
        validate_parent(folder.parent_id, Some(folder.id), &snapshot.folders)?;
    }
    for connection in &snapshot.connections {
        let input = ConnectionInput {
            folder_id: connection.folder_id,
            sort_order: connection.sort_order,
            kind: connection.kind,
            name: connection.name.clone(),
            host: connection.host.clone(),
            port: connection.port,
            username: connection.username.clone(),
            proxy_id: connection.proxy_id,
            jump_host_ids: connection.jump_host_ids.clone(),
            terminal_profile_id: connection.terminal_profile_id,
            authentication: connection.authentication,
            private_key_path: connection.private_key_path.clone(),
            allow_legacy_algorithms: connection.allow_legacy_algorithms,
            openssh_fallback: connection.openssh_fallback,
            tags: connection.tags.clone(),
            notes: connection.notes.clone(),
        };
        validate_connection_input(&input)?;
        validate_connection_references(&input, snapshot, Some(connection.id))?;
    }
    for proxy in &snapshot.proxies {
        validate_proxy_input(&ProxyInput {
            kind: proxy.kind,
            name: proxy.name.clone(),
            host: proxy.host.clone(),
            port: proxy.port,
            username: proxy.username.clone(),
            dns_mode: proxy.dns_mode,
            fallback_port: proxy.fallback_port,
            relay_server_name: proxy.relay_server_name.clone(),
            relay_mapping: proxy.relay_mapping.clone(),
            relay_certificate_path: proxy.relay_certificate_path.clone(),
        })?;
    }
    for terminal in &snapshot.terminal_profiles {
        validate_terminal_input(&TerminalProfileInput {
            name: terminal.name.clone(),
            font_family: terminal.font_family.clone(),
            font_size: terminal.font_size,
            theme: terminal.theme.clone(),
            palette: terminal.palette.clone(),
            scrollback: terminal.scrollback,
            encoding: terminal.encoding,
            keymap: terminal.keymap,
        })?;
    }
    for snippet in &snapshot.snippets {
        validate_snippet_input(&SnippetInput {
            name: snippet.name.clone(),
            tags: snippet.tags.clone(),
            template: snippet.template.clone(),
            parameters: snippet.parameters.clone(),
        })?;
    }
    Ok(())
}

fn validate_import_name(file_name: &str) -> Result<(), PublicError> {
    if file_name.len() > 200
        || !file_name.ends_with(".firstshell-backup")
        || Path::new(file_name)
            .file_name()
            .and_then(|value| value.to_str())
            != Some(file_name)
    {
        return Err(invalid("invalid import file name"));
    }
    Ok(())
}

fn normalized_tags(tags: Vec<String>) -> Vec<String> {
    let mut tags: Vec<_> = tags.into_iter().map(|tag| tag.trim().to_owned()).collect();
    tags.sort_unstable_by_key(|value| value.to_lowercase());
    tags.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    tags
}

fn require_revision(current: Option<u64>, expected: u64) -> Result<(), PublicError> {
    match current {
        None => Err(not_found()),
        Some(value) if value != expected => Err(conflict()),
        Some(_) => Ok(()),
    }
}

fn ensure_unlocked(session: &VaultSession) -> Result<(), PublicError> {
    if session.is_unlocked().map_err(map_security)? {
        Ok(())
    } else {
        Err(PublicError::new(ErrorCode::InvalidState, "vault is locked"))
    }
}

fn connection_kind_name(kind: ConnectionKind) -> &'static str {
    match kind {
        ConnectionKind::Ssh => "ssh",
        ConnectionKind::Rdp => "rdp",
        ConnectionKind::Local => "local",
    }
}

fn proxy_kind_name(kind: ProxyKind) -> &'static str {
    match kind {
        ProxyKind::Direct => "direct",
        ProxyKind::Socks5 => "socks5",
        ProxyKind::HttpConnect => "http_connect",
        ProxyKind::System => "system",
        ProxyKind::Relay => "relay",
    }
}

fn import_summary(snapshot: &WorkspaceSnapshot) -> ImportSummary {
    ImportSummary {
        folders: snapshot.folders.len(),
        connections: snapshot.connections.len(),
        proxies: snapshot.proxies.len(),
        terminal_profiles: snapshot.terminal_profiles.len(),
        snippets: snapshot.snippets.len(),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_storage(error: StorageError) -> PublicError {
    match error {
        StorageError::Conflict => conflict(),
        StorageError::InvalidRecord(_) => invalid("invalid storage record"),
        StorageError::ActorUnavailable => internal("workspace storage is unavailable"),
        StorageError::Sqlite(_)
        | StorageError::MigrationChecksum { .. }
        | StorageError::Integrity(_) => internal("workspace database validation failed"),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_security(error: SecurityError) -> PublicError {
    match error {
        SecurityError::VaultLocked => PublicError::new(ErrorCode::InvalidState, "vault is locked"),
        SecurityError::ExportKey | SecurityError::Authentication => {
            invalid("password or encrypted data is invalid")
        }
        SecurityError::PayloadTooLarge | SecurityError::ExportTooLarge => {
            invalid("encrypted payload exceeds limits")
        }
        SecurityError::Random(_)
        | SecurityError::InvalidKeyLength(_)
        | SecurityError::KeyDerivation
        | SecurityError::CipherInitialization
        | SecurityError::UnsupportedPayload
        | SecurityError::Keyring(_)
        | SecurityError::VaultSessionUnavailable => internal("vault operation failed"),
    }
}

fn invalid(message: impl Into<String>) -> PublicError {
    PublicError::new(ErrorCode::InvalidInput, message)
}

fn not_found() -> PublicError {
    PublicError::new(ErrorCode::NotFound, "record was not found")
}

fn conflict() -> PublicError {
    PublicError::new(ErrorCode::Conflict, "record revision conflict")
}

fn internal(message: impl Into<String>) -> PublicError {
    PublicError::new(ErrorCode::Internal, message)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::*;

    #[derive(Default)]
    struct MemoryKeyStore {
        keys: Mutex<HashMap<String, RecoveryKeySlot>>,
    }

    const MEMORY_STORE_PASSWORD: &[u8] = b"memory key store wrapping password";

    impl VaultKeyStore for MemoryKeyStore {
        fn load(&self, vault_id: &str) -> Result<VaultKey, SecurityError> {
            let keys = self
                .keys
                .lock()
                .map_err(|_| SecurityError::VaultSessionUnavailable)?;
            let slot = keys.get(vault_id).ok_or(SecurityError::VaultLocked)?;
            unwrap_vault_key(MEMORY_STORE_PASSWORD, vault_id, slot)
        }

        fn store(&self, vault_id: &str, key: &VaultKey) -> Result<(), SecurityError> {
            let slot = wrap_vault_key(MEMORY_STORE_PASSWORD, vault_id, key)?;
            self.keys
                .lock()
                .map_err(|_| SecurityError::VaultSessionUnavailable)?
                .insert(vault_id.to_owned(), slot);
            Ok(())
        }
    }

    impl MemoryKeyStore {
        fn clear(&self) {
            self.keys
                .lock()
                .expect("test key store must remain available")
                .clear();
        }
    }

    #[test]
    fn import_name_should_reject_path_traversal() {
        assert!(validate_import_name("backup.firstshell-backup").is_ok());
        assert!(validate_import_name("../backup.firstshell-backup").is_err());
        assert!(validate_import_name(r"C:\backup.firstshell-backup").is_err());
    }

    #[test]
    fn folder_parent_validation_should_reject_cycles() {
        let first = FolderId::new();
        let second = FolderId::new();
        let folders = vec![
            Folder {
                id: first,
                parent_id: Some(second),
                revision: 1,
                sort_order: 0,
                name: "First".to_owned(),
                notes: String::new(),
            },
            Folder {
                id: second,
                parent_id: Some(first),
                revision: 1,
                sort_order: 1,
                name: "Second".to_owned(),
                notes: String::new(),
            },
        ];
        assert!(validate_parent(Some(second), Some(first), &folders).is_err());
    }

    #[test]
    fn connection_validation_should_enforce_endpoint_semantics() {
        let mut remote = ConnectionInput {
            folder_id: None,
            sort_order: 0,
            kind: ConnectionKind::Ssh,
            name: "Production".to_owned(),
            host: String::new(),
            port: 22,
            username: "operator".to_owned(),
            proxy_id: None,
            jump_host_ids: Vec::new(),
            terminal_profile_id: None,
            authentication: SshAuthenticationKind::Agent,
            private_key_path: String::new(),
            allow_legacy_algorithms: false,
            openssh_fallback: false,
            tags: Vec::new(),
            notes: String::new(),
        };
        assert!(validate_connection_input(&remote).is_err());
        remote.host = "server.internal".to_owned();
        remote.username = "operator\nother".to_owned();
        assert!(validate_connection_input(&remote).is_err());
        remote.username = "operator".to_owned();
        assert!(validate_connection_input(&remote).is_ok());
    }

    #[test]
    fn connection_validation_should_reject_transitive_jump_cycles() {
        let first = ConnectionId::new();
        let second = ConnectionId::new();
        let third = ConnectionId::new();
        let profile = |id, name: &str, jump_host_ids| ConnectionProfile {
            id,
            folder_id: None,
            revision: 1,
            sort_order: 0,
            kind: ConnectionKind::Ssh,
            name: name.to_owned(),
            host: format!("{name}.internal"),
            port: 22,
            username: "operator".to_owned(),
            proxy_id: None,
            jump_host_ids,
            terminal_profile_id: None,
            authentication: SshAuthenticationKind::Agent,
            private_key_path: String::new(),
            allow_legacy_algorithms: false,
            openssh_fallback: false,
            tags: Vec::new(),
            notes: String::new(),
        };
        let snapshot = WorkspaceSnapshot {
            connections: vec![
                profile(first, "first", vec![second]),
                profile(second, "second", vec![third]),
                profile(third, "third", Vec::new()),
            ],
            ..WorkspaceSnapshot::default()
        };
        let input = ConnectionInput {
            folder_id: None,
            sort_order: 0,
            kind: ConnectionKind::Ssh,
            name: "third".to_owned(),
            host: "third.internal".to_owned(),
            port: 22,
            username: "operator".to_owned(),
            proxy_id: None,
            jump_host_ids: vec![first],
            terminal_profile_id: None,
            authentication: SshAuthenticationKind::Agent,
            private_key_path: String::new(),
            allow_legacy_algorithms: false,
            openssh_fallback: false,
            tags: Vec::new(),
            notes: String::new(),
        };

        let error = validate_connection_references(&input, &snapshot, Some(third))
            .expect_err("three-node jump cycle must be rejected");
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert!(error.message.contains("transitive loop"));
    }

    #[test]
    fn custom_terminal_theme_should_require_a_complete_palette() {
        let error = validate_terminal_input(&TerminalProfileInput {
            name: "Custom".to_owned(),
            font_family: "Cascadia Mono".to_owned(),
            font_size: 14.0,
            theme: "custom".to_owned(),
            palette: None,
            scrollback: 10_000,
            encoding: TerminalEncoding::Utf8,
            keymap: TerminalKeymap::Xterm,
        })
        .expect_err("custom themes without a palette must fail closed");
        assert_eq!(error.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn terminal_colors_should_accept_only_six_digit_hex_values() {
        assert!(valid_terminal_color("#12aBcF"));
        assert!(!valid_terminal_color("rgba(0,0,0,0.5)"));
        assert!(!valid_terminal_color("#fff"));
        assert!(!valid_terminal_color("#12345g"));
    }

    #[tokio::test]
    async fn mandatory_recovery_password_should_survive_keyring_loss()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let key_store = Arc::new(MemoryKeyStore::default());
        let service = WorkspaceService::open(
            WorkspacePaths::under(directory.path()),
            Duration::from_secs(300),
            key_store.clone(),
        )
        .await?;
        assert!(!service.vault_status()?.initialized);

        let initialized = service
            .initialize("a durable recovery password".to_owned())
            .await?;
        assert_eq!(initialized.terminal_profiles.len(), 1);
        let folder = service
            .create_folder(FolderInput {
                parent_id: None,
                sort_order: 0,
                name: "Production".to_owned(),
                notes: "private".to_owned(),
            })
            .await?;
        service.lock()?;
        key_store.clear();
        assert!(service.unlock().await.is_err());

        let recovered = service
            .recover("a durable recovery password".to_owned())
            .await?;
        assert_eq!(recovered.folders[0].id, folder.id);
        assert!(service.vault_status()?.keyring_available);

        let terminal_id = recovered.terminal_profiles[0].id;
        let connection = service
            .create_connection(ConnectionInput {
                folder_id: Some(folder.id),
                sort_order: 0,
                kind: ConnectionKind::Ssh,
                name: "Production SSH".to_owned(),
                host: "server.internal".to_owned(),
                port: 22,
                username: "operator".to_owned(),
                proxy_id: None,
                jump_host_ids: Vec::new(),
                terminal_profile_id: Some(terminal_id),
                authentication: SshAuthenticationKind::Agent,
                private_key_path: String::new(),
                allow_legacy_algorithms: false,
                openssh_fallback: false,
                tags: vec!["critical".to_owned()],
                notes: String::new(),
            })
            .await?;
        assert_eq!(service.search("server.internal", 10)?.len(), 1);
        service
            .delete_connection(connection.id, connection.revision)
            .await?;
        assert!(service.search("server.internal", 10)?.is_empty());
        let restored = service.restore_connection(connection.id, 2).await?;
        assert_eq!(restored.revision, 3);
        assert_eq!(
            service.search("critical", 10)?[0].id,
            connection.id.to_string()
        );

        let layout = WorkspaceLayout {
            left_width: 300,
            left_open: true,
            right_width: 320,
            bottom_height: 210,
            bottom_open: false,
        };
        service.save_layout(layout).await?;
        assert_eq!(service.load_layout().await?, layout);
        service.lock()?;
        assert!(service.snapshot().await.is_err());
        assert!(service.search("server.internal", 10).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn portable_restore_should_replace_data_and_create_recovery_backup()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let paths = WorkspacePaths::under(directory.path());
        let service = WorkspaceService::open(
            paths.clone(),
            Duration::from_secs(300),
            Arc::new(MemoryKeyStore::default()),
        )
        .await?;
        service
            .initialize("a durable recovery password".to_owned())
            .await?;
        let exported_folder = service
            .create_folder(FolderInput {
                parent_id: None,
                sort_order: 0,
                name: "Exported".to_owned(),
                notes: "portable".to_owned(),
            })
            .await?;
        let export = service
            .export_workspace("an independent export password".to_owned())
            .await?;
        service
            .create_folder(FolderInput {
                parent_id: None,
                sort_order: 1,
                name: "Added later".to_owned(),
                notes: String::new(),
            })
            .await?;

        assert!(
            service
                .validate_import(&export.file_name, "incorrect password".to_owned())
                .await
                .is_err()
        );
        assert_eq!(service.snapshot().await?.folders.len(), 2);

        let restored = service
            .restore_import(
                &export.file_name,
                "an independent export password".to_owned(),
            )
            .await?;
        let snapshot = service.snapshot().await?;
        assert_eq!(snapshot.folders.len(), 1);
        assert_eq!(snapshot.folders[0].id, exported_folder.id);
        assert_eq!(restored.imported.folders, 1);
        assert!(paths.recovery.join(restored.recovery_file_name).is_file());
        Ok(())
    }

    #[tokio::test]
    async fn encrypted_snippets_and_history_should_validate_render_and_restore()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let service = WorkspaceService::open(
            WorkspacePaths::under(directory.path()),
            Duration::from_secs(300),
            Arc::new(MemoryKeyStore::default()),
        )
        .await?;
        let initialized = service
            .initialize("a durable recovery password".to_owned())
            .await?;
        let connection = service
            .create_connection(ConnectionInput {
                folder_id: None,
                sort_order: 0,
                kind: ConnectionKind::Ssh,
                name: "Snippet target".to_owned(),
                host: "server.internal".to_owned(),
                port: 22,
                username: "operator".to_owned(),
                proxy_id: None,
                jump_host_ids: Vec::new(),
                terminal_profile_id: Some(initialized.terminal_profiles[0].id),
                authentication: SshAuthenticationKind::Agent,
                private_key_path: String::new(),
                allow_legacy_algorithms: false,
                openssh_fallback: false,
                tags: Vec::new(),
                notes: String::new(),
            })
            .await?;

        let undeclared = service
            .create_snippet(SnippetInput {
                name: "Invalid".to_owned(),
                tags: Vec::new(),
                template: "ping {{host}} {{missing}}".to_owned(),
                parameters: vec![snippet_parameter("host", SnippetParameterKind::Host)],
            })
            .await
            .expect_err("undeclared placeholders must fail closed");
        assert_eq!(undeclared.code, ErrorCode::InvalidInput);

        let snippet = service
            .create_snippet(SnippetInput {
                name: "Bounded ping".to_owned(),
                tags: vec!["network".to_owned(), "diagnostic".to_owned()],
                template: "ping -c {{count}} {{host}}".to_owned(),
                parameters: vec![
                    snippet_parameter("count", SnippetParameterKind::Integer),
                    snippet_parameter("host", SnippetParameterKind::Host),
                ],
            })
            .await?;
        let rendered = service
            .render_snippet(
                snippet.id,
                BTreeMap::from([
                    ("count".to_owned(), "4".to_owned()),
                    ("host".to_owned(), "example.internal".to_owned()),
                ]),
            )
            .await?;
        assert_eq!(rendered, "ping -c 4 example.internal");
        assert!(
            service
                .render_snippet(
                    snippet.id,
                    BTreeMap::from([
                        ("count".to_owned(), "4".to_owned()),
                        ("host".to_owned(), "example.internal; reboot".to_owned()),
                    ]),
                )
                .await
                .is_err()
        );
        assert_eq!(
            service.search("diagnostic", 10)?[0].id,
            snippet.id.to_string()
        );

        for index in 0..505 {
            service
                .record_command(connection.id, format!("echo history-{index}"))
                .await?;
        }
        assert_eq!(
            service
                .command_history(Some(connection.id), 500)
                .await?
                .len(),
            500
        );
        service.lock()?;
        let recovered = service
            .recover("a durable recovery password".to_owned())
            .await?;
        assert_eq!(recovered.snippets, vec![snippet]);
        let history = service.command_history(Some(connection.id), 500).await?;
        assert_eq!(history.len(), 500);
        assert_eq!(history[0].command, "echo history-504");
        assert_eq!(history[499].command, "echo history-5");
        Ok(())
    }

    #[test]
    fn relay_proxy_should_require_complete_non_secret_routing_metadata() {
        let mut input = ProxyInput {
            kind: ProxyKind::Relay,
            name: "Production relay".to_owned(),
            host: "relay.example".to_owned(),
            port: 9443,
            username: String::new(),
            dns_mode: firstshell_domain::ProxyDnsMode::Remote,
            fallback_port: 9444,
            relay_server_name: "relay.example".to_owned(),
            relay_mapping: "ssh-prod".to_owned(),
            relay_certificate_path: r"C:\relay\cert.pem".to_owned(),
        };
        assert!(validate_proxy_input(&input).is_ok());

        input.fallback_port = 0;
        assert!(validate_proxy_input(&input).is_err());
        input.fallback_port = 9444;
        input.username = "must-not-be-used-for-token".to_owned();
        assert!(validate_proxy_input(&input).is_err());
    }

    fn snippet_parameter(name: &str, kind: SnippetParameterKind) -> SnippetParameter {
        SnippetParameter {
            name: name.to_owned(),
            label: name.to_owned(),
            kind,
            required: true,
            default_value: String::new(),
            choices: Vec::new(),
        }
    }
}
