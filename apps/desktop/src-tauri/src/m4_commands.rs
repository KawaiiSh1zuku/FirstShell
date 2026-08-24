//! M4 agentless monitoring and bounded efficiency-tool orchestration.

// Tauri commands deserialize owned arguments at the IPC boundary.
#![allow(clippy::needless_pass_by_value)]

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use firstshell_domain::{ConnectionId, ErrorCode, PublicError};
use firstshell_ssh::{ProbeKind, ProbeResult, ProbeSession, SshSessionFactory};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::commands::DesktopState;
use crate::session_commands::{DesktopHostKeyEvidence, public, resolve_ssh_connection};
use crate::transfer_commands::{approve_factory, validate_fingerprints};

const MONITOR_RING_CAPACITY: usize = 300;
const MAX_MONITORS: usize = 16;
const MIN_INTERVAL_SECONDS: u64 = 2;
const MAX_INTERVAL_SECONDS: u64 = 60;
const MAX_HISTORY_POINTS: usize = 300;

/// Desktop-owned monitor registries. Probe connections never enter the `WebView`.
pub struct DesktopTools {
    monitors: Mutex<HashMap<Uuid, MonitorRecord>>,
}

impl DesktopTools {
    /// Creates an empty bounded tools registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            monitors: Mutex::new(HashMap::new()),
        }
    }

    /// Cancels all probes before Vault key material is locked.
    pub async fn close_all(&self) {
        let monitors = std::mem::take(&mut *self.monitors.lock().await);
        for monitor in monitors.into_values() {
            if let MonitorRecord::Running { cancellation, .. } = monitor {
                cancellation.cancel();
            }
        }
    }
}

impl Default for DesktopTools {
    fn default() -> Self {
        Self::new()
    }
}

enum MonitorRecord {
    Pending {
        connection_id: ConnectionId,
        interval_seconds: u64,
        factory: Arc<SshSessionFactory>,
        events: Arc<Channel<MonitorEvent>>,
    },
    Running {
        cancellation: CancellationToken,
        shared: Arc<MonitorShared>,
    },
}

struct MonitorShared {
    history: Mutex<VecDeque<ProbeResult>>,
    state: Mutex<MonitorState>,
    diagnostic: Mutex<Option<String>>,
}

/// Public lifecycle state of one periodic monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorState {
    /// Host identity must be explicitly approved.
    HostKeyRequired,
    /// Periodic sampling is active.
    Running,
    /// Three consecutive bounded probes failed.
    Degraded,
}

/// Public monitor registry snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    id: Uuid,
    connection_id: ConnectionId,
    interval_seconds: u64,
    state: MonitorState,
    sample_count: usize,
    diagnostic: Option<String>,
    host_key: Option<DesktopHostKeyEvidence>,
}

/// One periodic monitor event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorEvent {
    monitor_id: Uuid,
    state: MonitorState,
    sample: Option<ProbeResult>,
    diagnostic: Option<String>,
}

fn authorize(window: &WebviewWindow) -> Result<(), PublicError> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(public(
            ErrorCode::InvalidState,
            "window is not authorized for host monitoring",
        ))
    }
}

/// Starts a bounded periodic host-metrics monitor.
#[tauri::command]
pub async fn start_host_monitor(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
    interval_seconds: u64,
    on_event: Channel<MonitorEvent>,
) -> Result<MonitorSnapshot, PublicError> {
    authorize(&window)?;
    if !(MIN_INTERVAL_SECONDS..=MAX_INTERVAL_SECONDS).contains(&interval_seconds) {
        return Err(public(
            ErrorCode::InvalidInput,
            "monitor interval must be between 2 and 60 seconds",
        ));
    }
    let factory = resolve_ssh_connection(&state, connection_id).await?.factory;
    let id = Uuid::new_v4();
    let events = Arc::new(on_event);
    if state.tools.monitors.lock().await.len() >= MAX_MONITORS {
        return Err(public(
            ErrorCode::ResourceExhausted,
            "host monitor limit reached",
        ));
    }
    match factory.open_probe_session().await {
        Ok(session) => {
            start_ready_monitor(
                &state.tools,
                id,
                connection_id,
                interval_seconds,
                session,
                events,
            )
            .await
        }
        Err(_) if factory.pending_host_key().is_some() => {
            let host_key = factory.pending_host_key().map(Into::into);
            state.tools.monitors.lock().await.insert(
                id,
                MonitorRecord::Pending {
                    connection_id,
                    interval_seconds,
                    factory,
                    events,
                },
            );
            Ok(MonitorSnapshot {
                id,
                connection_id,
                interval_seconds,
                state: MonitorState::HostKeyRequired,
                sample_count: 0,
                diagnostic: None,
                host_key,
            })
        }
        Err(_) => Err(public(
            ErrorCode::InvalidState,
            "agentless monitor could not connect",
        )),
    }
}

