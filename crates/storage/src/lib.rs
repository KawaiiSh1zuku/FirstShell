//! Single-owner `SQLite` actor with migrations, typed records, and online backup.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode as SqliteErrorCode, MAIN_DB, OptionalExtension, params};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Bounded number of pending database operations.
pub const DB_QUEUE_CAPACITY: usize = 128;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024 + 1024;

// This migration shipped in M0. Its bytes are immutable because the checksum is persisted.
const MIGRATION_1: &str = "
CREATE TABLE probe_records (
    id TEXT PRIMARY KEY NOT NULL,
    payload BLOB NOT NULL
) STRICT;
";

const MIGRATION_2: &str = "
CREATE TABLE vault_meta (
    vault_id TEXT PRIMARY KEY NOT NULL,
    key_version INTEGER NOT NULL CHECK (key_version > 0),
    verifier BLOB NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;

CREATE TABLE vault_key_slots (
    slot_id TEXT PRIMARY KEY NOT NULL,
    vault_id TEXT NOT NULL REFERENCES vault_meta(vault_id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('recovery_password', 'device_public_key')),
    payload BLOB NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (vault_id, kind, slot_id)
) STRICT;
CREATE INDEX vault_key_slots_vault_kind ON vault_key_slots(vault_id, kind);

CREATE TABLE folders (
    id TEXT PRIMARY KEY NOT NULL,
    parent_id TEXT REFERENCES folders(id),
    sort_order INTEGER NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    deleted_at INTEGER,
    payload BLOB NOT NULL
) STRICT;
CREATE INDEX folders_parent_order ON folders(parent_id, sort_order, id);

CREATE TABLE connection_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    folder_id TEXT REFERENCES folders(id),
    kind TEXT NOT NULL CHECK (kind IN ('ssh', 'rdp', 'local')),
    sort_order INTEGER NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    deleted_at INTEGER,
    payload BLOB NOT NULL
) STRICT;
CREATE INDEX connections_folder_order ON connection_profiles(folder_id, sort_order, id);

CREATE TABLE proxy_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('direct', 'socks5', 'http_connect', 'system')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    deleted_at INTEGER,
    payload BLOB NOT NULL
) STRICT;

CREATE TABLE terminal_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    deleted_at INTEGER,
    payload BLOB NOT NULL
) STRICT;

CREATE TABLE ui_state (
    workspace_id TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    payload BLOB NOT NULL
) STRICT;
";

const MIGRATIONS: &[(u32, &str)] = &[(1, MIGRATION_1), (2, MIGRATION_2)];

/// Effective `SQLite` safety and concurrency settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseSettings {
    /// Current journal mode, expected to be `wal` for file databases.
    pub journal_mode: String,
    /// Whether foreign-key enforcement is enabled.
    pub foreign_keys: bool,
    /// Latest applied schema version.
    pub schema_version: u32,
}

/// Persistent metadata needed before a vault is unlocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultMetadata {
    /// Stable vault identifier and keyring account.
    pub vault_id: String,
    /// Current vault key generation.
    pub key_version: u32,
    /// Authenticated verifier encrypted by the vault key.
    pub verifier: Vec<u8>,
}

/// Password or device slot that wraps the vault key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultKeySlot {
    /// Stable slot identifier.
    pub slot_id: String,
    /// Owning vault identifier.
    pub vault_id: String,
    /// Versioned slot category.
    pub kind: String,
    /// Serialized authenticated wrapping envelope.
    pub payload: Vec<u8>,
}

/// Concrete record table selected without accepting arbitrary SQL identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// Connection-tree folder.
    Folder,
    /// SSH, RDP, or local connection profile.
    Connection,
    /// Proxy profile.
    Proxy,
    /// Terminal profile.
    TerminalProfile,
    /// Workspace layout state.
    UiState,
}

/// Encrypted row with a variant-specific plaintext index envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredRecord {
    /// Folder row.
    Folder {
        /// Stable identifier.
        id: String,
        /// Optional parent relation.
        parent_id: Option<String>,
        /// Ordering within the parent.
        sort_order: i64,
        /// Current optimistic revision.
        revision: u64,
        /// Serialized authenticated-encryption envelope.
        payload: Vec<u8>,
    },
    /// Connection row.
    Connection {
        /// Stable identifier.
        id: String,
        /// Optional folder relation.
        folder_id: Option<String>,
        /// Non-secret connection category.
        kind: String,
        /// Ordering within the folder.
        sort_order: i64,
        /// Current optimistic revision.
        revision: u64,
        /// Serialized authenticated-encryption envelope.
        payload: Vec<u8>,
    },
    /// Proxy row.
    Proxy {
        /// Stable identifier.
        id: String,
        /// Non-secret proxy category.
        kind: String,
        /// Current optimistic revision.
        revision: u64,
        /// Serialized authenticated-encryption envelope.
        payload: Vec<u8>,
    },
    /// Terminal profile row.
    TerminalProfile {
        /// Stable identifier.
        id: String,
        /// Current optimistic revision.
        revision: u64,
        /// Serialized authenticated-encryption envelope.
        payload: Vec<u8>,
    },
    /// Workspace layout row.
    UiState {
        /// Stable workspace identifier.
        workspace_id: String,
        /// Current optimistic revision.
        revision: u64,
        /// Serialized authenticated-encryption envelope.
        payload: Vec<u8>,
    },
}

