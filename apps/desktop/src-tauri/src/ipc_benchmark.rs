//! Repeatable `Tauri` channel benchmark shared by the command and tests.

use std::time::Instant;

use firstshell_domain::{ErrorCode, PublicError};
use serde::Serialize;
use tauri::ipc::Channel;

const MAX_EVENTS: u32 = 10_000;
const MAX_PAYLOAD_BYTES: u32 = 64 * 1024;

/// Workload category carried by a benchmark event.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkEventKind {
    /// Simulated terminal output chunk.
    Terminal,
    /// Simulated aggregated monitoring sample.
    Monitor,
}

/// Event sent over the actual `Tauri` channel implementation.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkEvent {
    /// Zero-based event sequence.
    sequence: u32,
    /// Workload category.
    kind: BenchmarkEventKind,
    /// Fixed-size payload used to exercise serialization thresholds.
    payload: String,
}

/// Backend-side result for one channel run.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkSummary {
    /// Number of events emitted.
    sent: u32,
    /// Time spent serializing and dispatching channel events in Rust.
    backend_elapsed_micros: u128,
}

/// Sends a bounded terminal/monitor workload through a `Tauri` channel.
///
/// # Errors
/// Returns a public invalid-input error for an unsafe workload size, or a
/// redacted internal error if the channel closes during the run.
pub fn run(
    total_events: u32,
    payload_bytes: u32,
    on_event: &Channel<BenchmarkEvent>,
) -> Result<BenchmarkSummary, PublicError> {
    if total_events == 0 || total_events > MAX_EVENTS {
        return Err(PublicError::new(
            ErrorCode::InvalidInput,
            format!("total_events must be between 1 and {MAX_EVENTS}"),
        ));
    }
    if payload_bytes > MAX_PAYLOAD_BYTES {
        return Err(PublicError::new(
            ErrorCode::InvalidInput,
            format!("payload_bytes must not exceed {MAX_PAYLOAD_BYTES}"),
        ));
    }

    let payload = "x".repeat(payload_bytes as usize);
    let started = Instant::now();
    for sequence in 0..total_events {
        let kind = if sequence % 2 == 0 {
            BenchmarkEventKind::Terminal
        } else {
            BenchmarkEventKind::Monitor
        };
        on_event
            .send(BenchmarkEvent {
                sequence,
                kind,
                payload: payload.clone(),
            })
            .map_err(|_| PublicError::new(ErrorCode::Internal, "IPC benchmark channel closed"))?;
    }

    Ok(BenchmarkSummary {
        sent: total_events,
        backend_elapsed_micros: started.elapsed().as_micros(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tauri::ipc::InvokeResponseBody;

    use super::*;

    #[test]
    fn run_should_reject_an_unbounded_event_count() {
        let channel = Channel::new(|_: InvokeResponseBody| Ok(()));
        let error = run(MAX_EVENTS + 1, 1, &channel).expect_err("workload must be rejected");
        assert_eq!(error.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn run_should_dispatch_every_event_through_tauri_channel() {
        let received = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&received);
        let channel = Channel::new(move |_: InvokeResponseBody| {
            count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });

        let summary = run(100, 256, &channel).expect("bounded workload must run");

        assert_eq!(summary.sent as usize, received.load(Ordering::Relaxed));
    }

    #[test]
    #[ignore = "run explicitly in release mode on the M0 reference hardware"]
    fn ipc_channel_throughput_reference_benchmark() {
        let channel = Channel::new(|_: InvokeResponseBody| Ok(()));
        let summary = run(10_000, 1024, &channel).expect("reference workload must run");
        println!(
            "events={} payload_bytes=1024 backend_elapsed_micros={}",
            summary.sent, summary.backend_elapsed_micros
        );
    }
}
