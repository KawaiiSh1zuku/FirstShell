//! Rooted SFTP workspace and resumable transfer primitives.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh::ChannelMsg;
use russh_sftp::client::{Config as SftpConfig, SftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use zmodem2::{Action as ZmodemAction, Event as ZmodemEvent, FileInfo, Position};

use super::{AuthenticatedChain, SshError, SshSessionFactory};
use firstshell_session::{INTERACTIVE_QUEUE_CAPACITY, SessionEvent};

const TRANSFER_CHUNK_BYTES: usize = 256 * 1024;
const TRANSFER_PACKET_BYTES: u32 = 256 * 1024;
const MAX_REMOTE_PATH_BYTES: usize = 4096;
const TEMP_SUFFIX: &str = ".firstshell-part";
const MAX_REMOTE_EDIT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ARCHIVE_SOURCES: usize = 64;
const MAX_REMOTE_TASK_OUTPUT: usize = 64 * 1024;

/// Observable remote entry category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteEntryKind {
    /// Regular remote file.
    File,
    /// Remote directory.
    Directory,
    /// Symbolic link that is never followed implicitly.
    Symlink,
    /// Device, socket, FIFO, or unknown remote entry.
    Other,
}

/// One rooted SFTP directory entry safe to expose to the desktop UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    /// Path relative to the selected workspace root.
    pub path: String,
    /// Final path component.
    pub name: String,
    /// Entry category without following links.
    pub kind: RemoteEntryKind,
    /// File size when supplied by the server.
    pub size: Option<u64>,
    /// Unix modification time when supplied by the server.
    pub modified_at: Option<u32>,
    /// Raw POSIX permission bits when supplied by the server.
    pub permissions: Option<u32>,
    /// Link target for symbolic links only.
    pub link_target: Option<String>,
}

/// Existing-target behavior for a transfer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Refuse to overwrite an existing completed target.
    #[default]
    Deny,
    /// Replace the completed target after the temporary transfer succeeds.
    Replace,
    /// Continue a matching `.firstshell-part` file from its current size.
    Resume,
}

/// Optional post-transfer verification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// Verify length and successful close only.
    #[default]
    Size,
    /// Re-read both sides and compare SHA-256 digests before replacement.
    Sha256,
}

/// User-selected transfer behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferOptions {
    /// Conflict and resume strategy.
    pub conflict: ConflictPolicy,
    /// Verification performed before final replacement.
    pub verify: VerifyMode,
}

/// Progress emitted by a running transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    /// Bytes confirmed by the local async write path.
    pub transferred: u64,
    /// Expected total bytes.
    pub total: u64,
}

/// Successful transfer outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReceipt {
    /// Final byte length.
    pub bytes: u64,
    /// Optional lowercase SHA-256 digest.
    pub sha256: Option<String>,
    /// Whether an existing partial file supplied the starting offset.
    pub resumed: bool,
}

/// Bounded UTF-8 remote document opened with a conflict fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEditDocument {
    /// Root-relative file path.
    pub path: String,
    /// Decoded UTF-8 content without an optional BOM.
    pub content: String,
    /// SHA-256 fingerprint of the original remote bytes.
    pub fingerprint: String,
    /// Whether the original bytes carried a UTF-8 BOM.
    pub utf8_bom: bool,
    /// Original remote byte length.
    pub bytes: u64,
}

/// Successful controlled remote archive creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveReceipt {
    /// Root-relative archive path.
    pub path: String,
    /// Final remote archive length.
    pub bytes: u64,
}

/// Authenticated SFTP workspace confined to one canonical remote root.
pub struct SftpWorkspace {
    root: String,
    sftp: Arc<SftpSession>,
    chain: Mutex<Option<AuthenticatedChain>>,
}

impl std::fmt::Debug for SftpWorkspace {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SftpWorkspace")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl SshSessionFactory {
    /// Opens an independently scheduled SFTP channel rooted at a canonical remote directory.
    ///
    /// # Errors
    /// Returns an SSH or SFTP error when authentication, host-key validation, subsystem startup,
    /// or root canonicalization fails.
    pub async fn open_sftp(&self, root: &str) -> Result<SftpWorkspace, SftpError> {
        validate_remote_input(root, true)?;
        let (events, _receiver) = mpsc::channel::<SessionEvent>(INTERACTIVE_QUEUE_CAPACITY);
        let chain = self.connect_authenticated(events).await?;
        let channel = chain
            .final_client
            .channel_open_session()
            .await
            .map_err(SshError::Russh)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(SshError::Russh)?;
        let sftp = SftpSession::new_with_config(
            channel.into_stream(),
            SftpConfig {
                max_packet_len: TRANSFER_PACKET_BYTES,
                max_concurrent_writes: 8,
                request_timeout_secs: 30,
            },
        )
        .await
        .map_err(SftpError::protocol)?;
        let root =
            normalize_canonical_root(&sftp.canonicalize(root).await.map_err(SftpError::protocol)?)?;
        Ok(SftpWorkspace {
            root,
            sftp: Arc::new(sftp),
            chain: Mutex::new(Some(chain)),
        })
    }
}

