//! Single-owner `SQLite` actor with migrations, integrity checks, and online backup.

use std::fmt::Write as _;

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, MAIN_DB, OptionalExtension};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Bounded number of pending database operations.
pub const DB_QUEUE_CAPACITY: usize = 128;
const MIGRATION_1: &str = "
CREATE TABLE probe_records (
    id TEXT PRIMARY KEY NOT NULL,
    payload BLOB NOT NULL
) STRICT;
";

/// Effective `SQLite` safety and concurrency settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseSettings {
    /// Current journal mode, expected to be `wal` for file databases.
    pub journal_mode: String,
    /// Whether foreign-key enforcement is enabled.
    pub foreign_keys: bool,
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
    /// The actor has stopped and cannot accept more requests.
    #[error("database actor is unavailable")]
    ActorUnavailable,
}

enum Request {
    PutProbe {
        id: String,
        payload: Vec<u8>,
        reply: oneshot::Sender<Result<(), StorageError>>,
    },
    GetProbe {
        id: String,
        reply: oneshot::Sender<Result<Option<Vec<u8>>, StorageError>>,
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

    /// Inserts or replaces a probe record through the database actor.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn put_probe(
        &self,
        id: impl Into<String>,
        payload: Vec<u8>,
    ) -> Result<(), StorageError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::PutProbe {
                id: id.into(),
                payload,
                reply,
            })
            .await
            .map_err(|_| StorageError::ActorUnavailable)?;
        response.await.map_err(|_| StorageError::ActorUnavailable)?
    }

    /// Reads a probe record through the database actor.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn get_probe(&self, id: impl Into<String>) -> Result<Option<Vec<u8>>, StorageError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::GetProbe {
                id: id.into(),
                reply,
            })
            .await
            .map_err(|_| StorageError::ActorUnavailable)?;
        response.await.map_err(|_| StorageError::ActorUnavailable)?
    }

    /// Runs `SQLite`'s full integrity check on the actor-owned connection.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn integrity_check(&self) -> Result<(), StorageError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::Integrity { reply })
            .await
            .map_err(|_| StorageError::ActorUnavailable)?;
        response.await.map_err(|_| StorageError::ActorUnavailable)?
    }

    /// Returns effective connection settings for verification and diagnostics.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error.
    pub async fn settings(&self) -> Result<DatabaseSettings, StorageError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::Settings { reply })
            .await
            .map_err(|_| StorageError::ActorUnavailable)?;
        response.await.map_err(|_| StorageError::ActorUnavailable)?
    }

    /// Creates a consistent `SQLite` online backup at `destination`.
    ///
    /// # Errors
    /// Returns a storage or actor lifecycle error. Existing destinations are
    /// opened as `SQLite` databases and replaced through the backup API.
    pub async fn backup(&self, destination: impl AsRef<Path>) -> Result<(), StorageError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::Backup {
                destination: destination.as_ref().to_path_buf(),
                reply,
            })
            .await
            .map_err(|_| StorageError::ActorUnavailable)?;
        response.await.map_err(|_| StorageError::ActorUnavailable)?
    }

    /// Flushes prior requests and stops the actor thread.
    ///
    /// # Errors
    /// Returns [`StorageError::ActorUnavailable`] if the actor already stopped.
    pub async fn shutdown(self) -> Result<(), StorageError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Request::Shutdown { reply })
            .await
            .map_err(|_| StorageError::ActorUnavailable)?;
        response.await.map_err(|_| StorageError::ActorUnavailable)?
    }
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

    let checksum = migration_checksum(MIGRATION_1);
    let recorded: Option<String> = connection
        .query_row(
            "SELECT checksum FROM schema_migrations WHERE version = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(recorded) = recorded {
        if recorded != checksum {
            return Err(StorageError::MigrationChecksum { version: 1 });
        }
        return Ok(());
    }

    let transaction = connection.transaction()?;
    transaction.execute_batch(MIGRATION_1)?;
    transaction.execute(
        "INSERT INTO schema_migrations(version, checksum) VALUES (1, ?1)",
        [&checksum],
    )?;
    transaction.commit()?;
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
    Ok(DatabaseSettings {
        journal_mode,
        foreign_keys: foreign_keys == 1,
    })
}

fn actor_loop(connection: Connection, mut receiver: mpsc::Receiver<Request>) {
    while let Some(request) = receiver.blocking_recv() {
        match request {
            Request::PutProbe { id, payload, reply } => {
                let result = connection
                    .execute(
                        "INSERT INTO probe_records(id, payload) VALUES (?1, ?2)
                         ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
                        rusqlite::params![id, payload],
                    )
                    .map(|_| ())
                    .map_err(StorageError::from);
                let _ = reply.send(result);
            }
            Request::GetProbe { id, reply } => {
                let result = connection
                    .query_row(
                        "SELECT payload FROM probe_records WHERE id = ?1",
                        [id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(StorageError::from);
                let _ = reply.send(result);
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn actor_should_enable_wal_and_foreign_keys() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let actor = DbActor::open(directory.path().join("vault.db")).await?;
        let settings = actor.settings().await?;
        actor.shutdown().await?;
        assert_eq!(
            settings,
            DatabaseSettings {
                journal_mode: "wal".to_owned(),
                foreign_keys: true
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn online_backup_should_preserve_committed_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let backup_path = directory.path().join("backup.db");
        let actor = DbActor::open(directory.path().join("vault.db")).await?;
        actor.put_probe("record-1", b"ciphertext".to_vec()).await?;
        actor.backup(&backup_path).await?;
        actor.shutdown().await?;

        let restored = DbActor::open(backup_path).await?;
        let payload = restored.get_probe("record-1").await?;
        restored.shutdown().await?;
        assert_eq!(payload.as_deref(), Some(b"ciphertext".as_slice()));
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_primary_should_fail_closed_while_backup_remains_recoverable()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let primary_path = directory.path().join("vault.db");
        let backup_path = directory.path().join("backup.db");
        let actor = DbActor::open(&primary_path).await?;
        actor.put_probe("record-1", b"sealed".to_vec()).await?;
        actor.backup(&backup_path).await?;
        actor.shutdown().await?;

        fs::write(&primary_path, b"not a sqlite database")?;
        assert!(DbActor::open(&primary_path).await.is_err());

        let recovered = DbActor::open(backup_path).await?;
        let payload = recovered.get_probe("record-1").await?;
        recovered.shutdown().await?;
        assert_eq!(payload.as_deref(), Some(b"sealed".as_slice()));
        Ok(())
    }
}
