#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! `FirstShell` desktop adapter entrypoint.

mod commands;
mod ipc_benchmark;
mod session_commands;

use std::sync::Arc;
use std::time::Duration;

use commands::DesktopState;
use firstshell_application::{WorkspacePaths, WorkspaceService};
use firstshell_domain::{ErrorCode, PublicError};
use tauri::ipc::Channel;
use tauri::{Emitter, Manager, WebviewWindow};

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
        milestone: "M1",
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

fn main() {
    let result = tauri::Builder::default()
        .setup(|app| {
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
            app.manage(DesktopState {
                workspace: Arc::clone(&workspace),
                sessions: Arc::clone(&sessions),
            });

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                loop {
                    interval.tick().await;
                    if matches!(workspace.enforce_idle_lock(), Ok(true)) {
                        sessions.close_all().await;
                        let _ = app_handle.emit_to("main", "vault-locked", ());
                    }
                }
            });
            Ok(())
        })
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
            commands::search_workspace,
            commands::load_workspace_layout,
            commands::save_workspace_layout,
            commands::export_workspace,
            commands::validate_workspace_import,
            commands::restore_workspace_import,
            session_commands::configure_connection_credential,
            session_commands::connection_credential_status,
            session_commands::open_terminal_session,
            session_commands::open_quick_terminal,
            session_commands::list_terminal_sessions,
            session_commands::send_terminal_input,
            session_commands::resize_terminal,
            session_commands::reconnect_terminal,
            session_commands::close_terminal,
            session_commands::pending_host_key,
            session_commands::approve_host_key,
            session_commands::start_session_log,
            session_commands::stop_session_log,
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("FirstShell desktop runtime failed: {error}");
        std::process::exit(1);
    }
}