impl SftpWorkspace {
    /// Returns the canonical root. Paths returned by other APIs remain relative to it.
    #[must_use]
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Lists a directory without implicitly following symbolic links.
    ///
    /// # Errors
    /// Rejects traversal outside the root, symlink traversal, and SFTP failures.
    pub async fn list(&self, relative: &str) -> Result<Vec<RemoteEntry>, SftpError> {
        let directory = self
            .resolve_existing(relative, EntryExpectation::Directory)
            .await?;
        let entries = self
            .sftp
            .read_dir(&directory)
            .await
            .map_err(SftpError::protocol)?;
        let mut result = Vec::new();
        for entry in entries {
            let name = entry.file_name();
            let metadata = entry.metadata();
            let kind = entry_kind(&metadata);
            let relative_path = join_relative(relative, &name)?;
            let link_target = if kind == RemoteEntryKind::Symlink {
                self.sftp.read_link(entry.path()).await.ok()
            } else {
                None
            };
            result.push(RemoteEntry {
                path: relative_path,
                name,
                kind,
                size: metadata.size,
                modified_at: metadata.mtime,
                permissions: metadata.permissions,
                link_target,
            });
        }
        result.sort_by(|left, right| {
            entry_sort_key(left.kind)
                .cmp(&entry_sort_key(right.kind))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(result)
    }

    /// Creates one directory beneath an existing rooted parent.
    ///
    /// # Errors
    /// Rejects unsafe names, symlink parents, traversal, and server failures.
    pub async fn create_dir(&self, parent: &str, name: &str) -> Result<(), SftpError> {
        validate_name(name)?;
        let parent = self
            .resolve_existing(parent, EntryExpectation::Directory)
            .await?;
        self.sftp
            .create_dir(join_remote(&parent, name))
            .await
            .map_err(SftpError::protocol)
    }

    /// Renames one entry within its existing parent.
    ///
    /// # Errors
    /// Rejects symbolic links, unsafe names, traversal, and server failures.
    pub async fn rename(&self, relative: &str, name: &str) -> Result<String, SftpError> {
        validate_name(name)?;
        let source = self
            .resolve_existing(relative, EntryExpectation::Any)
            .await?;
        let parent_relative = relative_parent(relative)?;
        let parent = self
            .resolve_existing(&parent_relative, EntryExpectation::Directory)
            .await?;
        let destination = join_remote(&parent, name);
        self.sftp
            .rename(source, destination)
            .await
            .map_err(SftpError::protocol)?;
        join_relative(&parent_relative, name)
    }

    /// Deletes a regular file, symlink itself, or an empty directory.
    ///
    /// # Errors
    /// Rejects traversal and non-empty directories; it never recursively follows links.
    pub async fn delete(&self, relative: &str) -> Result<(), SftpError> {
        let normalized = normalize_relative(relative)?;
        if normalized.is_empty() {
            return Err(SftpError::RootMutation);
        }
        let path = join_root(&self.root, &normalized);
        let metadata = self
            .sftp
            .symlink_metadata(&path)
            .await
            .map_err(SftpError::protocol)?;
        if metadata.is_dir() {
            self.sftp
                .remove_dir(path)
                .await
                .map_err(SftpError::protocol)
        } else {
            self.sftp
                .remove_file(path)
                .await
                .map_err(SftpError::protocol)
        }
    }

    /// Changes POSIX permission bits without changing the remote entry type.
    ///
    /// # Errors
    /// Rejects symbolic links, roots, out-of-range modes, traversal, and server failures.
    pub async fn chmod(&self, relative: &str, mode: u32) -> Result<(), SftpError> {
        if mode > 0o7777 || normalize_relative(relative)?.is_empty() {
            return Err(SftpError::InvalidPath);
        }
        let path = self
            .resolve_existing(relative, EntryExpectation::Any)
            .await?;
        self.sftp
            .set_metadata(
                path,
                FileAttributes {
                    permissions: Some(mode),
                    ..FileAttributes::empty()
                },
            )
            .await
            .map_err(SftpError::protocol)
    }

    /// Opens one bounded regular UTF-8 file for conflict-aware remote editing.
    ///
    /// # Errors
    /// Rejects traversal, symbolic links, non-UTF-8/NUL content, oversized files, and SFTP errors.
    pub async fn open_text(&self, relative: &str) -> Result<RemoteEditDocument, SftpError> {
        let remote = self
            .resolve_existing(relative, EntryExpectation::File)
            .await?;
        let metadata = self
            .sftp
            .metadata(&remote)
            .await
            .map_err(SftpError::protocol)?;
        let bytes = metadata.size.ok_or(SftpError::VerificationFailed)?;
        if bytes > MAX_REMOTE_EDIT_BYTES as u64 {
            return Err(SftpError::EditTooLarge);
        }
        let mut file = self.sftp.open(&remote).await.map_err(SftpError::protocol)?;
        let mut raw = Vec::with_capacity(usize::try_from(bytes).unwrap_or(MAX_REMOTE_EDIT_BYTES));
        {
            let mut limited = (&mut file).take((MAX_REMOTE_EDIT_BYTES + 1) as u64);
            limited.read_to_end(&mut raw).await?;
        }
        file.shutdown().await?;
        if raw.len() > MAX_REMOTE_EDIT_BYTES {
            return Err(SftpError::EditTooLarge);
        }
        let utf8_bom = raw.starts_with(&[0xEF, 0xBB, 0xBF]);
        let text_bytes = if utf8_bom { &raw[3..] } else { &raw };
        let content = std::str::from_utf8(text_bytes)
            .map_err(|_| SftpError::UnsupportedEncoding)?
            .to_owned();
        if content.contains('\0') {
            return Err(SftpError::UnsupportedEncoding);
        }
        Ok(RemoteEditDocument {
            path: normalize_relative(relative)?,
            content,
            fingerprint: format!("{:x}", Sha256::digest(&raw)),
            utf8_bom,
            bytes,
        })
    }

    /// Saves one bounded UTF-8 document only when the remote fingerprint is unchanged.
    ///
    /// # Errors
    /// Rejects stale fingerprints before writing, unsafe paths, oversized/NUL content, and SFTP errors.
    pub async fn save_text(
        &self,
        relative: &str,
        expected_fingerprint: &str,
        content: &str,
        utf8_bom: bool,
    ) -> Result<RemoteEditDocument, SftpError> {
        if content.len() > MAX_REMOTE_EDIT_BYTES
            || content.contains('\0')
            || expected_fingerprint.len() != 64
            || !expected_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(SftpError::InvalidEdit);
        }
        let current = self.open_text(relative).await?;
        if current.fingerprint != expected_fingerprint {
            return Err(SftpError::Conflict);
        }
        let final_path = self
            .resolve_existing(relative, EntryExpectation::File)
            .await?;
        let temp_path = format!(
            "{final_path}.firstshell-edit-{}",
            &expected_fingerprint[..12]
        );
        let _ = self.sftp.remove_file(&temp_path).await;
        let mut destination = self
            .sftp
            .create(&temp_path)
            .await
            .map_err(SftpError::protocol)?;
        if utf8_bom {
            destination.write_all(&[0xEF, 0xBB, 0xBF]).await?;
        }
        destination.write_all(content.as_bytes()).await?;
        destination.flush().await?;
        destination.sync_all().await.map_err(SftpError::protocol)?;
        destination.shutdown().await?;
        let latest = self.open_text(relative).await?;
        if latest.fingerprint != expected_fingerprint {
            let _ = self.sftp.remove_file(&temp_path).await;
            return Err(SftpError::Conflict);
        }
        self.sftp
            .remove_file(&final_path)
            .await
            .map_err(SftpError::protocol)?;
        self.sftp
            .rename(temp_path, final_path)
            .await
            .map_err(SftpError::protocol)?;
        self.open_text(relative).await
    }

    /// Creates a gzip-compressed tar archive from rooted entries using a fixed command template.
    ///
    /// # Errors
    /// Rejects empty/oversized source sets, traversal, symbolic links, existing destinations,
    /// command failure, timeout, excessive output, and SFTP errors.
    pub async fn create_archive(
        &self,
        sources: &[String],
        destination: &str,
    ) -> Result<ArchiveReceipt, SftpError> {
        if sources.is_empty() || sources.len() > MAX_ARCHIVE_SOURCES {
            return Err(SftpError::InvalidArchive);
        }
        let destination_relative = normalize_relative(destination)?;
        if !destination_relative.ends_with(".tar.gz") {
            return Err(SftpError::InvalidArchive);
        }
        let destination_remote = self.resolve_new_file(&destination_relative).await?;
        if self
            .sftp
            .try_exists(&destination_remote)
            .await
            .map_err(SftpError::protocol)?
        {
            return Err(SftpError::Conflict);
        }
        let mut resolved = Vec::with_capacity(sources.len());
        for source in sources {
            resolved.push(self.resolve_existing(source, EntryExpectation::Any).await?);
        }
        let temp_remote = format!("{destination_remote}{TEMP_SUFFIX}");
        let mut command = format!("LC_ALL=C tar -czf {} --", shell_quote(&temp_remote));
        for source in &resolved {
            command.push(' ');
            command.push_str(&shell_quote(source));
        }
        let (status, _) = self
            .run_controlled_command(&command, Duration::from_secs(120))
            .await?;
        if status != Some(0) {
            let _ = self.sftp.remove_file(&temp_remote).await;
            return Err(SftpError::RemoteCommandFailed);
        }
        let metadata = self
            .sftp
            .metadata(&temp_remote)
            .await
            .map_err(SftpError::protocol)?;
        self.sftp
            .rename(temp_remote, destination_remote)
            .await
            .map_err(SftpError::protocol)?;
        Ok(ArchiveReceipt {
            path: destination_relative,
            bytes: metadata.size.ok_or(SftpError::VerificationFailed)?,
        })
    }

    /// Uploads one regular file through a caller-driven ZMODEM session and remote `rz`.
    ///
    /// # Errors
    /// Rejects unsafe paths, existing targets, files larger than the protocol position space,
    /// cancellation, remote-tool absence, protocol failure, and local I/O errors.
    pub async fn zmodem_upload(
        &self,
        local_source: &Path,
        remote_directory: &str,
        progress: &mpsc::Sender<TransferProgress>,
        cancellation: &CancellationToken,
    ) -> Result<TransferReceipt, SftpError> {
        let metadata = fs::metadata(local_source).await?;
        if !metadata.is_file() {
            return Err(SftpError::LocalNotFile);
        }
        let size = u32::try_from(metadata.len()).map_err(|_| SftpError::ZmodemLimit)?;
        let name = local_source
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(SftpError::InvalidLocalPath)?;
        validate_name(name)?;
        let directory = self
            .resolve_existing(remote_directory, EntryExpectation::Directory)
            .await?;
        let remote_target = join_remote(&directory, name);
        if self
            .sftp
            .try_exists(&remote_target)
            .await
            .map_err(SftpError::protocol)?
        {
            return Err(SftpError::Conflict);
        }
        let command = format!("cd {} && exec rz --protect", shell_quote(&directory));
        let mut stream = self.open_exec_stream(&command).await?;
        let mut file = fs::File::open(local_source).await?;
        let mut sender = zmodem2::Sender::new().map_err(|_| SftpError::Zmodem)?;
        sender
            .start_file(FileInfo::new(name.as_bytes(), Some(Position::new(size))))
            .map_err(|_| SftpError::Zmodem)?;
        let mut finishing = false;
        let mut wire = vec![0_u8; 8192];
        let started = tokio::time::Instant::now();
        loop {
            if started.elapsed() > Duration::from_secs(10 * 60) {
                sender.abort();
                return Err(SftpError::Timeout);
            }
            match sender.poll() {
                ZmodemAction::WriteWire(bytes) => {
                    let bytes = bytes.to_vec();
                    let written = tokio::select! {
                        () = cancellation.cancelled() => { sender.abort(); return Err(SftpError::Cancelled); }
                        result = stream.write(&bytes) => result?,
                    };
                    sender.wire_written(written);
                }
                ZmodemAction::ReadFile { offset, max_len } => {
                    file.seek(SeekFrom::Start(u64::from(offset.get()))).await?;
                    let mut buffer = vec![0_u8; max_len.min(256 * 1024)];
                    let read = tokio::select! {
                        () = cancellation.cancelled() => { sender.abort(); return Err(SftpError::Cancelled); }
                        result = file.read(&mut buffer) => result?,
                    };
                    sender
                        .submit_file(&buffer[..read])
                        .map_err(|_| SftpError::Zmodem)?;
                    let transferred = u64::from(offset.get()).saturating_add(read as u64);
                    let _ = progress.try_send(TransferProgress {
                        transferred,
                        total: metadata.len(),
                    });
                }
                ZmodemAction::Event(event) => match event {
                    ZmodemEvent::FileCompleted if !finishing => {
                        sender.finish().map_err(|_| SftpError::Zmodem)?;
                        finishing = true;
                    }
                    ZmodemEvent::SessionCompleted => break,
                    ZmodemEvent::Aborted => return Err(SftpError::Zmodem),
                    _ => {}
                },
                ZmodemAction::Idle => {
                    match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut wire)).await
                    {
                        Ok(Ok(0)) => return Err(SftpError::Zmodem),
                        Ok(Ok(read)) => {
                            sender
                                .submit_wire(&wire[..read])
                                .map_err(|_| SftpError::Zmodem)?;
                        }
                        Ok(Err(error)) => return Err(error.into()),
                        Err(_) => sender.timeout().map_err(|_| SftpError::Zmodem)?,
                    }
                }
                _ => return Err(SftpError::Zmodem),
            }
        }
        let _ = stream.shutdown().await;
        let _ = progress.try_send(TransferProgress {
            transferred: metadata.len(),
            total: metadata.len(),
        });
        Ok(TransferReceipt {
            bytes: metadata.len(),
            sha256: None,
            resumed: false,
        })
    }

    /// Downloads one regular file through a caller-driven ZMODEM session and remote `sz`.
    ///
    /// # Errors
    /// Rejects unsafe paths, unexpected advertised names, cancellation, remote-tool absence,
    /// protocol failure, and local I/O errors. The final path changes only after completion.
    pub async fn zmodem_download(
        &self,
        remote_relative: &str,
        local_destination: &Path,
        progress: &mpsc::Sender<TransferProgress>,
        cancellation: &CancellationToken,
    ) -> Result<TransferReceipt, SftpError> {
        let remote = self
            .resolve_existing(remote_relative, EntryExpectation::File)
            .await?;
        let metadata = self
            .sftp
            .metadata(&remote)
            .await
            .map_err(SftpError::protocol)?;
        let total = metadata.size.ok_or(SftpError::VerificationFailed)?;
        u32::try_from(total).map_err(|_| SftpError::ZmodemLimit)?;
        let name = remote_relative
            .rsplit('/')
            .next()
            .ok_or(SftpError::InvalidPath)?;
        validate_name(name)?;
        let parent_relative = relative_parent(remote_relative)?;
        let directory = self
            .resolve_existing(&parent_relative, EntryExpectation::Directory)
            .await?;
        let command = format!(
            "cd {} && exec sz -- {}",
            shell_quote(&directory),
            shell_quote(name)
        );
        let temp_path = local_temp_path(local_destination)?;
        let _ = fs::remove_file(&temp_path).await;
        let mut destination = fs::File::create(&temp_path).await?;
        let mut stream = self.open_exec_stream(&command).await?;
        let mut receiver = zmodem2::Receiver::new().map_err(|_| SftpError::Zmodem)?;
        let transferred = receive_zmodem_file(
            &mut receiver,
            &mut stream,
            &mut destination,
            name,
            total,
            progress,
            cancellation,
        )
        .await?;
        let _ = stream.shutdown().await;
        destination.flush().await?;
        destination.sync_all().await?;
        destination.shutdown().await?;
        if transferred != total {
            return Err(SftpError::VerificationFailed);
        }
        if fs::try_exists(local_destination).await? {
            fs::remove_file(local_destination).await?;
        }
        fs::rename(&temp_path, local_destination).await?;
        Ok(TransferReceipt {
            bytes: total,
            sha256: None,
            resumed: false,
        })
    }

    /// Uploads one local file through a resumable remote temporary file.
    ///
    /// # Errors
    /// Preserves the partial file on cancellation or recoverable failure and rejects changes to
    /// the local source observed during transfer.
    pub async fn upload(
        &self,
        local_source: &Path,
        remote_relative: &str,
        options: TransferOptions,
        progress: &mpsc::Sender<TransferProgress>,
        cancellation: &CancellationToken,
    ) -> Result<TransferReceipt, SftpError> {
        let source_before = fs::metadata(local_source).await?;
        if !source_before.is_file() {
            return Err(SftpError::LocalNotFile);
        }
        let final_path = self.resolve_new_file(remote_relative).await?;
        let temp_path = format!("{final_path}{TEMP_SUFFIX}");
        let final_exists = self
            .sftp
            .try_exists(&final_path)
            .await
            .map_err(SftpError::protocol)?;
        if final_exists && options.conflict == ConflictPolicy::Deny {
            return Err(SftpError::Conflict);
        }

        let mut offset = if options.conflict == ConflictPolicy::Resume {
            self.sftp
                .metadata(&temp_path)
                .await
                .ok()
                .and_then(|metadata| metadata.size)
                .unwrap_or(0)
        } else {
            let _ = self.sftp.remove_file(&temp_path).await;
            0
        };
        if offset > source_before.len() {
            let _ = self.sftp.remove_file(&temp_path).await;
            offset = 0;
        }
        let resumed = offset > 0;
        let mut source = fs::File::open(local_source).await?;
        source.seek(SeekFrom::Start(offset)).await?;
        let flags = OpenFlags::CREATE | OpenFlags::WRITE;
        let mut destination = self
            .sftp
            .open_with_flags(&temp_path, flags)
            .await
            .map_err(SftpError::protocol)?;
        destination.seek(SeekFrom::Start(offset)).await?;
        copy_with_progress(
            &mut source,
            &mut destination,
            offset,
            source_before.len(),
            progress,
            cancellation,
        )
        .await?;
        destination.flush().await?;
        destination.sync_all().await.map_err(SftpError::protocol)?;
        destination.shutdown().await?;

        let source_after = fs::metadata(local_source).await?;
        if source_after.len() != source_before.len()
            || source_after.modified().ok() != source_before.modified().ok()
        {
            return Err(SftpError::SourceChanged);
        }
        let remote_size = self
            .sftp
            .metadata(&temp_path)
            .await
            .map_err(SftpError::protocol)?
            .size
            .ok_or(SftpError::VerificationFailed)?;
        if remote_size != source_before.len() {
            return Err(SftpError::VerificationFailed);
        }
        let sha256 = if options.verify == VerifyMode::Sha256 {
            let local = hash_local(local_source, cancellation).await?;
            let remote = self.hash_remote(&temp_path, cancellation).await?;
            if local != remote {
                return Err(SftpError::VerificationFailed);
            }
            Some(local)
        } else {
            None
        };
        if final_exists {
            self.sftp
                .remove_file(&final_path)
                .await
                .map_err(SftpError::protocol)?;
        }
        self.sftp
            .rename(temp_path, final_path)
            .await
            .map_err(SftpError::protocol)?;
        Ok(TransferReceipt {
            bytes: source_before.len(),
            sha256,
            resumed,
        })
    }

    /// Downloads one remote file through a resumable local temporary file.
    ///
    /// # Errors
    /// Preserves the local partial file on cancellation or recoverable failure.
    pub async fn download(
        &self,
        remote_relative: &str,
        local_destination: &Path,
        options: TransferOptions,
        progress: &mpsc::Sender<TransferProgress>,
        cancellation: &CancellationToken,
    ) -> Result<TransferReceipt, SftpError> {
        let remote_path = self
            .resolve_existing(remote_relative, EntryExpectation::File)
            .await?;
        let total = self
            .sftp
            .metadata(&remote_path)
            .await
            .map_err(SftpError::protocol)?
            .size
            .ok_or(SftpError::VerificationFailed)?;
        if fs::try_exists(local_destination).await? && options.conflict == ConflictPolicy::Deny {
            return Err(SftpError::Conflict);
        }
        let temp_path = local_temp_path(local_destination)?;
        let mut offset = if options.conflict == ConflictPolicy::Resume {
            fs::metadata(&temp_path)
                .await
                .ok()
                .map_or(0, |metadata| metadata.len())
        } else {
            let _ = fs::remove_file(&temp_path).await;
            0
        };
        if offset > total {
            let _ = fs::remove_file(&temp_path).await;
            offset = 0;
        }
        let resumed = offset > 0;
        let mut source = self
            .sftp
            .open(remote_path)
            .await
            .map_err(SftpError::protocol)?;
        source.seek(SeekFrom::Start(offset)).await?;
        let mut destination = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(offset == 0)
            .open(&temp_path)
            .await?;
        destination.seek(SeekFrom::Start(offset)).await?;
        copy_with_progress(
            &mut source,
            &mut destination,
            offset,
            total,
            progress,
            cancellation,
        )
        .await?;
        destination.flush().await?;
        destination.sync_all().await?;
        source.shutdown().await?;
        if fs::metadata(&temp_path).await?.len() != total {
            return Err(SftpError::VerificationFailed);
        }
        let sha256 = if options.verify == VerifyMode::Sha256 {
            Some(hash_local(&temp_path, cancellation).await?)
        } else {
            None
        };
        if fs::try_exists(local_destination).await? {
            fs::remove_file(local_destination).await?;
        }
        fs::rename(&temp_path, local_destination).await?;
        Ok(TransferReceipt {
            bytes: total,
            sha256,
            resumed,
        })
    }

    /// Closes the SFTP subsystem and underlying SSH connection.
    pub async fn close(&self) {
        let _ = self.sftp.close().await;
        if let Some(chain) = self.chain.lock().await.take() {
            chain.disconnect("SFTP workspace closed").await;
        }
    }

    async fn resolve_existing(
        &self,
        relative: &str,
        expectation: EntryExpectation,
    ) -> Result<String, SftpError> {
        let normalized = normalize_relative(relative)?;
        let joined = join_root(&self.root, &normalized);
        let metadata = self
            .sftp
            .symlink_metadata(&joined)
            .await
            .map_err(SftpError::protocol)?;
        if metadata.is_symlink() {
            return Err(SftpError::SymlinkTraversal);
        }
        match expectation {
            EntryExpectation::Directory if !metadata.is_dir() => {
                return Err(SftpError::NotDirectory);
            }
            EntryExpectation::File if !metadata.is_regular() => return Err(SftpError::NotFile),
            _ => {}
        }
        let canonical = self
            .sftp
            .canonicalize(&joined)
            .await
            .map_err(SftpError::protocol)?;
        ensure_beneath_root(&self.root, &canonical)?;
        Ok(canonical)
    }

    async fn resolve_new_file(&self, relative: &str) -> Result<String, SftpError> {
        let normalized = normalize_relative(relative)?;
        if normalized.is_empty() {
            return Err(SftpError::RootMutation);
        }
        let parent = relative_parent(&normalized)?;
        let name = normalized
            .rsplit('/')
            .next()
            .ok_or(SftpError::InvalidPath)?;
        validate_name(name)?;
        let parent = self
            .resolve_existing(&parent, EntryExpectation::Directory)
            .await?;
        Ok(join_remote(&parent, name))
    }

    async fn hash_remote(
        &self,
        remote_path: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, SftpError> {
        let mut file = self
            .sftp
            .open(remote_path)
            .await
            .map_err(SftpError::protocol)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
        loop {
            let read = tokio::select! {
                () = cancellation.cancelled() => return Err(SftpError::Cancelled),
                result = file.read(&mut buffer) => result?,
            };
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
        file.shutdown().await?;
        Ok(format!("{:x}", hash.finalize()))
    }

    async fn run_controlled_command(
        &self,
        command: &str,
        timeout: std::time::Duration,
    ) -> Result<(Option<u32>, Vec<u8>), SftpError> {
        let chain = self.chain.lock().await;
        let chain = chain.as_ref().ok_or(SftpError::WorkspaceClosed)?;
        let task = async {
            let mut channel = chain
                .final_client
                .channel_open_session()
                .await
                .map_err(SshError::Russh)?;
            channel
                .exec(true, command.as_bytes())
                .await
                .map_err(SshError::Russh)?;
            let mut output = Vec::new();
            let mut status = None;
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                        if output.len().saturating_add(data.len()) > MAX_REMOTE_TASK_OUTPUT {
                            return Err(SftpError::OutputLimit);
                        }
                        output.extend_from_slice(&data);
                    }
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    _ => {}
                }
            }
            Ok((status, output))
        };
        tokio::time::timeout(timeout, task)
            .await
            .map_err(|_| SftpError::Timeout)?
    }

    async fn open_exec_stream(
        &self,
        command: &str,
    ) -> Result<russh::ChannelStream<russh::client::Msg>, SftpError> {
        let chain = self.chain.lock().await;
        let chain = chain.as_ref().ok_or(SftpError::WorkspaceClosed)?;
        let channel = chain
            .final_client
            .channel_open_session()
            .await
            .map_err(SshError::Russh)?;
        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(SshError::Russh)?;
        Ok(channel.into_stream())
    }
}