impl StoredRecord {
    /// Returns the concrete storage category.
    #[must_use]
    pub const fn kind(&self) -> RecordKind {
        match self {
            Self::Folder { .. } => RecordKind::Folder,
            Self::Connection { .. } => RecordKind::Connection,
            Self::Proxy { .. } => RecordKind::Proxy,
            Self::TerminalProfile { .. } => RecordKind::TerminalProfile,
            Self::UiState { .. } => RecordKind::UiState,
        }
    }

    /// Returns the stable identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Folder { id, .. }
            | Self::Connection { id, .. }
            | Self::Proxy { id, .. }
            | Self::TerminalProfile { id, .. } => id,
            Self::UiState { workspace_id, .. } => workspace_id,
        }
    }

    /// Returns the current optimistic revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        match self {
            Self::Folder { revision, .. }
            | Self::Connection { revision, .. }
            | Self::Proxy { revision, .. }
            | Self::TerminalProfile { revision, .. }
            | Self::UiState { revision, .. } => *revision,
        }
    }

    /// Replaces the optimistic revision while preserving the encrypted envelope.
    pub const fn set_revision(&mut self, next_revision: u64) {
        match self {
            Self::Folder { revision, .. }
            | Self::Connection { revision, .. }
            | Self::Proxy { revision, .. }
            | Self::TerminalProfile { revision, .. }
            | Self::UiState { revision, .. } => *revision = next_revision,
        }
    }

    fn payload(&self) -> &[u8] {
        match self {
            Self::Folder { payload, .. }
            | Self::Connection { payload, .. }
            | Self::Proxy { payload, .. }
            | Self::TerminalProfile { payload, .. }
            | Self::UiState { payload, .. } => payload,
        }
    }
}