/// Approves pending monitor host-key evidence and starts sampling.
#[tauri::command]
pub async fn approve_monitor_host_key(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    monitor_id: Uuid,
    fingerprint: String,
    previous_fingerprint: Option<String>,
) -> Result<MonitorSnapshot, PublicError> {
    authorize(&window)?;
    validate_fingerprints(&fingerprint, previous_fingerprint.as_deref())?;
    let record = state
        .tools
        .monitors
        .lock()
        .await
        .remove(&monitor_id)
        .ok_or_else(|| public(ErrorCode::NotFound, "host monitor not found"))?;
    let MonitorRecord::Pending {
        connection_id,
        interval_seconds,
        factory,
        events,
    } = record
    else {
        return Err(public(
            ErrorCode::InvalidState,
            "host monitor is already active",
        ));
    };
    approve_factory(&factory, &fingerprint, previous_fingerprint.as_deref())?;
    let session = factory.open_probe_session().await.map_err(|_| {
        public(
            ErrorCode::InvalidState,
            "agentless monitor could not connect after approval",
        )
    })?;
    start_ready_monitor(
        &state.tools,
        monitor_id,
        connection_id,
        interval_seconds,
        session,
        events,
    )
    .await
}

/// Runs a one-shot process, port, ping, traceroute, or host-metrics probe.
#[tauri::command]
pub async fn run_host_probe(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection_id: ConnectionId,
    probe: ProbeKind,
) -> Result<ProbeResult, PublicError> {
    authorize(&window)?;
    resolve_ssh_connection(&state, connection_id)
        .await?
        .factory
        .run_probe(probe)
        .await
        .map_err(|_| public(ErrorCode::InvalidState, "agentless probe failed"))
}

/// Returns evenly downsampled history without copying the full ring into the `WebView`.
#[tauri::command]
pub async fn monitor_history(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    monitor_id: Uuid,
    max_points: usize,
) -> Result<Vec<ProbeResult>, PublicError> {
    authorize(&window)?;
    if !(1..=MAX_HISTORY_POINTS).contains(&max_points) {
        return Err(public(
            ErrorCode::InvalidInput,
            "monitor history point limit is invalid",
        ));
    }
    let shared = {
        let monitors = state.tools.monitors.lock().await;
        match monitors.get(&monitor_id) {
            Some(MonitorRecord::Running { shared, .. }) => Arc::clone(shared),
            Some(MonitorRecord::Pending { .. }) => return Ok(Vec::new()),
            None => return Err(public(ErrorCode::NotFound, "host monitor not found")),
        }
    };
    let history = shared.history.lock().await;
    Ok(downsample(&history, max_points))
}

/// Stops one monitor and closes its dedicated low-priority SSH connection.
#[tauri::command]
pub async fn stop_host_monitor(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    monitor_id: Uuid,
) -> Result<(), PublicError> {
    authorize(&window)?;
    let record = state
        .tools
        .monitors
        .lock()
        .await
        .remove(&monitor_id)
        .ok_or_else(|| public(ErrorCode::NotFound, "host monitor not found"))?;
    if let MonitorRecord::Running { cancellation, .. } = record {
        cancellation.cancel();
    }
    Ok(())
}