async fn receive_zmodem_file(
    receiver: &mut zmodem2::Receiver,
    stream: &mut russh::ChannelStream<russh::client::Msg>,
    destination: &mut fs::File,
    name: &str,
    total: u64,
    progress: &mpsc::Sender<TransferProgress>,
    cancellation: &CancellationToken,
) -> Result<u64, SftpError> {
    let mut wire = vec![0_u8; 8192];
    let mut transferred = 0_u64;
    let mut file_started = false;
    let started = tokio::time::Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(10 * 60) {
            receiver.abort().map_err(|_| SftpError::Zmodem)?;
            return Err(SftpError::Timeout);
        }
        match receiver.poll() {
            ZmodemAction::WriteWire(bytes) => {
                let bytes = bytes.to_vec();
                let written = tokio::select! {
                    () = cancellation.cancelled() => { let _ = receiver.abort(); return Err(SftpError::Cancelled); }
                    result = stream.write(&bytes) => result?,
                };
                receiver.wire_written(written);
            }
            ZmodemAction::WriteFile(bytes) => {
                let bytes = bytes.to_vec();
                let written = tokio::select! {
                    () = cancellation.cancelled() => { let _ = receiver.abort(); return Err(SftpError::Cancelled); }
                    result = destination.write(&bytes) => result?,
                };
                receiver
                    .file_written(written)
                    .map_err(|_| SftpError::Zmodem)?;
                transferred = transferred.saturating_add(written as u64);
                let _ = progress.try_send(TransferProgress { transferred, total });
            }
            ZmodemAction::Event(ZmodemEvent::FileStarted(info)) => {
                if file_started || info.name != name.as_bytes() {
                    let _ = receiver.abort();
                    return Err(SftpError::ZmodemNameMismatch);
                }
                file_started = true;
            }
            ZmodemAction::Event(ZmodemEvent::SessionCompleted) if file_started => {
                return Ok(transferred);
            }
            ZmodemAction::Event(ZmodemEvent::SessionCompleted | ZmodemEvent::Aborted) => {
                return Err(SftpError::Zmodem);
            }
            ZmodemAction::Event(_) => {}
            ZmodemAction::Idle => {
                match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut wire)).await {
                    Ok(Ok(0)) => return Err(SftpError::Zmodem),
                    Ok(Ok(read)) => receiver
                        .submit_wire(&wire[..read])
                        .map(|_| ())
                        .map_err(|_| SftpError::Zmodem)?,
                    Ok(Err(error)) => return Err(error.into()),
                    Err(_) => receiver.timeout().map_err(|_| SftpError::Zmodem)?,
                }
            }
            _ => return Err(SftpError::Zmodem),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum EntryExpectation {
    Any,
    File,
    Directory,
}

