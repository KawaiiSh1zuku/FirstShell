//! Typed desktop commands with explicit window authorization.

// Tauri commands deserialize owned arguments at the IPC boundary.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use firstshell_application::{
    ExportReceipt, ImportSummary, RestoreReceipt, VaultStatus, WorkspaceService,
};
use firstshell_domain::{
    ConnectionId, ConnectionInput, ConnectionProfile, ErrorCode, Folder, FolderId, FolderInput,
    ProxyId, ProxyInput, ProxyProfile, PublicError, SearchResult, TerminalProfile,
    TerminalProfileId, TerminalProfileInput, WorkspaceLayout, WorkspaceSnapshot,
};
use tauri::{State, WebviewWindow};

/// Managed application services shared by authorized desktop commands.
pub struct DesktopState {
    /// Encrypted workspace use cases.
    pub workspace: Arc<WorkspaceService>,
    /// Terminal session runtime and native credential boundary.
    pub sessions: Arc<crate::session_commands::DesktopSessions>,
}

fn authorize_window_label(label: &str) -> Result<(), PublicError> {
    if label == "main" {
        Ok(())
    } else {
        Err(PublicError::new(
            ErrorCode::InvalidState,
            "window is not authorized for workspace access",
        ))
    }
}

fn authorize(window: &WebviewWindow) -> Result<(), PublicError> {
    authorize_window_label(window.label())
}

#[tauri::command]
pub fn vault_status(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<VaultStatus, PublicError> {
    authorize(&window)?;
    state.workspace.vault_status()
}

#[tauri::command]
pub async fn initialize_vault(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    recovery_password: String,
) -> Result<WorkspaceSnapshot, PublicError> {
    authorize(&window)?;
    state.workspace.initialize(recovery_password).await
}

#[tauri::command]
pub async fn unlock_vault(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<WorkspaceSnapshot, PublicError> {
    authorize(&window)?;
    state.workspace.unlock().await
}

#[tauri::command]
pub async fn recover_vault(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    recovery_password: String,
) -> Result<WorkspaceSnapshot, PublicError> {
    authorize(&window)?;
    state.workspace.recover(recovery_password).await
}

#[tauri::command]
pub async fn lock_vault(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state.sessions.close_all().await;
    state.workspace.lock()
}

#[tauri::command]
pub async fn workspace_snapshot(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<WorkspaceSnapshot, PublicError> {
    authorize(&window)?;
    state.workspace.snapshot().await
}

#[tauri::command]
pub async fn create_folder(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    input: FolderInput,
) -> Result<Folder, PublicError> {
    authorize(&window)?;
    state.workspace.create_folder(input).await
}

#[tauri::command]
pub async fn update_folder(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: FolderId,
    expected_revision: u64,
    input: FolderInput,
) -> Result<Folder, PublicError> {
    authorize(&window)?;
    state
        .workspace
        .update_folder(id, expected_revision, input)
        .await
}

#[tauri::command]
pub async fn delete_folder(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: FolderId,
    expected_revision: u64,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state.workspace.delete_folder(id, expected_revision).await
}

#[tauri::command]
pub async fn restore_folder(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: FolderId,
    expected_revision: u64,
) -> Result<Folder, PublicError> {
    authorize(&window)?;
    state.workspace.restore_folder(id, expected_revision).await
}

#[tauri::command]
pub async fn create_connection(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    input: ConnectionInput,
) -> Result<ConnectionProfile, PublicError> {
    authorize(&window)?;
    state.workspace.create_connection(input).await
}

#[tauri::command]
pub async fn update_connection(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: ConnectionId,
    expected_revision: u64,
    input: ConnectionInput,
) -> Result<ConnectionProfile, PublicError> {
    authorize(&window)?;
    state
        .workspace
        .update_connection(id, expected_revision, input)
        .await
}

#[tauri::command]
pub async fn delete_connection(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: ConnectionId,
    expected_revision: u64,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state
        .workspace
        .delete_connection(id, expected_revision)
        .await
}

#[tauri::command]
pub async fn restore_connection(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: ConnectionId,
    expected_revision: u64,
) -> Result<ConnectionProfile, PublicError> {
    authorize(&window)?;
    state
        .workspace
        .restore_connection(id, expected_revision)
        .await
}

#[tauri::command]
pub async fn create_proxy(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    input: ProxyInput,
) -> Result<ProxyProfile, PublicError> {
    authorize(&window)?;
    state.workspace.create_proxy(input).await
}

#[tauri::command]
pub async fn update_proxy(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: ProxyId,
    expected_revision: u64,
    input: ProxyInput,
) -> Result<ProxyProfile, PublicError> {
    authorize(&window)?;
    state
        .workspace
        .update_proxy(id, expected_revision, input)
        .await
}

#[tauri::command]
pub async fn delete_proxy(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: ProxyId,
    expected_revision: u64,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state.workspace.delete_proxy(id, expected_revision).await
}

#[tauri::command]
pub async fn create_terminal_profile(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    input: TerminalProfileInput,
) -> Result<TerminalProfile, PublicError> {
    authorize(&window)?;
    state.workspace.create_terminal_profile(input).await
}

#[tauri::command]
pub async fn update_terminal_profile(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: TerminalProfileId,
    expected_revision: u64,
    input: TerminalProfileInput,
) -> Result<TerminalProfile, PublicError> {
    authorize(&window)?;
    state
        .workspace
        .update_terminal_profile(id, expected_revision, input)
        .await
}

#[tauri::command]
pub async fn delete_terminal_profile(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    id: TerminalProfileId,
    expected_revision: u64,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state
        .workspace
        .delete_terminal_profile(id, expected_revision)
        .await
}

#[tauri::command]
pub fn search_workspace(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    query: String,
    limit: usize,
) -> Result<Vec<SearchResult>, PublicError> {
    authorize(&window)?;
    state.workspace.search(&query, limit)
}

#[tauri::command]
pub async fn load_workspace_layout(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<WorkspaceLayout, PublicError> {
    authorize(&window)?;
    state.workspace.load_layout().await
}

#[tauri::command]
pub async fn save_workspace_layout(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    layout: WorkspaceLayout,
) -> Result<(), PublicError> {
    authorize(&window)?;
    state.workspace.save_layout(layout).await
}

#[tauri::command]
pub async fn export_workspace(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    password: String,
) -> Result<ExportReceipt, PublicError> {
    authorize(&window)?;
    state.workspace.export_workspace(password).await
}

#[tauri::command]
pub async fn validate_workspace_import(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    file_name: String,
    password: String,
) -> Result<ImportSummary, PublicError> {
    authorize(&window)?;
    state.workspace.validate_import(&file_name, password).await
}

#[tauri::command]
pub async fn restore_workspace_import(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    file_name: String,
    password: String,
) -> Result<RestoreReceipt, PublicError> {
    authorize(&window)?;
    state.workspace.restore_import(&file_name, password).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_primary_window_should_receive_workspace_commands() {
        assert!(authorize_window_label("main").is_ok());
        assert!(authorize_window_label("settings").is_err());
        assert!(authorize_window_label("").is_err());
    }
}