/// Storage failures returned across the actor boundary.
#[derive(Debug, Error)]
pub enum StorageError {
    /// `SQLite` rejected an operation.
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A recorded migration does not match the source bundled with this build.
    #[error("migration {version} checksum mismatch")]
    MigrationChecksum {
        /// Version whose checksum changed.
        version: u32,
    },
    /// `SQLite` integrity check did not return `ok`.
    #[error("database integrity check failed: {0}")]
    Integrity(String),
    /// A record violated storage invariants before reaching `SQLite`.
    #[error("invalid encrypted record: {0}")]
    InvalidRecord(&'static str),
    /// An optimistic update, delete, or unique insert conflicted.
    #[error("record revision conflict")]
    Conflict,
    /// The actor has stopped and cannot accept more requests.
    #[error("database actor is unavailable")]
    ActorUnavailable,
}

enum Request {
    VaultMetadata {
        reply: oneshot::Sender<Result<Option<VaultMetadata>, StorageError>>,
    },
    InitializeVault {
        metadata: VaultMetadata,
        recovery_slot: VaultKeySlot,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    RecoverySlot {
        vault_id: String,
        reply: oneshot::Sender<Result<Option<VaultKeySlot>, StorageError>>,
    },
    List {
        kind: RecordKind,
        reply: oneshot::Sender<Result<Vec<StoredRecord>, StorageError>>,
    },
    Deleted {
        kind: RecordKind,
        id: String,
        reply: oneshot::Sender<Result<Option<StoredRecord>, StorageError>>,
    },
    Save {
        record: StoredRecord,
        expected_revision: Option<u64>,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    Delete {
        kind: RecordKind,
        id: String,
        expected_revision: u64,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    Restore {
        record: StoredRecord,
        expected_revision: u64,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    ReplaceAll {
        records: Vec<StoredRecord>,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    Integrity {
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    Settings {
        reply: oneshot::Sender<Result<DatabaseSettings, StorageError>>,
    },
    Backup {
        destination: PathBuf,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
}

/// Typed asynchronous handle to a dedicated `SQLite` connection owner.
#[derive(Debug)]
pub struct DbActor {
    sender: mpsc::Sender<Request>,
}

impl DbActor {
    /// Opens, configures, migrates, and verifies a database before returning.
    ///
    /// # Errors
    /// Returns a `SQLite`, migration-checksum, integrity, or actor lifecycle error.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();
        let (sender, receiver) = mpsc::channel(DB_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = oneshot::channel();

        tokio::task::spawn_blocking(move || match open_connection(&path) {
            Ok(connection) => {
                let _ = ready_tx.send(Ok(()));
                actor_loop(connection, receiver);
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        });

        ready_rx
            .await
            .map_err(|_| StorageError::ActorUnavailable)??;
        Ok(Self { sender })
    }

    /// Loads pre-unlock vault metadata.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn vault_metadata(&self) -> Result<Option<VaultMetadata>, StorageError> {
        self.request(|reply| Request::VaultMetadata { reply }).await
    }

    /// Initializes metadata exactly once.
    ///
    /// # Errors
    /// Returns [`StorageError::Conflict`] when a vault already exists.
    pub async fn initialize_vault(
        &self,
        metadata: VaultMetadata,
        recovery_slot: VaultKeySlot,
    ) -> Result<(), StorageError> {
        if recovery_slot.vault_id != metadata.vault_id
            || recovery_slot.kind != "recovery_password"
            || recovery_slot.payload.is_empty()
        {
            return Err(StorageError::InvalidRecord("invalid recovery key slot"));
        }
        self.request(|reply| Request::InitializeVault {
            metadata,
            recovery_slot,
            reply,
        })
        .await
    }

    /// Loads the mandatory password recovery slot for a vault.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn recovery_slot(
        &self,
        vault_id: impl Into<String>,
    ) -> Result<Option<VaultKeySlot>, StorageError> {
        self.request(|reply| Request::RecoverySlot {
            vault_id: vault_id.into(),
            reply,
        })
        .await
    }

    /// Lists visible encrypted records of one concrete kind.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn list(&self, kind: RecordKind) -> Result<Vec<StoredRecord>, StorageError> {
        self.request(|reply| Request::List { kind, reply }).await
    }

    /// Loads one tombstoned encrypted record for authenticated undo.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn deleted(
        &self,
        kind: RecordKind,
        id: impl Into<String>,
    ) -> Result<Option<StoredRecord>, StorageError> {
        self.request(|reply| Request::Deleted {
            kind,
            id: id.into(),
            reply,
        })
        .await
    }

    /// Inserts or revision-checks an encrypted record.
    ///
    /// # Errors
    /// Returns a validation, conflict, storage, or actor lifecycle error.
    pub async fn save(
        &self,
        record: StoredRecord,
        expected_revision: Option<u64>,
    ) -> Result<(), StorageError> {
        validate_record(&record, expected_revision)?;
        self.request(|reply| Request::Save {
            record,
            expected_revision,
            reply,
        })
        .await
    }

    /// Tombstones a revision-checked record.
    ///
    /// # Errors
    /// Returns a conflict, storage, or actor lifecycle error.
    pub async fn delete(
        &self,
        kind: RecordKind,
        id: impl Into<String>,
        expected_revision: u64,
    ) -> Result<(), StorageError> {
        self.request(|reply| Request::Delete {
            kind,
            id: id.into(),
            expected_revision,
            reply,
        })
        .await
    }

    /// Restores a revision-checked tombstone.
    ///
    /// # Errors
    /// Returns a conflict, storage, or actor lifecycle error.
    pub async fn restore(
        &self,
        record: StoredRecord,
        expected_revision: u64,
    ) -> Result<(), StorageError> {
        validate_record(&record, Some(expected_revision))?;
        if record.kind() == RecordKind::UiState {
            return Err(StorageError::InvalidRecord("UI state cannot be restored"));
        }
        self.request(|reply| Request::Restore {
            record,
            expected_revision,
            reply,
        })
        .await
    }

    /// Atomically replaces all user records with validated revision-one rows.
    ///
    /// The vault metadata and migration history remain unchanged.
    ///
    /// # Errors
    /// Returns a validation, conflict, storage, or actor lifecycle error.
    pub async fn replace_all(&self, records: Vec<StoredRecord>) -> Result<(), StorageError> {
        for record in &records {
            validate_record(record, None)?;
        }
        self.request(|reply| Request::ReplaceAll { records, reply })
            .await
    }

    /// Runs `SQLite`'s full integrity check on the actor-owned connection.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn integrity_check(&self) -> Result<(), StorageError> {
        self.request(|reply| Request::Integrity { reply }).await
    }

    /// Returns effective connection settings for verification and diagnostics.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn settings(&self) -> Result<DatabaseSettings, StorageError> {
        self.request(|reply| Request::Settings { reply }).await
    }

    /// Creates a consistent `SQLite` online backup at `destination`.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn backup(&self, destination: impl AsRef<Path>) -> Result<(), StorageError> {
        self.request(|reply| Request::Backup {
            destination: destination.as_ref().to_path_buf(),
            reply,
        })
        .await
    }

    /// Flushes prior requests and stops the actor thread.
    ///
    /// # Errors
    /// Returns [`StorageError::ActorUnavailable`] if the actor already stopped.
    pub async fn shutdown(&self) -> Result<(), StorageError> {
        self.request(|reply| Request::Shutdown { reply }).await
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, StorageError>>) -> Request,
    ) -> Result<T, StorageError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(build(reply))
            .await
            .map_err(|_| StorageError::ActorUnavailable)?;
        response.await.map_err(|_| StorageError::ActorUnavailable)?
    }
}