async fn copy_with_progress<R, W>(
    source: &mut R,
    destination: &mut W,
    mut transferred: u64,
    total: u64,
    progress: &mpsc::Sender<TransferProgress>,
    cancellation: &CancellationToken,
) -> Result<(), SftpError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let _ = progress.try_send(TransferProgress { transferred, total });
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    loop {
        let read = tokio::select! {
            () = cancellation.cancelled() => return Err(SftpError::Cancelled),
            result = source.read(&mut buffer) => result?,
        };
        if read == 0 {
            break;
        }
        tokio::select! {
            () = cancellation.cancelled() => return Err(SftpError::Cancelled),
            result = destination.write_all(&buffer[..read]) => result?,
        }
        transferred = transferred.saturating_add(read as u64);
        let _ = progress.try_send(TransferProgress { transferred, total });
    }
    Ok(())
}

async fn hash_local(path: &Path, cancellation: &CancellationToken) -> Result<String, SftpError> {
    let mut file = fs::File::open(path).await?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
    loop {
        let read = tokio::select! {
            () = cancellation.cancelled() => return Err(SftpError::Cancelled),
            result = file.read(&mut buffer) => result?,
        };
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn normalize_canonical_root(root: &str) -> Result<String, SftpError> {
    if !root.starts_with('/') || root.len() > MAX_REMOTE_PATH_BYTES || root.contains('\0') {
        return Err(SftpError::InvalidRoot);
    }
    if root == "/" {
        Ok(root.to_owned())
    } else {
        Ok(root.trim_end_matches('/').to_owned())
    }
}

fn normalize_relative(path: &str) -> Result<String, SftpError> {
    validate_remote_input(path, false)?;
    if path.starts_with('/') || path.contains('\\') {
        return Err(SftpError::InvalidPath);
    }
    let mut parts = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(SftpError::PathTraversal),
            value => parts.push(value),
        }
    }
    Ok(parts.join("/"))
}