async fn start_ready_monitor(
    tools: &DesktopTools,
    id: Uuid,
    connection_id: ConnectionId,
    interval_seconds: u64,
    session: ProbeSession,
    events: Arc<Channel<MonitorEvent>>,
) -> Result<MonitorSnapshot, PublicError> {
    let first = session
        .run(ProbeKind::HostMetrics)
        .await
        .map_err(|_| public(ErrorCode::InvalidState, "initial host probe failed"))?;
    let mut history = VecDeque::with_capacity(MONITOR_RING_CAPACITY);
    history.push_back(first.clone());
    let shared = Arc::new(MonitorShared {
        history: Mutex::new(history),
        state: Mutex::new(MonitorState::Running),
        diagnostic: Mutex::new(first.diagnostic.clone()),
    });
    let cancellation = CancellationToken::new();
    tools.monitors.lock().await.insert(
        id,
        MonitorRecord::Running {
            cancellation: cancellation.clone(),
            shared: Arc::clone(&shared),
        },
    );
    let _ = events.send(MonitorEvent {
        monitor_id: id,
        state: MonitorState::Running,
        sample: Some(first),
        diagnostic: None,
    });
    tokio::spawn(run_monitor_loop(
        id,
        interval_seconds,
        session,
        cancellation,
        Arc::clone(&shared),
        events,
    ));
    Ok(MonitorSnapshot {
        id,
        connection_id,
        interval_seconds,
        state: MonitorState::Running,
        sample_count: 1,
        diagnostic: shared.diagnostic.lock().await.clone(),
        host_key: None,
    })
}

async fn run_monitor_loop(
    id: Uuid,
    interval_seconds: u64,
    session: ProbeSession,
    cancellation: CancellationToken,
    shared: Arc<MonitorShared>,
    events: Arc<Channel<MonitorEvent>>,
) {
    let mut timer = tokio::time::interval(Duration::from_secs(interval_seconds));
    timer.tick().await;
    let mut consecutive_failures = 0_u8;
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            _ = timer.tick() => {
                if let Ok(sample) = session.run(ProbeKind::HostMetrics).await {
                    consecutive_failures = 0;
                    *shared.state.lock().await = MonitorState::Running;
                    (*shared.diagnostic.lock().await).clone_from(&sample.diagnostic);
                    let mut history = shared.history.lock().await;
                    push_sample(&mut history, sample.clone());
                    drop(history);
                    let _ = events.send(MonitorEvent { monitor_id: id, state: MonitorState::Running, sample: Some(sample), diagnostic: None });
                } else {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    let diagnostic = "PROBE_FAILED".to_owned();
                    *shared.diagnostic.lock().await = Some(diagnostic.clone());
                    let event_state = if consecutive_failures >= 3 { MonitorState::Degraded } else { MonitorState::Running };
                    let _ = events.send(MonitorEvent { monitor_id: id, state: event_state, sample: None, diagnostic: Some(diagnostic) });
                    if consecutive_failures >= 3 {
                        *shared.state.lock().await = MonitorState::Degraded;
                    }
                }
            }
        }
    }
    session.close().await;
}

fn push_sample(history: &mut VecDeque<ProbeResult>, sample: ProbeResult) {
    if history.len() == MONITOR_RING_CAPACITY {
        history.pop_front();
    }
    history.push_back(sample);
}

fn downsample(history: &VecDeque<ProbeResult>, max_points: usize) -> Vec<ProbeResult> {
    if history.len() <= max_points {
        return history.iter().cloned().collect();
    }
    if max_points == 1 {
        return history.back().cloned().into_iter().collect();
    }
    let last = history.len() - 1;
    (0..max_points)
        .filter_map(|index| {
            let source = index.saturating_mul(last) / (max_points - 1);
            history.get(source).cloned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(sequence: u64) -> ProbeResult {
        ProbeResult {
            version: 1,
            probe: ProbeKind::HostMetrics,
            captured_at_ms: sequence,
            exit_code: Some(0),
            metrics: Vec::new(),
            rows: Vec::new(),
            diagnostic: None,
        }
    }

    #[test]
    fn downsampling_keeps_endpoints_and_requested_bound() {
        let history = (0..300).map(sample).collect::<VecDeque<_>>();
        let reduced = downsample(&history, 30);
        assert_eq!(reduced.len(), 30);
        assert_eq!(reduced.first().map(|value| value.captured_at_ms), Some(0));
        assert_eq!(reduced.last().map(|value| value.captured_at_ms), Some(299));
    }

    #[test]
    fn ring_should_remain_bounded_after_long_running_sampling() {
        let mut ring = VecDeque::new();
        for index in 0..100_000 {
            push_sample(&mut ring, sample(index));
        }
        assert_eq!(ring.len(), MONITOR_RING_CAPACITY);
        assert_eq!(ring.front().map(|value| value.captured_at_ms), Some(99_700));
        assert_eq!(ring.back().map(|value| value.captured_at_ms), Some(99_999));
    }
}
