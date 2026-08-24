//! Desktop orchestration for rooted SFTP workspaces, transfers, and SSH tunnels.

// Tauri commands deserialize owned arguments at the IPC boundary.
#![allow(clippy::needless_pass_by_value)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use firstshell_domain::{ConnectionId, ErrorCode, PublicError};
use firstshell_ssh::{
    RemoteEntry, RunningTunnel, SftpError, SftpWorkspace, SshSessionFactory, TransferOptions,
    TransferProgress, TransferReceipt, TunnelSnapshot, TunnelSpec,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::commands::DesktopState;
use crate::session_commands::{DesktopHostKeyEvidence, public, resolve_ssh_connection};

const MAX_TRANSFER_JOBS: usize = 256;
const MAX_CONCURRENT_TRANSFERS: usize = 2;
const MAX_STAGED_DROP_BATCHES: usize = 8;
const MAX_UPLOAD_FILES: usize = 64;
const STAGED_DROP_TTL: Duration = Duration::from_secs(5 * 60);

/// Desktop-owned bulk-operation state. Secrets and local paths remain inside this boundary.
pub struct DesktopOperations {
    sftp_workspaces: Mutex<HashMap<Uuid, SftpRecord>>,
    tunnels: Mutex<HashMap<Uuid, TunnelRecord>>,
    transfers: Mutex<HashMap<Uuid, TransferRecord>>,
    staged_uploads: Mutex<HashMap<Uuid, StagedUploadRecord>>,
    transfer_permits: Arc<Semaphore>,
}

impl DesktopOperations {
    /// Creates empty bounded registries.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sftp_workspaces: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
            transfers: Mutex::new(HashMap::new()),
            staged_uploads: Mutex::new(HashMap::new()),
            transfer_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_TRANSFERS)),
        }
    }

    /// Cancels and closes all bulk resources when the Vault locks.
    pub async fn close_all(&self) {
        for record in self.transfers.lock().await.values_mut() {
            record.cancellation.cancel();
            if !record.snapshot.status.is_terminal() {
                record.snapshot.status = TransferStatus::Cancelling;
            }
        }
        self.transfers.lock().await.clear();
        self.staged_uploads.lock().await.clear();
        let workspaces = std::mem::take(&mut *self.sftp_workspaces.lock().await);
        let tunnels = std::mem::take(&mut *self.tunnels.lock().await);
        for record in workspaces.into_values() {
            if let SftpRecord::Ready { workspace, .. } = record {
                workspace.close().await;
            }
        }
        for record in tunnels.into_values() {
            if let TunnelRecord::Running { tunnel, .. } = record {
                tunnel.close().await;
            }
        }
    }

    async fn stage_uploads(&self, selected: Vec<PathBuf>) -> Result<StagedUploadDrop, PublicError> {
        if selected.is_empty() || selected.len() > MAX_UPLOAD_FILES {
            return Err(public(
                ErrorCode::ResourceExhausted,
                "dropped upload batch must contain 1 to 64 files",
            ));
        }
        let mut files = Vec::with_capacity(selected.len());
        let mut paths = Vec::with_capacity(selected.len());
        for selected_path in selected {
            let metadata = tokio::fs::symlink_metadata(&selected_path)
                .await
                .map_err(|_| public(ErrorCode::InvalidInput, "dropped file is unavailable"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(public(
                    ErrorCode::InvalidInput,
                    "dropped upload item must be a regular file",
                ));
            }
            let canonical = tokio::fs::canonicalize(&selected_path)
                .await
                .map_err(|_| public(ErrorCode::InvalidInput, "dropped file is unavailable"))?;
            let name = canonical
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| public(ErrorCode::InvalidInput, "dropped file name is invalid"))?
                .to_owned();
            files.push(StagedUploadFile {
                name,
                size: metadata.len(),
            });
            paths.push(canonical);
        }

        let now = Instant::now();
        let mut staged = self.staged_uploads.lock().await;
        staged.retain(|_, value| now.duration_since(value.created_at) < STAGED_DROP_TTL);
        if staged.len() >= MAX_STAGED_DROP_BATCHES {
            return Err(public(
                ErrorCode::ResourceExhausted,
                "too many unconsumed dropped upload batches",
            ));
        }
        let batch_id = Uuid::new_v4();
        staged.insert(
            batch_id,
            StagedUploadRecord {
                created_at: now,
                paths,
            },
        );
        Ok(StagedUploadDrop {
            batch_id: Some(batch_id),
            files,
            error: None,
        })
    }

    async fn take_staged_uploads(&self, batch_id: Uuid) -> Result<Vec<PathBuf>, PublicError> {
        let record = self
            .staged_uploads
            .lock()
            .await
            .remove(&batch_id)
            .ok_or_else(|| {
                public(
                    ErrorCode::NotFound,
                    "dropped upload batch expired or was used",
                )
            })?;
        if record.created_at.elapsed() >= STAGED_DROP_TTL {
            Err(public(
                ErrorCode::InvalidState,
                "dropped upload batch expired",
            ))
        } else {
            Ok(record.paths)
        }
    }
}