fn validate_remote_input(path: &str, allow_absolute: bool) -> Result<(), SftpError> {
    if path.len() > MAX_REMOTE_PATH_BYTES
        || path.contains('\0')
        || path.chars().any(char::is_control)
        || (!allow_absolute && path.starts_with('/'))
    {
        return Err(SftpError::InvalidPath);
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), SftpError> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control)
        || name.len() > 255
    {
        return Err(SftpError::InvalidPath);
    }
    Ok(())
}

fn relative_parent(relative: &str) -> Result<String, SftpError> {
    let normalized = normalize_relative(relative)?;
    Ok(normalized
        .rsplit_once('/')
        .map_or_else(String::new, |(parent, _)| parent.to_owned()))
}

fn join_relative(parent: &str, name: &str) -> Result<String, SftpError> {
    validate_name(name)?;
    let parent = normalize_relative(parent)?;
    Ok(if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    })
}

fn join_root(root: &str, relative: &str) -> String {
    if relative.is_empty() {
        root.to_owned()
    } else {
        join_remote(root, relative)
    }
}

fn join_remote(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{}/{child}", parent.trim_end_matches('/'))
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn ensure_beneath_root(root: &str, path: &str) -> Result<(), SftpError> {
    if root == "/"
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
    {
        Ok(())
    } else {
        Err(SftpError::PathTraversal)
    }
}

fn entry_kind(metadata: &FileAttributes) -> RemoteEntryKind {
    if metadata.is_dir() {
        RemoteEntryKind::Directory
    } else if metadata.is_symlink() {
        RemoteEntryKind::Symlink
    } else if metadata.is_regular() {
        RemoteEntryKind::File
    } else {
        RemoteEntryKind::Other
    }
}

const fn entry_sort_key(kind: RemoteEntryKind) -> u8 {
    match kind {
        RemoteEntryKind::Directory => 0,
        RemoteEntryKind::File => 1,
        RemoteEntryKind::Symlink => 2,
        RemoteEntryKind::Other => 3,
    }
}

fn local_temp_path(destination: &Path) -> Result<PathBuf, SftpError> {
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(SftpError::InvalidLocalPath)?;
    Ok(destination.with_file_name(format!("{file_name}{TEMP_SUFFIX}")))
}

/// Stable rooted-SFTP and transfer failure.
#[derive(Debug, Error)]
pub enum SftpError {
    /// SSH authentication, host-key, or transport setup failed.
    #[error("SFTP SSH setup failed")]
    Ssh(#[from] SshError),
    /// Remote path syntax is invalid.
    #[error("invalid remote path")]
    InvalidPath,
    /// Canonical remote root is invalid.
    #[error("invalid SFTP root")]
    InvalidRoot,
    /// A path attempted to escape the selected root.
    #[error("remote path traversal rejected")]
    PathTraversal,
    /// An operation attempted to follow a symbolic link.
    #[error("symbolic link traversal rejected")]
    SymlinkTraversal,
    /// The selected workspace root cannot be mutated.
    #[error("SFTP workspace root cannot be mutated")]
    RootMutation,
    /// The remote entry is not a directory.
    #[error("remote entry is not a directory")]
    NotDirectory,
    /// The remote entry is not a regular file.
    #[error("remote entry is not a regular file")]
    NotFile,
    /// The local source is not a regular file.
    #[error("local source is not a regular file")]
    LocalNotFile,
    /// The local destination path cannot carry a safe temporary sibling.
    #[error("invalid local destination path")]
    InvalidLocalPath,
    /// Existing destination conflicts with the selected policy.
    #[error("transfer target already exists")]
    Conflict,
    /// Transfer was cancelled while preserving its partial file.
    #[error("transfer cancelled")]
    Cancelled,
    /// Local upload source changed during the transfer.
    #[error("local source changed during transfer")]
    SourceChanged,
    /// Size or digest verification failed.
    #[error("transfer verification failed")]
    VerificationFailed,
    /// Remote editor only accepts bounded UTF-8 text.
    #[error("remote edit file exceeds size limit")]
    EditTooLarge,
    /// Remote editor detected unsupported content encoding.
    #[error("remote edit file encoding is unsupported")]
    UnsupportedEncoding,
    /// Remote editor request contains an invalid fingerprint or content.
    #[error("invalid remote edit request")]
    InvalidEdit,
    /// Controlled archive request is empty, oversized, or has an invalid destination.
    #[error("invalid controlled archive request")]
    InvalidArchive,
    /// Controlled remote command returned a failure status.
    #[error("controlled remote command failed")]
    RemoteCommandFailed,
    /// Controlled remote command exceeded its output cap.
    #[error("controlled remote command output limit exceeded")]
    OutputLimit,
    /// Controlled remote command exceeded its deadline.
    #[error("controlled remote command timed out")]
    Timeout,
    /// Workspace connection has already been closed.
    #[error("SFTP workspace is closed")]
    WorkspaceClosed,
    /// ZMODEM state machine or remote peer failed.
    #[error("ZMODEM protocol failed")]
    Zmodem,
    /// ZMODEM position space cannot represent the selected file.
    #[error("ZMODEM file exceeds protocol limit")]
    ZmodemLimit,
    /// Remote peer advertised a file name other than the explicitly selected file.
    #[error("ZMODEM remote file name mismatch")]
    ZmodemNameMismatch,
    /// Redacted SFTP protocol failure.
    #[error("SFTP operation failed")]
    Protocol(String),
    /// Local filesystem I/O failure.
    #[error("local transfer I/O failed")]
    Io(#[from] std::io::Error),
}

impl SftpError {
    fn protocol(error: impl std::fmt::Display) -> Self {
        let _ = error;
        Self::Protocol("remote server rejected the SFTP operation".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zmodem2::{Receiver, Sender};

    #[test]
    fn relative_paths_should_reject_parent_and_absolute_traversal() {
        assert!(matches!(
            normalize_relative("a/../b"),
            Err(SftpError::PathTraversal)
        ));
        assert!(matches!(
            normalize_relative("/etc"),
            Err(SftpError::InvalidPath)
        ));
    }

    #[test]
    fn root_boundary_should_not_accept_similar_prefixes() {
        assert!(ensure_beneath_root("/srv/root", "/srv/root/file").is_ok());
        assert!(matches!(
            ensure_beneath_root("/srv/root", "/srv/root-escape/file"),
            Err(SftpError::PathTraversal)
        ));
    }

    #[test]
    fn names_should_reject_separators_and_control_characters() {
        assert!(validate_name("report.txt").is_ok());
        assert!(matches!(
            validate_name("../report"),
            Err(SftpError::InvalidPath)
        ));
        assert!(matches!(
            validate_name("bad\nname"),
            Err(SftpError::InvalidPath)
        ));
    }

    #[test]
    fn controlled_archive_arguments_should_remain_single_shell_words() {
        assert_eq!(shell_quote("/srv/a b"), "'/srv/a b'");
        assert_eq!(shell_quote("/srv/a'b"), "'/srv/a'\"'\"'b'");
        assert!(normalize_relative("logs.tar.gz").is_ok());
        assert!(normalize_relative("../logs.tar.gz").is_err());
    }

    #[test]
    fn zmodem_core_should_round_trip_bounded_bytes_in_memory() {
        let contents = b"FirstShell bounded ZMODEM round trip\0\x01\xff";
        let mut sender = Sender::new().expect("sender state machine");
        let mut receiver = Receiver::new().expect("receiver state machine");
        sender
            .start_file(FileInfo::new(
                b"payload.bin",
                Some(Position::new(
                    u32::try_from(contents.len()).expect("small fixture"),
                )),
            ))
            .expect("file offer");
        sender.finish().expect("single-file finish request");
        let mut downstream = Vec::new();
        let mut upstream = Vec::new();
        let mut persisted = Vec::new();
        let mut file_done = false;
        let mut session_done = false;

        for _ in 0..100_000 {
            let mut progressed = false;
            match sender.poll() {
                ZmodemAction::WriteWire(bytes) => {
                    let count = bytes.len();
                    downstream.extend_from_slice(bytes);
                    sender.wire_written(count);
                    progressed = true;
                }
                ZmodemAction::ReadFile { offset, max_len } => {
                    let start = offset.get() as usize;
                    let end = start.saturating_add(max_len).min(contents.len());
                    sender
                        .submit_file(&contents[start..end])
                        .expect("sender file input");
                    progressed = true;
                }
                ZmodemAction::Event(ZmodemEvent::SessionCompleted) => {
                    session_done = true;
                    progressed = true;
                }
                ZmodemAction::Event(_) => progressed = true,
                ZmodemAction::Idle if !upstream.is_empty() => {
                    let consumed = sender.submit_wire(&upstream).expect("sender wire input");
                    upstream.drain(..consumed);
                    progressed = true;
                }
                ZmodemAction::Idle => {}
                other => panic!("unexpected sender action: {other:?}"),
            }
            match receiver.poll() {
                ZmodemAction::WriteWire(bytes) => {
                    let count = bytes.len();
                    upstream.extend_from_slice(bytes);
                    receiver.wire_written(count);
                    progressed = true;
                }
                ZmodemAction::WriteFile(bytes) => {
                    let count = bytes.len();
                    persisted.extend_from_slice(bytes);
                    receiver.file_written(count).expect("receiver file output");
                    progressed = true;
                }
                ZmodemAction::Event(ZmodemEvent::FileCompleted) => {
                    file_done = true;
                    progressed = true;
                }
                ZmodemAction::Event(_) => progressed = true,
                ZmodemAction::Idle if !downstream.is_empty() => {
                    let consumed = receiver
                        .submit_wire(&downstream)
                        .expect("receiver wire input");
                    downstream.drain(..consumed);
                    progressed = true;
                }
                ZmodemAction::Idle => {}
                other => panic!("unexpected receiver action: {other:?}"),
            }
            if session_done && downstream.is_empty() && upstream.is_empty() {
                break;
            }
            assert!(progressed, "in-memory ZMODEM state machine deadlocked");
        }
        assert!(file_done);
        assert!(session_done);
        assert_eq!(persisted, contents);
    }
}