fn validate_record(
    record: &StoredRecord,
    expected_revision: Option<u64>,
) -> Result<(), StorageError> {
    if record.id().is_empty()
        || record.payload().is_empty()
        || record.payload().len() > MAX_RECORD_BYTES
    {
        return Err(StorageError::InvalidRecord("identifier or payload size"));
    }
    let valid_revision = expected_revision.map_or(record.revision() == 1, |expected| {
        record.revision() == expected.saturating_add(1)
    });
    if !valid_revision {
        return Err(StorageError::InvalidRecord("revision transition"));
    }
    Ok(())
}

fn open_connection(path: &Path) -> Result<Connection, StorageError> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    apply_migrations(&mut connection)?;
    verify_integrity(&connection)?;
    Ok(connection)
}

fn apply_migrations(connection: &mut Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY NOT NULL,
            checksum TEXT NOT NULL,
            applied_at INTEGER NOT NULL DEFAULT (unixepoch())
        ) STRICT;",
    )?;

    for &(version, sql) in MIGRATIONS {
        let checksum = migration_checksum(sql);
        let recorded: Option<String> = connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version = ?1",
                [version],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(recorded) = recorded {
            if recorded != checksum {
                return Err(StorageError::MigrationChecksum { version });
            }
            continue;
        }

        let transaction = connection.transaction()?;
        transaction.execute_batch(sql)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, checksum) VALUES (?1, ?2)",
            params![version, checksum],
        )?;
        transaction.commit()?;
    }
    Ok(())
}

fn migration_checksum(sql: &str) -> String {
    let digest = Sha256::digest(sql.as_bytes());
    digest.iter().fold(
        String::with_capacity(digest.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String must not fail");
            output
        },
    )
}

fn verify_integrity(connection: &Connection) -> Result<(), StorageError> {
    let result: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(StorageError::Integrity(result))
    }
}

fn read_settings(connection: &Connection) -> Result<DatabaseSettings, StorageError> {
    let journal_mode = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    let foreign_keys: i64 =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    let schema_version = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    Ok(DatabaseSettings {
        journal_mode,
        foreign_keys: foreign_keys == 1,
        schema_version,
    })
}

fn actor_loop(mut connection: Connection, mut receiver: mpsc::Receiver<Request>) {
    while let Some(request) = receiver.blocking_recv() {
        match request {
            Request::VaultMetadata { reply } => {
                let _ = reply.send(read_vault_metadata(&connection));
            }
            Request::InitializeVault {
                metadata,
                recovery_slot,
                reply,
            } => {
                let result = initialize_vault(&mut connection, &metadata, &recovery_slot);
                let _ = reply.send(result);
            }
            Request::RecoverySlot { vault_id, reply } => {
                let _ = reply.send(read_recovery_slot(&connection, &vault_id));
            }
            Request::List { kind, reply } => {
                let _ = reply.send(list_records(&connection, kind));
            }
            Request::Deleted { kind, id, reply } => {
                let _ = reply.send(read_deleted_record(&connection, kind, &id));
            }
            Request::Save {
                record,
                expected_revision,
                reply,
            } => {
                let _ = reply.send(save_record(&connection, &record, expected_revision));
            }
            Request::Delete {
                kind,
                id,
                expected_revision,
                reply,
            } => {
                let _ = reply.send(delete_record(&connection, kind, &id, expected_revision));
            }
            Request::Restore {
                record,
                expected_revision,
                reply,
            } => {
                let _ = reply.send(restore_record(&connection, &record, expected_revision));
            }
            Request::ReplaceAll { records, reply } => {
                let _ = reply.send(replace_all_records(&mut connection, &records));
            }
            Request::Integrity { reply } => {
                let _ = reply.send(verify_integrity(&connection));
            }
            Request::Settings { reply } => {
                let _ = reply.send(read_settings(&connection));
            }
            Request::Backup { destination, reply } => {
                let result = connection
                    .backup(MAIN_DB, destination, None)
                    .map_err(StorageError::from);
                let _ = reply.send(result);
            }
            Request::Shutdown { reply } => {
                let result = connection
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .map_err(StorageError::from);
                drop(connection);
                let _ = reply.send(result);
                return;
            }
        }
    }
}