struct StagedUploadRecord {
    created_at: Instant,
    paths: Vec<PathBuf>,
}

/// Safe metadata emitted after Rust stages a native file drop.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedUploadFile {
    name: String,
    size: u64,
}

/// Native drop result that never contains a local path.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedUploadDrop {
    batch_id: Option<Uuid>,
    files: Vec<StagedUploadFile>,
    error: Option<String>,
}

impl StagedUploadDrop {
    fn rejected(error: PublicError) -> Self {
        Self {
            batch_id: None,
            files: Vec::new(),
            error: Some(error.message),
        }
    }
}

/// Stages native drag-and-drop paths inside Rust and returns only safe metadata.
pub async fn stage_sftp_drop(
    operations: &DesktopOperations,
    paths: Vec<PathBuf>,
) -> StagedUploadDrop {
    operations
        .stage_uploads(paths)
        .await
        .unwrap_or_else(StagedUploadDrop::rejected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Queued,
    Running,
    Pausing,
    Cancelling,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl TransferStatus {
    const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSnapshot {
    id: Uuid,
    workspace_id: Uuid,
    generation: u64,
    direction: TransferDirection,
    name: String,
    remote_path: String,
    status: TransferStatus,
    transferred: u64,
    total: u64,
    resumed: bool,
    sha256: Option<String>,
    error: Option<String>,
}

#[derive(Clone)]
enum TransferOperation {
    Upload {
        workspace_id: Uuid,
        local_path: PathBuf,
        remote_path: String,
        options: TransferOptions,
    },
    Download {
        workspace_id: Uuid,
        remote_path: String,
        local_path: PathBuf,
        options: TransferOptions,
    },
}

#[derive(Clone)]
struct TransferRecord {
    snapshot: TransferSnapshot,
    operation: TransferOperation,
    cancellation: CancellationToken,
    stop_status: TransferStatus,
    generation: u64,
}

impl Default for DesktopOperations {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
enum SftpRecord {
    Pending {
        connection_id: ConnectionId,
        root: String,
        factory: Arc<SshSessionFactory>,
    },
    Ready {
        workspace: Arc<SftpWorkspace>,
    },
}

#[derive(Clone)]
enum TunnelRecord {
    Pending {
        connection_id: ConnectionId,
        spec: TunnelSpec,
        factory: Arc<SshSessionFactory>,
    },
    Running {
        connection_id: ConnectionId,
        tunnel: Arc<RunningTunnel>,
    },
}

/// SFTP workspace connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SftpWorkspaceState {
    /// Another host key in the direct/jump chain requires approval.
    HostKeyRequired,
    /// Root canonicalization and SFTP startup completed.
    Ready,
}

/// Public rooted SFTP workspace state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpWorkspaceSnapshot {
    id: Uuid,
    connection_id: ConnectionId,
    root: String,
    state: SftpWorkspaceState,
    host_key: Option<DesktopHostKeyEvidence>,
}

/// Tunnel startup state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    /// Another host key in the direct/jump chain requires approval.
    HostKeyRequired,
    /// The listener or remote forwarding request is active.
    Running,
}

/// Public desktop tunnel state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopTunnelSnapshot {
    id: Uuid,
    connection_id: ConnectionId,
    state: TunnelState,
    tunnel: Option<TunnelSnapshot>,
    host_key: Option<DesktopHostKeyEvidence>,
}

fn authorize(window: &WebviewWindow) -> Result<(), PublicError> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(public(
            ErrorCode::InvalidState,
            "window is not authorized for file or tunnel access",
        ))
    }
}

#[tauri::command]
pub async fn open_sftp_workspace(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
    root: String,
) -> Result<SftpWorkspaceSnapshot, PublicError> {
    authorize(&window)?;
    if root.is_empty() || root.len() > 4096 {
        return Err(public(ErrorCode::InvalidInput, "SFTP root is invalid"));
    }
    let factory = resolve_ssh_connection(&state, connection_id).await?.factory;
    let id = Uuid::new_v4();
    match factory.open_sftp(&root).await {
        Ok(workspace) => {
            let root = workspace.root().to_owned();
            state.operations.sftp_workspaces.lock().await.insert(
                id,
                SftpRecord::Ready {
                    workspace: Arc::new(workspace),
                },
            );
            Ok(SftpWorkspaceSnapshot {
                id,
                connection_id,
                root,
                state: SftpWorkspaceState::Ready,
                host_key: None,
            })
        }
        Err(_) if factory.pending_host_key().is_some() => {
            let host_key = factory.pending_host_key().map(Into::into);
            state.operations.sftp_workspaces.lock().await.insert(
                id,
                SftpRecord::Pending {
                    connection_id,
                    root: root.clone(),
                    factory,
                },
            );
            Ok(SftpWorkspaceSnapshot {
                id,
                connection_id,
                root,
                state: SftpWorkspaceState::HostKeyRequired,
                host_key,
            })
        }
        Err(_) => Err(public(
            ErrorCode::InvalidState,
            "SFTP workspace could not be opened",
        )),
    }
}

