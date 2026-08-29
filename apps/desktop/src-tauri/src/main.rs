#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! `FirstShell` desktop adapter entrypoint.

mod commands;
mod ipc_benchmark;
mod m4_commands;
mod rdp_commands;
mod session_commands;
mod transfer_commands;

use std::sync::Arc;
use std::time::Duration;

use commands::DesktopState;
use firstshell_application::{WorkspacePaths, WorkspaceService};
use firstshell_domain::{ErrorCode, PublicError};
use tauri::ipc::Channel;
use tauri::{DragDropEvent, Emitter, Manager, WebviewEvent, WebviewWindow};

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn application_info(window: WebviewWindow) -> Result<ApplicationInfo, PublicError> {
    if window.label() != "main" {
        return Err(PublicError::new(
            ErrorCode::InvalidState,
            "window is not authorized",
        ));
    }
    Ok(ApplicationInfo {
        name: "FirstShell",
        milestone: "M6",
    })
}

#[derive(serde::Serialize)]
struct ApplicationInfo {
    name: &'static str,
    milestone: &'static str,
}

#[tauri::command]
async fn run_ipc_benchmark(
    window: WebviewWindow,
    total_events: u32,
    payload_bytes: u32,
    on_event: Channel<ipc_benchmark::BenchmarkEvent>,
) -> Result<ipc_benchmark::BenchmarkSummary, PublicError> {
    if window.label() != "main" {
        return Err(PublicError::new(
            ErrorCode::InvalidState,
            "window is not authorized",
        ));
    }
    ipc_benchmark::run(total_events, payload_bytes, &on_event)
}

fn handle_webview_event(webview: &tauri::Webview, event: &WebviewEvent) {
    if webview.label() != "main" {
        return;
    }
    let WebviewEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event else {
        return;
    };
    let paths = paths.clone();
    let app_handle = webview.app_handle().clone();
    let operations = Arc::clone(&app_handle.state::<DesktopState>().operations);
    tauri::async_runtime::spawn(async move {
        let staged = transfer_commands::stage_sftp_drop(&operations, paths).await;
        let _ = app_handle.emit_to("main", "sftp-upload-drop", staged);
    });
}

fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let app_data = app.path().app_data_dir()?;
    let workspace = tauri::async_runtime::block_on(WorkspaceService::open_default(
        WorkspacePaths::under(&app_data),
        Duration::from_secs(15 * 60),
    ))?;
    let workspace = Arc::new(workspace);
    let sessions = Arc::new(session_commands::DesktopSessions::new(
        app_data.join("known_hosts"),
        app_data.join("session-logs"),
    ));
    let operations = Arc::new(transfer_commands::DesktopOperations::new());
    let tools = Arc::new(m4_commands::DesktopTools::new());
    let rdp = Arc::new(rdp_commands::DesktopRdp::new());
    app.manage(DesktopState {
        workspace: Arc::clone(&workspace),
        sessions: Arc::clone(&sessions),
        operations: Arc::clone(&operations),
        tools: Arc::clone(&tools),
        rdp: Arc::clone(&rdp),
    });

    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            if matches!(workspace.enforce_idle_lock(), Ok(true)) {
                sessions.close_all().await;
                operations.close_all().await;
                tools.close_all().await;
                rdp.close_all().await;
                let _ = app_handle.emit_to("main", "vault-locked", ());
            }
        }
    });
    Ok(())
}

fn main() {
    let result = tauri::Builder::default()
        .on_webview_event(handle_webview_event)
        .setup(setup_app)
        .invoke_handler(tauri::generate_handler![
            application_info,
            run_ipc_benchmark,
            commands::vault_status,
            commands::initialize_vault,
            commands::unlock_vault,
            commands::recover_vault,
            commands::lock_vault,
            commands::workspace_snapshot,
            commands::create_folder,
            commands::update_folder,
            commands::delete_folder,
            commands::restore_folder,
            commands::create_connection,
            commands::update_connection,
            commands::delete_connection,
            commands::restore_connection,
            commands::create_proxy,
            commands::update_proxy,
            commands::delete_proxy,
            commands::create_terminal_profile,
            commands::update_terminal_profile,
            commands::delete_terminal_profile,
            commands::create_snippet,
            commands::update_snippet,
            commands::delete_snippet,
            commands::render_snippet,
            commands::record_command_history,
            commands::list_command_history,
            commands::clear_command_history,
            commands::search_workspace,
            commands::load_workspace_layout,
            commands::save_workspace_layout,
            commands::export_workspace,
            commands::validate_workspace_import,
            commands::restore_workspace_import,
            session_commands::configure_connection_credential,
            session_commands::connection_credential_status,
            session_commands::configure_proxy_credential,
            session_commands::proxy_credential_status,
            session_commands::open_terminal_session,
            session_commands::open_quick_terminal,
            session_commands::list_terminal_sessions,
            session_commands::send_terminal_input,
            session_commands::broadcast_terminal_command,
            session_commands::stop_terminal_broadcast,
            session_commands::resize_terminal,
            session_commands::reconnect_terminal,
            session_commands::close_terminal,
            session_commands::pending_host_key,
            session_commands::approve_host_key,
            session_commands::start_session_log,
            session_commands::stop_session_log,
            rdp_commands::configure_rdp_credential,
            rdp_commands::rdp_credential_status,
            rdp_commands::open_rdp_session,
            rdp_commands::close_rdp_session,
            transfer_commands::open_sftp_workspace,
            transfer_commands::approve_sftp_host_key,
            transfer_commands::close_sftp_workspace,
            transfer_commands::list_sftp_directory,
            transfer_commands::create_sftp_directory,
            transfer_commands::rename_sftp_entry,
            transfer_commands::delete_sftp_entry,
            transfer_commands::chmod_sftp_entry,
            transfer_commands::open_remote_text,
            transfer_commands::save_remote_text,
            transfer_commands::create_remote_archive,
            transfer_commands::choose_sftp_uploads,
            transfer_commands::queue_sftp_drop,
            transfer_commands::choose_sftp_download,
            transfer_commands::choose_zmodem_upload,
            transfer_commands::choose_zmodem_download,
            transfer_commands::list_transfers,
            transfer_commands::pause_transfer,
            transfer_commands::cancel_transfer,
            transfer_commands::retry_transfer,
            transfer_commands::clear_finished_transfers,
            transfer_commands::start_tunnel,
            transfer_commands::approve_tunnel_host_key,
            transfer_commands::list_tunnels,
            transfer_commands::close_tunnel,
            m4_commands::start_host_monitor,
            m4_commands::approve_monitor_host_key,
            m4_commands::monitor_history,
            m4_commands::run_host_probe,
            m4_commands::stop_host_monitor,
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("FirstShell desktop runtime failed: {error}");
        std::process::exit(1);
    }
}