fn read_vault_metadata(connection: &Connection) -> Result<Option<VaultMetadata>, StorageError> {
    connection
        .query_row(
            "SELECT vault_id, key_version, verifier FROM vault_meta LIMIT 1",
            [],
            |row| {
                Ok(VaultMetadata {
                    vault_id: row.get(0)?,
                    key_version: row.get(1)?,
                    verifier: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(StorageError::from)
}

fn initialize_vault(
    connection: &mut Connection,
    metadata: &VaultMetadata,
    recovery_slot: &VaultKeySlot,
) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction
        .execute(
            "INSERT INTO vault_meta(vault_id, key_version, verifier) VALUES (?1, ?2, ?3)",
            params![metadata.vault_id, metadata.key_version, metadata.verifier],
        )
        .map_err(map_write_error)?;
    transaction
        .execute(
            "INSERT INTO vault_key_slots(slot_id, vault_id, kind, payload) VALUES (?1, ?2, ?3, ?4)",
            params![
                recovery_slot.slot_id,
                recovery_slot.vault_id,
                recovery_slot.kind,
                recovery_slot.payload
            ],
        )
        .map_err(map_write_error)?;
    transaction.commit().map_err(StorageError::from)
}

fn read_recovery_slot(
    connection: &Connection,
    vault_id: &str,
) -> Result<Option<VaultKeySlot>, StorageError> {
    connection
        .query_row(
            "SELECT slot_id, vault_id, kind, payload FROM vault_key_slots WHERE vault_id=?1 AND kind='recovery_password' ORDER BY created_at DESC, slot_id DESC LIMIT 1",
            [vault_id],
            |row| {
                Ok(VaultKeySlot {
                    slot_id: row.get(0)?,
                    vault_id: row.get(1)?,
                    kind: row.get(2)?,
                    payload: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(StorageError::from)
}

fn list_records(
    connection: &Connection,
    kind: RecordKind,
) -> Result<Vec<StoredRecord>, StorageError> {
    match kind {
        RecordKind::Folder => query_records(
            connection,
            "SELECT id, parent_id, sort_order, revision, payload FROM folders WHERE deleted_at IS NULL ORDER BY parent_id, sort_order, id",
            |row| {
                Ok(StoredRecord::Folder {
                    id: row.get(0)?,
                    parent_id: row.get(1)?,
                    sort_order: row.get(2)?,
                    revision: read_revision(row, 3)?,
                    payload: row.get(4)?,
                })
            },
        ),
        RecordKind::Connection => query_records(
            connection,
            "SELECT id, folder_id, kind, sort_order, revision, payload FROM connection_profiles WHERE deleted_at IS NULL ORDER BY folder_id, sort_order, id",
            |row| {
                Ok(StoredRecord::Connection {
                    id: row.get(0)?,
                    folder_id: row.get(1)?,
                    kind: row.get(2)?,
                    sort_order: row.get(3)?,
                    revision: read_revision(row, 4)?,
                    payload: row.get(5)?,
                })
            },
        ),
        RecordKind::Proxy => query_records(
            connection,
            "SELECT id, kind, revision, payload FROM proxy_profiles WHERE deleted_at IS NULL ORDER BY id",
            |row| {
                Ok(StoredRecord::Proxy {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    revision: read_revision(row, 2)?,
                    payload: row.get(3)?,
                })
            },
        ),
        RecordKind::TerminalProfile => query_records(
            connection,
            "SELECT id, revision, payload FROM terminal_profiles WHERE deleted_at IS NULL ORDER BY id",
            |row| {
                Ok(StoredRecord::TerminalProfile {
                    id: row.get(0)?,
                    revision: read_revision(row, 1)?,
                    payload: row.get(2)?,
                })
            },
        ),
        RecordKind::UiState => query_records(
            connection,
            "SELECT workspace_id, revision, payload FROM ui_state ORDER BY workspace_id",
            |row| {
                Ok(StoredRecord::UiState {
                    workspace_id: row.get(0)?,
                    revision: read_revision(row, 1)?,
                    payload: row.get(2)?,
                })
            },
        ),
    }
}

fn query_records(
    connection: &Connection,
    sql: &str,
    mut map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<StoredRecord>,
) -> Result<Vec<StoredRecord>, StorageError> {
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement.query_map([], |row| map(row))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StorageError::from)
}

fn read_deleted_record(
    connection: &Connection,
    kind: RecordKind,
    id: &str,
) -> Result<Option<StoredRecord>, StorageError> {
    let record = match kind {
        RecordKind::Folder => connection
            .query_row(
                "SELECT id, parent_id, sort_order, revision, payload FROM folders WHERE id=?1 AND deleted_at IS NOT NULL",
                [id],
                |row| Ok(StoredRecord::Folder { id: row.get(0)?, parent_id: row.get(1)?, sort_order: row.get(2)?, revision: read_revision(row, 3)?, payload: row.get(4)? }),
            )
            .optional()?,
        RecordKind::Connection => connection
            .query_row(
                "SELECT id, folder_id, kind, sort_order, revision, payload FROM connection_profiles WHERE id=?1 AND deleted_at IS NOT NULL",
                [id],
                |row| Ok(StoredRecord::Connection { id: row.get(0)?, folder_id: row.get(1)?, kind: row.get(2)?, sort_order: row.get(3)?, revision: read_revision(row, 4)?, payload: row.get(5)? }),
            )
            .optional()?,
        RecordKind::Proxy => connection
            .query_row(
                "SELECT id, kind, revision, payload FROM proxy_profiles WHERE id=?1 AND deleted_at IS NOT NULL",
                [id],
                |row| Ok(StoredRecord::Proxy { id: row.get(0)?, kind: row.get(1)?, revision: read_revision(row, 2)?, payload: row.get(3)? }),
            )
            .optional()?,
        RecordKind::TerminalProfile => connection
            .query_row(
                "SELECT id, revision, payload FROM terminal_profiles WHERE id=?1 AND deleted_at IS NOT NULL",
                [id],
                |row| Ok(StoredRecord::TerminalProfile { id: row.get(0)?, revision: read_revision(row, 1)?, payload: row.get(2)? }),
            )
            .optional()?,
        RecordKind::UiState => None,
    };
    Ok(record)
}

fn read_revision(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn sql_revision(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::InvalidRecord("revision exceeds SQLite INTEGER"))
}

fn save_record(
    connection: &Connection,
    record: &StoredRecord,
    expected_revision: Option<u64>,
) -> Result<(), StorageError> {
    let revision = sql_revision(record.revision())?;
    let changed = match (record, expected_revision) {
        (StoredRecord::Folder { id, parent_id, sort_order, payload, .. }, None) => connection.execute(
            "INSERT INTO folders(id, parent_id, sort_order, revision, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, parent_id, sort_order, revision, payload],
        ),
        (StoredRecord::Folder { id, parent_id, sort_order, payload, .. }, Some(expected)) => connection.execute(
            "UPDATE folders SET parent_id=?2, sort_order=?3, revision=?4, payload=?5 WHERE id=?1 AND revision=?6 AND deleted_at IS NULL",
            params![id, parent_id, sort_order, revision, payload, sql_revision(expected)?],
        ),
        (StoredRecord::Connection { id, folder_id, kind, sort_order, payload, .. }, None) => connection.execute(
            "INSERT INTO connection_profiles(id, folder_id, kind, sort_order, revision, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, folder_id, kind, sort_order, revision, payload],
        ),
        (StoredRecord::Connection { id, folder_id, kind, sort_order, payload, .. }, Some(expected)) => connection.execute(
            "UPDATE connection_profiles SET folder_id=?2, kind=?3, sort_order=?4, revision=?5, payload=?6 WHERE id=?1 AND revision=?7 AND deleted_at IS NULL",
            params![id, folder_id, kind, sort_order, revision, payload, sql_revision(expected)?],
        ),
        (StoredRecord::Proxy { id, kind, payload, .. }, None) => connection.execute(
            "INSERT INTO proxy_profiles(id, kind, revision, payload) VALUES (?1, ?2, ?3, ?4)",
            params![id, kind, revision, payload],
        ),
        (StoredRecord::Proxy { id, kind, payload, .. }, Some(expected)) => connection.execute(
            "UPDATE proxy_profiles SET kind=?2, revision=?3, payload=?4 WHERE id=?1 AND revision=?5 AND deleted_at IS NULL",
            params![id, kind, revision, payload, sql_revision(expected)?],
        ),
        (StoredRecord::TerminalProfile { id, payload, .. }, None) => connection.execute(
            "INSERT INTO terminal_profiles(id, revision, payload) VALUES (?1, ?2, ?3)",
            params![id, revision, payload],
        ),
        (StoredRecord::TerminalProfile { id, payload, .. }, Some(expected)) => connection.execute(
            "UPDATE terminal_profiles SET revision=?2, payload=?3 WHERE id=?1 AND revision=?4 AND deleted_at IS NULL",
            params![id, revision, payload, sql_revision(expected)?],
        ),
        (StoredRecord::UiState { workspace_id, payload, .. }, None) => connection.execute(
            "INSERT INTO ui_state(workspace_id, revision, payload) VALUES (?1, ?2, ?3)",
            params![workspace_id, revision, payload],
        ),
        (StoredRecord::UiState { workspace_id, payload, .. }, Some(expected)) => connection.execute(
            "UPDATE ui_state SET revision=?2, payload=?3 WHERE workspace_id=?1 AND revision=?4",
            params![workspace_id, revision, payload, sql_revision(expected)?],
        ),
    }
    .map_err(map_write_error)?;

    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::Conflict)
    }
}

fn delete_record(
    connection: &Connection,
    kind: RecordKind,
    id: &str,
    expected_revision: u64,
) -> Result<(), StorageError> {
    if kind == RecordKind::UiState {
        return Err(StorageError::InvalidRecord("UI state cannot be tombstoned"));
    }
    let sql = match kind {
        RecordKind::Folder => {
            "UPDATE folders SET deleted_at=unixepoch(), revision=revision+1 WHERE id=?1 AND revision=?2 AND deleted_at IS NULL"
        }
        RecordKind::Connection => {
            "UPDATE connection_profiles SET deleted_at=unixepoch(), revision=revision+1 WHERE id=?1 AND revision=?2 AND deleted_at IS NULL"
        }
        RecordKind::Proxy => {
            "UPDATE proxy_profiles SET deleted_at=unixepoch(), revision=revision+1 WHERE id=?1 AND revision=?2 AND deleted_at IS NULL"
        }
        RecordKind::TerminalProfile => {
            "UPDATE terminal_profiles SET deleted_at=unixepoch(), revision=revision+1 WHERE id=?1 AND revision=?2 AND deleted_at IS NULL"
        }
        RecordKind::UiState => unreachable!(),
    };
    let changed = connection
        .execute(sql, params![id, sql_revision(expected_revision)?])
        .map_err(map_write_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::Conflict)
    }
}

fn restore_record(
    connection: &Connection,
    record: &StoredRecord,
    expected_revision: u64,
) -> Result<(), StorageError> {
    let revision = sql_revision(record.revision())?;
    let expected = sql_revision(expected_revision)?;
    let changed = match record {
        StoredRecord::Folder { id, parent_id, sort_order, payload, .. } => connection.execute(
            "UPDATE folders SET parent_id=?2, sort_order=?3, revision=?4, payload=?5, deleted_at=NULL WHERE id=?1 AND revision=?6 AND deleted_at IS NOT NULL",
            params![id, parent_id, sort_order, revision, payload, expected],
        ),
        StoredRecord::Connection { id, folder_id, kind, sort_order, payload, .. } => connection.execute(
            "UPDATE connection_profiles SET folder_id=?2, kind=?3, sort_order=?4, revision=?5, payload=?6, deleted_at=NULL WHERE id=?1 AND revision=?7 AND deleted_at IS NOT NULL",
            params![id, folder_id, kind, sort_order, revision, payload, expected],
        ),
        StoredRecord::Proxy { id, kind, payload, .. } => connection.execute(
            "UPDATE proxy_profiles SET kind=?2, revision=?3, payload=?4, deleted_at=NULL WHERE id=?1 AND revision=?5 AND deleted_at IS NOT NULL",
            params![id, kind, revision, payload, expected],
        ),
        StoredRecord::TerminalProfile { id, payload, .. } => connection.execute(
            "UPDATE terminal_profiles SET revision=?2, payload=?3, deleted_at=NULL WHERE id=?1 AND revision=?4 AND deleted_at IS NOT NULL",
            params![id, revision, payload, expected],
        ),
        StoredRecord::UiState { .. } => return Err(StorageError::InvalidRecord("UI state cannot be restored")),
    }
    .map_err(map_write_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::Conflict)
    }
}

fn map_write_error(error: rusqlite::Error) -> StorageError {
    if matches!(
        error.sqlite_error_code(),
        Some(SqliteErrorCode::ConstraintViolation)
    ) {
        StorageError::Conflict
    } else {
        StorageError::Sqlite(error)
    }
}

fn replace_all_records(
    connection: &mut Connection,
    records: &[StoredRecord],
) -> Result<(), StorageError> {
    connection.pragma_update(None, "defer_foreign_keys", true)?;
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "DELETE FROM connection_profiles;
         DELETE FROM proxy_profiles;
         DELETE FROM terminal_profiles;
         DELETE FROM ui_state;
         DELETE FROM folders;",
    )?;
    for record in records {
        save_record(&transaction, record, None)?;
    }
    transaction.commit().map_err(StorageError::from)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn folder(revision: u64, payload: &[u8]) -> StoredRecord {
        StoredRecord::Folder {
            id: "folder-1".to_owned(),
            parent_id: None,
            sort_order: 0,
            revision,
            payload: payload.to_vec(),
        }
    }

    #[tokio::test]
    async fn actor_should_apply_migrations_and_revision_checks()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let actor = DbActor::open(directory.path().join("vault.db")).await?;
        let settings = actor.settings().await?;
        assert_eq!(settings.schema_version, 2);
        assert_eq!(settings.journal_mode, "wal");
        assert!(settings.foreign_keys);

        actor.save(folder(1, b"sealed-1"), None).await?;
        assert!(matches!(
            actor.save(folder(2, b"sealed-2"), Some(99)).await,
            Err(StorageError::InvalidRecord(_))
        ));
        actor.save(folder(2, b"sealed-2"), Some(1)).await?;
        assert_eq!(actor.list(RecordKind::Folder).await?.len(), 1);
        actor.delete(RecordKind::Folder, "folder-1", 2).await?;
        assert!(actor.list(RecordKind::Folder).await?.is_empty());
        assert_eq!(
            actor
                .deleted(RecordKind::Folder, "folder-1")
                .await?
                .map(|record| record.revision()),
            Some(3)
        );
        actor.restore(folder(4, b"sealed-4"), 3).await?;
        assert_eq!(
            actor.list(RecordKind::Folder).await?,
            vec![folder(4, b"sealed-4")]
        );
        actor.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn m0_database_should_upgrade_in_place_to_m1() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let database = directory.path().join("vault.db");
        {
            let connection = Connection::open(&database)?;
            connection.execute_batch(MIGRATION_1)?;
            connection.execute_batch(
                "CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    checksum TEXT NOT NULL
                ) STRICT;",
            )?;
            connection.execute(
                "INSERT INTO schema_migrations(version, checksum) VALUES (?1, ?2)",
                params![1, migration_checksum(MIGRATION_1)],
            )?;
            connection.execute(
                "INSERT INTO probe_records(id, payload) VALUES ('m0', X'0102')",
                [],
            )?;
        }

        let actor = DbActor::open(&database).await?;
        assert_eq!(actor.settings().await?.schema_version, 2);
        assert!(actor.vault_metadata().await?.is_none());
        actor.shutdown().await?;

        let connection = Connection::open(database)?;
        let probe: Vec<u8> = connection.query_row(
            "SELECT payload FROM probe_records WHERE id = 'm0'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(probe, vec![1, 2]);
        let slots: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'vault_key_slots'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(slots, 1);
        Ok(())
    }

    #[tokio::test]
    async fn online_backup_should_preserve_committed_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let backup_path = directory.path().join("backup.db");
        let actor = DbActor::open(directory.path().join("vault.db")).await?;
        actor.save(folder(1, b"ciphertext"), None).await?;
        actor.backup(&backup_path).await?;
        actor.shutdown().await?;

        let restored = DbActor::open(backup_path).await?;
        let records = restored.list(RecordKind::Folder).await?;
        restored.shutdown().await?;
        assert_eq!(records, vec![folder(1, b"ciphertext")]);
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_primary_should_fail_closed_while_backup_remains_recoverable()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let primary_path = directory.path().join("vault.db");
        let backup_path = directory.path().join("backup.db");
        let actor = DbActor::open(&primary_path).await?;
        actor.save(folder(1, b"sealed"), None).await?;
        actor.backup(&backup_path).await?;
        actor.shutdown().await?;

        fs::write(&primary_path, b"not a sqlite database")?;
        assert!(DbActor::open(&primary_path).await.is_err());

        let recovered = DbActor::open(backup_path).await?;
        assert_eq!(recovered.list(RecordKind::Folder).await?.len(), 1);
        recovered.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn vault_metadata_should_initialize_exactly_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let actor = DbActor::open(directory.path().join("vault.db")).await?;
        let metadata = VaultMetadata {
            vault_id: "vault-1".to_owned(),
            key_version: 1,
            verifier: b"sealed-verifier".to_vec(),
        };
        let slot = VaultKeySlot {
            slot_id: "recovery-1".to_owned(),
            vault_id: metadata.vault_id.clone(),
            kind: "recovery_password".to_owned(),
            payload: b"wrapped-vault-key".to_vec(),
        };
        actor
            .initialize_vault(metadata.clone(), slot.clone())
            .await?;
        assert_eq!(actor.vault_metadata().await?, Some(metadata.clone()));
        assert_eq!(actor.recovery_slot("vault-1").await?, Some(slot.clone()));
        assert!(matches!(
            actor.initialize_vault(metadata, slot).await,
            Err(StorageError::Conflict)
        ));
        actor.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn replace_all_should_be_atomic_and_remove_prior_rows()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let actor = DbActor::open(directory.path().join("vault.db")).await?;
        actor.save(folder(1, b"before"), None).await?;
        let replacement = StoredRecord::TerminalProfile {
            id: "terminal-1".to_owned(),
            revision: 1,
            payload: b"after".to_vec(),
        };
        actor.replace_all(vec![replacement.clone()]).await?;
        assert!(actor.list(RecordKind::Folder).await?.is_empty());
        assert_eq!(
            actor.list(RecordKind::TerminalProfile).await?,
            vec![replacement]
        );
        actor.shutdown().await?;
        Ok(())
    }
}