#[tauri::command]
pub async fn approve_sftp_host_key(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    fingerprint: String,
    previous_fingerprint: Option<String>,
) -> Result<SftpWorkspaceSnapshot, PublicError> {
    authorize(&window)?;
    validate_fingerprints(&fingerprint, previous_fingerprint.as_deref())?;
    let record = state
        .operations
        .sftp_workspaces
        .lock()
        .await
        .get(&workspace_id)
        .cloned()
        .ok_or_else(|| public(ErrorCode::NotFound, "SFTP workspace not found"))?;
    let SftpRecord::Pending {
        connection_id,
        root,
        factory,
    } = record
    else {
        return Err(public(
            ErrorCode::InvalidState,
            "SFTP workspace is already ready",
        ));
    };
    approve_factory(&factory, &fingerprint, previous_fingerprint.as_deref())?;
    match factory.open_sftp(&root).await {
        Ok(workspace) => {
            let canonical_root = workspace.root().to_owned();
            state.operations.sftp_workspaces.lock().await.insert(
                workspace_id,
                SftpRecord::Ready {
                    workspace: Arc::new(workspace),
                },
            );
            Ok(SftpWorkspaceSnapshot {
                id: workspace_id,
                connection_id,
                root: canonical_root,
                state: SftpWorkspaceState::Ready,
                host_key: None,
            })
        }
        Err(_) if factory.pending_host_key().is_some() => Ok(SftpWorkspaceSnapshot {
            id: workspace_id,
            connection_id,
            root,
            state: SftpWorkspaceState::HostKeyRequired,
            host_key: factory.pending_host_key().map(Into::into),
        }),
        Err(_) => {
            state
                .operations
                .sftp_workspaces
                .lock()
                .await
                .remove(&workspace_id);
            Err(public(
                ErrorCode::InvalidState,
                "SFTP workspace could not be opened",
            ))
        }
    }
}

#[tauri::command]
pub async fn close_sftp_workspace(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
) -> Result<(), PublicError> {
    authorize(&window)?;
    {
        let mut transfers = state.operations.transfers.lock().await;
        for record in transfers.values_mut().filter(|record| {
            matches!(
                &record.operation,
                TransferOperation::Upload { workspace_id: id, .. }
                    | TransferOperation::Download { workspace_id: id, .. }
                    if *id == workspace_id
            )
        }) {
            if !record.snapshot.status.is_terminal() {
                record.snapshot.status = TransferStatus::Cancelling;
                record.cancellation.cancel();
            }
        }
    }
    let record = state
        .operations
        .sftp_workspaces
        .lock()
        .await
        .remove(&workspace_id)
        .ok_or_else(|| public(ErrorCode::NotFound, "SFTP workspace not found"))?;
    if let SftpRecord::Ready { workspace, .. } = record {
        workspace.close().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn list_sftp_directory(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    path: String,
) -> Result<Vec<RemoteEntry>, PublicError> {
    authorize(&window)?;
    ready_sftp(&state, workspace_id)
        .await?
        .list(&path)
        .await
        .map_err(map_sftp)
}

#[tauri::command]
pub async fn create_sftp_directory(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    parent: String,
    name: String,
) -> Result<(), PublicError> {
    authorize(&window)?;
    ready_sftp(&state, workspace_id)
        .await?
        .create_dir(&parent, &name)
        .await
        .map_err(map_sftp)
}

#[tauri::command]
pub async fn rename_sftp_entry(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    path: String,
    name: String,
) -> Result<String, PublicError> {
    authorize(&window)?;
    ready_sftp(&state, workspace_id)
        .await?
        .rename(&path, &name)
        .await
        .map_err(map_sftp)
}

#[tauri::command]
pub async fn delete_sftp_entry(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    path: String,
) -> Result<(), PublicError> {
    authorize(&window)?;
    ready_sftp(&state, workspace_id)
        .await?
        .delete(&path)
        .await
        .map_err(map_sftp)
}

#[tauri::command]
pub async fn chmod_sftp_entry(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    path: String,
    mode: u32,
) -> Result<(), PublicError> {
    authorize(&window)?;
    ready_sftp(&state, workspace_id)
        .await?
        .chmod(&path, mode)
        .await
        .map_err(map_sftp)
}

#[tauri::command]
pub async fn choose_sftp_uploads(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    remote_directory: String,
    options: TransferOptions,
    on_event: Channel<TransferSnapshot>,
) -> Result<Vec<TransferSnapshot>, PublicError> {
    authorize(&window)?;
    let _ = ready_sftp(&state, workspace_id).await?;
    let selected = tokio::task::spawn_blocking(|| {
        tinyfiledialogs::open_file_dialog_multi("Select files to upload", "", None)
    })
    .await
    .map_err(|_| public(ErrorCode::Internal, "native file picker stopped"))?
    .unwrap_or_default();
    let operations = Arc::clone(&state.operations);
    queue_upload_paths(
        operations,
        workspace_id,
        remote_directory,
        options,
        selected.into_iter().map(PathBuf::from).collect(),
        Arc::new(on_event),
    )
    .await
}

#[tauri::command]
pub async fn queue_sftp_drop(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    batch_id: Uuid,
    remote_directory: String,
    options: TransferOptions,
    on_event: Channel<TransferSnapshot>,
) -> Result<Vec<TransferSnapshot>, PublicError> {
    authorize(&window)?;
    let _ = ready_sftp(&state, workspace_id).await?;
    let paths = state.operations.take_staged_uploads(batch_id).await?;
    queue_upload_paths(
        Arc::clone(&state.operations),
        workspace_id,
        remote_directory,
        options,
        paths,
        Arc::new(on_event),
    )
    .await
}

async fn queue_upload_paths(
    operations: Arc<DesktopOperations>,
    workspace_id: Uuid,
    remote_directory: String,
    options: TransferOptions,
    selected: Vec<PathBuf>,
    events: Arc<Channel<TransferSnapshot>>,
) -> Result<Vec<TransferSnapshot>, PublicError> {
    if selected.len() > MAX_UPLOAD_FILES {
        return Err(public(
            ErrorCode::ResourceExhausted,
            "upload selection exceeds 64 files",
        ));
    }
    if operations.transfers.lock().await.len() + selected.len() > MAX_TRANSFER_JOBS {
        return Err(public(
            ErrorCode::ResourceExhausted,
            "upload selection exceeds the remaining transfer queue capacity",
        ));
    }
    let mut queued = Vec::with_capacity(selected.len());
    for selected_path in selected {
        let metadata = tokio::fs::symlink_metadata(&selected_path)
            .await
            .map_err(|_| {
                public(
                    ErrorCode::InvalidInput,
                    "selected upload file is unavailable",
                )
            })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(public(
                ErrorCode::InvalidInput,
                "upload selection is not a regular file",
            ));
        }
        let local_path = tokio::fs::canonicalize(&selected_path).await.map_err(|_| {
            public(
                ErrorCode::InvalidInput,
                "selected upload file is unavailable",
            )
        })?;
        let name = local_path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| public(ErrorCode::InvalidInput, "selected upload path is invalid"))?
            .to_owned();
        let remote_path = join_remote_display(&remote_directory, &name)?;
        let snapshot = queue_transfer(
            &operations,
            TransferOperation::Upload {
                workspace_id,
                local_path,
                remote_path: remote_path.clone(),
                options,
            },
            TransferSnapshot {
                id: Uuid::new_v4(),
                workspace_id,
                generation: 0,
                direction: TransferDirection::Upload,
                name,
                remote_path,
                status: TransferStatus::Queued,
                transferred: 0,
                total: metadata.len(),
                resumed: false,
                sha256: None,
                error: None,
            },
        )
        .await?;
        queued.push(snapshot.clone());
        spawn_transfer(Arc::clone(&operations), snapshot.id, 0, Arc::clone(&events));
    }
    Ok(queued)
}

