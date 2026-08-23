#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! `FirstShell` desktop adapter entrypoint.

mod ipc_benchmark;

use firstshell_domain::PublicError;
use tauri::ipc::Channel;

#[tauri::command]
fn application_info() -> ApplicationInfo {
    ApplicationInfo {
        name: "FirstShell",
        milestone: "M0",
    }
}

#[derive(serde::Serialize)]
struct ApplicationInfo {
    name: &'static str,
    milestone: &'static str,
}

#[tauri::command]
async fn run_ipc_benchmark(
    total_events: u32,
    payload_bytes: u32,
    on_event: Channel<ipc_benchmark::BenchmarkEvent>,
) -> Result<ipc_benchmark::BenchmarkSummary, PublicError> {
    ipc_benchmark::run(total_events, payload_bytes, &on_event)
}

fn main() {
    let result = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            application_info,
            run_ipc_benchmark
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("FirstShell desktop runtime failed: {error}");
        std::process::exit(1);
    }
}