#[tauri::command]
pub async fn choose_sftp_download(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    workspace_id: Uuid,
    remote_path: String,
    options: TransferOptions,
    on_event: Channel<TransferSnapshot>,
) -> Result<Option<TransferSnapshot>, PublicError> {
    authorize(&window)?;
    let _ = ready_sftp(&state, workspace_id).await?;
    let name = remote_path
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty() && value.len() <= 255)
        .ok_or_else(|| public(ErrorCode::InvalidInput, "remote download path is invalid"))?
        .to_owned();
    let picker_name = name.clone();
    let selected = tokio::task::spawn_blocking(move || {
        tinyfiledialogs::save_file_dialog("Save downloaded file", &picker_name)
    })
    .await
    .map_err(|_| public(ErrorCode::Internal, "native save dialog stopped"))?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let operations = Arc::clone(&state.operations);
    let snapshot = queue_transfer(
        &operations,
        TransferOperation::Download {
            workspace_id,
            remote_path: remote_path.clone(),
            local_path: PathBuf::from(selected),
            options,
        },
        TransferSnapshot {
            id: Uuid::new_v4(),
            workspace_id,
            generation: 0,
            direction: TransferDirection::Download,
            name,
            remote_path,
            status: TransferStatus::Queued,
            transferred: 0,
            total: 0,
            resumed: false,
            sha256: None,
            error: None,
        },
    )
    .await?;
    spawn_transfer(operations, snapshot.id, 0, Arc::new(on_event));
    Ok(Some(snapshot))
}

#[tauri::command]
pub async fn list_transfers(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<TransferSnapshot>, PublicError> {
    authorize(&window)?;
    Ok(state
        .operations
        .transfers
        .lock()
        .await
        .values()
        .map(|record| record.snapshot.clone())
        .collect())
}

#[tauri::command]
pub async fn cancel_transfer(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    transfer_id: Uuid,
) -> Result<(), PublicError> {
    authorize(&window)?;
    let mut transfers = state.operations.transfers.lock().await;
    let record = transfers
        .get_mut(&transfer_id)
        .ok_or_else(|| public(ErrorCode::NotFound, "transfer not found"))?;
    cancel_transfer_record(record);
    Ok(())
}

fn cancel_transfer_record(record: &mut TransferRecord) {
    if record.snapshot.status == TransferStatus::Paused {
        record.snapshot.status = TransferStatus::Cancelled;
    } else if !record.snapshot.status.is_terminal() {
        record.stop_status = TransferStatus::Cancelled;
        record.snapshot.status = TransferStatus::Cancelling;
        record.cancellation.cancel();
    }
}

#[tauri::command]
pub async fn pause_transfer(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    transfer_id: Uuid,
) -> Result<(), PublicError> {
    authorize(&window)?;
    let mut transfers = state.operations.transfers.lock().await;
    let record = transfers
        .get_mut(&transfer_id)
        .ok_or_else(|| public(ErrorCode::NotFound, "transfer not found"))?;
    pause_transfer_record(record)
}

fn pause_transfer_record(record: &mut TransferRecord) -> Result<(), PublicError> {
    if !matches!(
        record.snapshot.status,
        TransferStatus::Queued | TransferStatus::Running
    ) {
        return Err(public(ErrorCode::InvalidState, "transfer is not pausable"));
    }
    record.stop_status = TransferStatus::Paused;
    record.snapshot.status = TransferStatus::Pausing;
    record.cancellation.cancel();
    Ok(())
}

#[tauri::command]
pub async fn retry_transfer(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    transfer_id: Uuid,
    on_event: Channel<TransferSnapshot>,
) -> Result<TransferSnapshot, PublicError> {
    authorize(&window)?;
    let (snapshot, generation) = {
        let mut transfers = state.operations.transfers.lock().await;
        let record = transfers
            .get_mut(&transfer_id)
            .ok_or_else(|| public(ErrorCode::NotFound, "transfer not found"))?;
        restart_transfer_record(record)?;
        (record.snapshot.clone(), record.generation)
    };
    spawn_transfer(
        Arc::clone(&state.operations),
        transfer_id,
        generation,
        Arc::new(on_event),
    );
    Ok(snapshot)
}

fn restart_transfer_record(record: &mut TransferRecord) -> Result<(), PublicError> {
    if !matches!(
        record.snapshot.status,
        TransferStatus::Failed | TransferStatus::Cancelled | TransferStatus::Paused
    ) {
        return Err(public(ErrorCode::InvalidState, "transfer is not retryable"));
    }
    record.generation = record.generation.saturating_add(1);
    record.snapshot.generation = record.generation;
    record.cancellation = CancellationToken::new();
    record.stop_status = TransferStatus::Cancelled;
    record.snapshot.status = TransferStatus::Queued;
    record.snapshot.transferred = 0;
    record.snapshot.error = None;
    Ok(())
}

#[tauri::command]
pub async fn clear_finished_transfers(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state
        .operations
        .transfers
        .lock()
        .await
        .retain(|_, record| !record.snapshot.status.is_terminal());
    Ok(())
}

#[tauri::command]
pub async fn start_tunnel(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
    spec: TunnelSpec,
) -> Result<DesktopTunnelSnapshot, PublicError> {
    authorize(&window)?;
    let factory = resolve_ssh_connection(&state, connection_id).await?.factory;
    let id = Uuid::new_v4();
    open_tunnel_record(&state, id, connection_id, spec, factory).await
}

#[tauri::command]
pub async fn approve_tunnel_host_key(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    tunnel_id: Uuid,
    fingerprint: String,
    previous_fingerprint: Option<String>,
) -> Result<DesktopTunnelSnapshot, PublicError> {
    authorize(&window)?;
    validate_fingerprints(&fingerprint, previous_fingerprint.as_deref())?;
    let record = state
        .operations
        .tunnels
        .lock()
        .await
        .get(&tunnel_id)
        .cloned()
        .ok_or_else(|| public(ErrorCode::NotFound, "tunnel not found"))?;
    let TunnelRecord::Pending {
        connection_id,
        spec,
        factory,
    } = record
    else {
        return Err(public(ErrorCode::InvalidState, "tunnel is already running"));
    };
    approve_factory(&factory, &fingerprint, previous_fingerprint.as_deref())?;
    open_tunnel_record(&state, tunnel_id, connection_id, spec, factory).await
}

#[tauri::command]
pub async fn list_tunnels(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<DesktopTunnelSnapshot>, PublicError> {
    authorize(&window)?;
    Ok(state
        .operations
        .tunnels
        .lock()
        .await
        .iter()
        .map(|(id, record)| tunnel_snapshot(*id, record))
        .collect())
}

#[tauri::command]
pub async fn close_tunnel(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    tunnel_id: Uuid,
) -> Result<(), PublicError> {
    authorize(&window)?;
    let record = state
        .operations
        .tunnels
        .lock()
        .await
        .remove(&tunnel_id)
        .ok_or_else(|| public(ErrorCode::NotFound, "tunnel not found"))?;
    if let TunnelRecord::Running { tunnel, .. } = record {
        tunnel.close().await;
    }
    Ok(())
}

async fn queue_transfer(
    operations: &DesktopOperations,
    operation: TransferOperation,
    snapshot: TransferSnapshot,
) -> Result<TransferSnapshot, PublicError> {
    let mut transfers = operations.transfers.lock().await;
    if transfers.len() >= MAX_TRANSFER_JOBS {
        return Err(public(
            ErrorCode::ResourceExhausted,
            "transfer queue reached its 256-job limit; clear finished jobs",
        ));
    }
    transfers.insert(
        snapshot.id,
        TransferRecord {
            snapshot: snapshot.clone(),
            operation,
            cancellation: CancellationToken::new(),
            stop_status: TransferStatus::Cancelled,
            generation: 0,
        },
    );
    Ok(snapshot)
}

fn spawn_transfer(
    operations: Arc<DesktopOperations>,
    id: Uuid,
    generation: u64,
    events: Arc<Channel<TransferSnapshot>>,
) {
    tokio::spawn(run_transfer(operations, id, generation, events));
}

async fn run_transfer(
    operations: Arc<DesktopOperations>,
    id: Uuid,
    generation: u64,
    events: Arc<Channel<TransferSnapshot>>,
) {
    let Some(record) = transfer_record(&operations, id, generation).await else {
        return;
    };
    let cancellation = record.cancellation.clone();
    let permit = tokio::select! {
        () = cancellation.cancelled() => {
            let stop_status = transfer_stop_status(&operations, id, generation).await;
            finish_transfer(&operations, id, generation, stop_status, None, &events).await;
            return;
        }
        permit = Arc::clone(&operations.transfer_permits).acquire_owned() => {
            let Ok(permit) = permit else { return };
            permit
        }
    };
    update_transfer(&operations, id, generation, &events, |snapshot| {
        snapshot.status = TransferStatus::Running;
    })
    .await;
    let workspace_id = match &record.operation {
        TransferOperation::Upload { workspace_id, .. }
        | TransferOperation::Download { workspace_id, .. } => *workspace_id,
    };
    let Some(workspace) = transfer_workspace(&operations, workspace_id).await else {
        drop(permit);
        finish_transfer(
            &operations,
            id,
            generation,
            TransferStatus::Failed,
            Some("SFTP workspace is unavailable".to_owned()),
            &events,
        )
        .await;
        return;
    };
    let (progress_tx, progress_task) =
        track_transfer_progress(Arc::clone(&operations), id, generation, Arc::clone(&events));
    let result = execute_transfer(&workspace, &record.operation, &progress_tx, &cancellation).await;
    drop(progress_tx);
    let _ = progress_task.await;
    drop(permit);
    finalize_transfer_result(&operations, id, generation, result, &events).await;
}

async fn transfer_workspace(
    operations: &DesktopOperations,
    workspace_id: Uuid,
) -> Option<Arc<SftpWorkspace>> {
    let workspaces = operations.sftp_workspaces.lock().await;
    if let Some(SftpRecord::Ready { workspace, .. }) = workspaces.get(&workspace_id) {
        Some(Arc::clone(workspace))
    } else {
        None
    }
}

fn track_transfer_progress(
    operations: Arc<DesktopOperations>,
    id: Uuid,
    generation: u64,
    events: Arc<Channel<TransferSnapshot>>,
) -> (mpsc::Sender<TransferProgress>, tokio::task::JoinHandle<()>) {
    let (progress_tx, mut progress_rx) = mpsc::channel::<TransferProgress>(32);
    let task = tokio::spawn(async move {
        while let Some(progress) = progress_rx.recv().await {
            update_transfer(&operations, id, generation, &events, |snapshot| {
                snapshot.transferred = progress.transferred;
                snapshot.total = progress.total;
            })
            .await;
        }
    });
    (progress_tx, task)
}

async fn execute_transfer(
    workspace: &SftpWorkspace,
    operation: &TransferOperation,
    progress: &mpsc::Sender<TransferProgress>,
    cancellation: &CancellationToken,
) -> Result<TransferReceipt, SftpError> {
    match operation {
        TransferOperation::Upload {
            local_path,
            remote_path,
            options,
            ..
        } => {
            workspace
                .upload(local_path, remote_path, *options, progress, cancellation)
                .await
        }
        TransferOperation::Download {
            remote_path,
            local_path,
            options,
            ..
        } => {
            workspace
                .download(remote_path, local_path, *options, progress, cancellation)
                .await
        }
    }
}

async fn finalize_transfer_result(
    operations: &DesktopOperations,
    id: Uuid,
    generation: u64,
    result: Result<TransferReceipt, SftpError>,
    events: &Channel<TransferSnapshot>,
) {
    match result {
        Ok(receipt) => complete_transfer(operations, id, generation, receipt, events).await,
        Err(SftpError::Cancelled) => {
            let stop_status = transfer_stop_status(operations, id, generation).await;
            finish_transfer(operations, id, generation, stop_status, None, events).await;
        }
        Err(_) => {
            finish_transfer(
                operations,
                id,
                generation,
                TransferStatus::Failed,
                Some("Transfer failed; the partial file was preserved when safe".to_owned()),
                events,
            )
            .await;
        }
    }
}

async fn transfer_stop_status(
    operations: &DesktopOperations,
    id: Uuid,
    generation: u64,
) -> TransferStatus {
    operations
        .transfers
        .lock()
        .await
        .get(&id)
        .filter(|record| record.generation == generation)
        .map_or(TransferStatus::Cancelled, |record| record.stop_status)
}

async fn transfer_record(
    operations: &DesktopOperations,
    id: Uuid,
    generation: u64,
) -> Option<TransferRecord> {
    operations
        .transfers
        .lock()
        .await
        .get(&id)
        .filter(|record| record.generation == generation)
        .cloned()
}

async fn update_transfer(
    operations: &DesktopOperations,
    id: Uuid,
    generation: u64,
    events: &Channel<TransferSnapshot>,
    update: impl FnOnce(&mut TransferSnapshot),
) {
    let snapshot = {
        let mut transfers = operations.transfers.lock().await;
        let Some(record) = transfers
            .get_mut(&id)
            .filter(|record| record.generation == generation)
        else {
            return;
        };
        update(&mut record.snapshot);
        record.snapshot.clone()
    };
    let _ = events.send(snapshot);
}

async fn complete_transfer(
    operations: &DesktopOperations,
    id: Uuid,
    generation: u64,
    receipt: TransferReceipt,
    events: &Channel<TransferSnapshot>,
) {
    update_transfer(operations, id, generation, events, |snapshot| {
        snapshot.status = TransferStatus::Completed;
        snapshot.transferred = receipt.bytes;
        snapshot.total = receipt.bytes;
        snapshot.resumed = receipt.resumed;
        snapshot.sha256 = receipt.sha256;
        snapshot.error = None;
    })
    .await;
}

async fn finish_transfer(
    operations: &DesktopOperations,
    id: Uuid,
    generation: u64,
    status: TransferStatus,
    error: Option<String>,
    events: &Channel<TransferSnapshot>,
) {
    update_transfer(operations, id, generation, events, |snapshot| {
        snapshot.status = status;
        snapshot.error = error;
    })
    .await;
}

fn join_remote_display(parent: &str, name: &str) -> Result<String, PublicError> {
    let invalid_name = name.is_empty()
        || name.len() > 255
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control);
    let invalid_parent = parent.len() > 4096
        || parent.contains('\\')
        || parent.chars().any(char::is_control)
        || parent
            .split('/')
            .any(|component| matches!(component, "." | ".."));
    if invalid_name || invalid_parent || parent.starts_with('/') {
        return Err(public(
            ErrorCode::InvalidInput,
            "remote transfer path is invalid",
        ));
    }
    Ok(if parent.trim_matches('/').is_empty() {
        name.to_owned()
    } else {
        format!("{}/{name}", parent.trim_matches('/'))
    })
}

async fn ready_sftp(state: &DesktopState, id: Uuid) -> Result<Arc<SftpWorkspace>, PublicError> {
    match state.operations.sftp_workspaces.lock().await.get(&id) {
        Some(SftpRecord::Ready { workspace, .. }) => Ok(Arc::clone(workspace)),
        Some(SftpRecord::Pending { .. }) => Err(public(
            ErrorCode::InvalidState,
            "SFTP host key approval is required",
        )),
        None => Err(public(ErrorCode::NotFound, "SFTP workspace not found")),
    }
}

async fn open_tunnel_record(
    state: &DesktopState,
    id: Uuid,
    connection_id: ConnectionId,
    spec: TunnelSpec,
    factory: Arc<SshSessionFactory>,
) -> Result<DesktopTunnelSnapshot, PublicError> {
    match factory.start_tunnel(spec.clone()).await {
        Ok(tunnel) => {
            let tunnel = Arc::new(tunnel);
            let snapshot = tunnel.snapshot().clone();
            state.operations.tunnels.lock().await.insert(
                id,
                TunnelRecord::Running {
                    connection_id,
                    tunnel,
                },
            );
            Ok(DesktopTunnelSnapshot {
                id,
                connection_id,
                state: TunnelState::Running,
                tunnel: Some(snapshot),
                host_key: None,
            })
        }
        Err(_) if factory.pending_host_key().is_some() => {
            let host_key = factory.pending_host_key().map(Into::into);
            state.operations.tunnels.lock().await.insert(
                id,
                TunnelRecord::Pending {
                    connection_id,
                    spec,
                    factory,
                },
            );
            Ok(DesktopTunnelSnapshot {
                id,
                connection_id,
                state: TunnelState::HostKeyRequired,
                tunnel: None,
                host_key,
            })
        }
        Err(_) => {
            state.operations.tunnels.lock().await.remove(&id);
            Err(public(
                ErrorCode::InvalidState,
                "SSH tunnel could not be started",
            ))
        }
    }
}

fn tunnel_snapshot(id: Uuid, record: &TunnelRecord) -> DesktopTunnelSnapshot {
    match record {
        TunnelRecord::Pending {
            connection_id,
            factory,
            ..
        } => DesktopTunnelSnapshot {
            id,
            connection_id: *connection_id,
            state: TunnelState::HostKeyRequired,
            tunnel: None,
            host_key: factory.pending_host_key().map(Into::into),
        },
        TunnelRecord::Running {
            connection_id,
            tunnel,
        } => DesktopTunnelSnapshot {
            id,
            connection_id: *connection_id,
            state: TunnelState::Running,
            tunnel: Some(tunnel.snapshot().clone()),
            host_key: None,
        },
    }
}

fn approve_factory(
    factory: &SshSessionFactory,
    fingerprint: &str,
    previous_fingerprint: Option<&str>,
) -> Result<(), PublicError> {
    if let Some(previous) = previous_fingerprint {
        factory
            .approve_replacement(previous, fingerprint)
            .map_err(|_| public(ErrorCode::Conflict, "host-key evidence changed"))
    } else {
        factory
            .approve_unknown(fingerprint)
            .map_err(|_| public(ErrorCode::Conflict, "host-key evidence changed"))
    }
}

fn validate_fingerprints(
    fingerprint: &str,
    previous_fingerprint: Option<&str>,
) -> Result<(), PublicError> {
    if fingerprint.is_empty()
        || fingerprint.len() > 256
        || previous_fingerprint.is_some_and(|value| value.len() > 256)
    {
        Err(public(
            ErrorCode::InvalidInput,
            "host-key fingerprint is invalid",
        ))
    } else {
        Ok(())
    }
}

fn map_sftp(_error: firstshell_ssh::SftpError) -> PublicError {
    public(ErrorCode::InvalidInput, "SFTP operation failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer_record_with_status(status: TransferStatus) -> TransferRecord {
        let workspace_id = Uuid::new_v4();
        TransferRecord {
            snapshot: TransferSnapshot {
                id: Uuid::new_v4(),
                workspace_id,
                generation: 0,
                direction: TransferDirection::Upload,
                name: "fixture.bin".to_owned(),
                remote_path: "fixture.bin".to_owned(),
                status,
                transferred: 1024,
                total: 2048,
                resumed: false,
                sha256: None,
                error: None,
            },
            operation: TransferOperation::Upload {
                workspace_id,
                local_path: PathBuf::from("fixture.bin"),
                remote_path: "fixture.bin".to_owned(),
                options: TransferOptions::default(),
            },
            cancellation: CancellationToken::new(),
            stop_status: TransferStatus::Cancelled,
            generation: 0,
        }
    }

    #[test]
    fn operation_limits_should_remain_bounded() {
        assert_eq!(MAX_CONCURRENT_TRANSFERS, 2);
        assert_eq!(MAX_TRANSFER_JOBS, 256);
    }

    #[tokio::test]
    async fn dropped_upload_paths_should_stay_rust_owned_and_be_consumed_once() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let local_path = directory.path().join("private-name.txt");
        std::fs::write(&local_path, b"drop payload").expect("write dropped fixture");
        let operations = DesktopOperations::new();
        let staged = stage_sftp_drop(&operations, vec![local_path.clone()]).await;
        let encoded = serde_json::to_string(&staged).expect("serialize safe drop metadata");

        assert!(!encoded.contains(&local_path.to_string_lossy().to_string()));
        assert!(encoded.contains("private-name.txt"));
        let batch_id = staged.batch_id.expect("accepted drop batch");
        let consumed = operations
            .take_staged_uploads(batch_id)
            .await
            .expect("first consume succeeds");
        assert_eq!(
            consumed,
            vec![std::fs::canonicalize(local_path).expect("canonical dropped fixture")]
        );
        assert!(operations.take_staged_uploads(batch_id).await.is_err());
    }

    #[tokio::test]
    async fn dropped_upload_should_reject_directories() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let operations = DesktopOperations::new();
        let staged = stage_sftp_drop(&operations, vec![directory.path().to_path_buf()]).await;

        assert!(staged.batch_id.is_none());
        assert!(staged.error.is_some());
        assert!(operations.staged_uploads.lock().await.is_empty());
    }

    #[test]
    fn pause_resume_and_cancel_should_have_distinct_states() {
        let mut record = transfer_record_with_status(TransferStatus::Running);
        pause_transfer_record(&mut record).expect("running transfer is pausable");
        assert_eq!(record.snapshot.status, TransferStatus::Pausing);
        assert_eq!(record.stop_status, TransferStatus::Paused);
        assert!(record.cancellation.is_cancelled());

        record.snapshot.status = record.stop_status;
        restart_transfer_record(&mut record).expect("paused transfer is resumable");
        assert_eq!(record.snapshot.status, TransferStatus::Queued);
        assert_eq!(record.snapshot.transferred, 0);
        assert_eq!(record.generation, 1);
        assert!(!record.cancellation.is_cancelled());

        record.snapshot.status = TransferStatus::Paused;
        cancel_transfer_record(&mut record);
        assert_eq!(record.snapshot.status, TransferStatus::Cancelled);
    }
}
