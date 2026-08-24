//! Bounded actors for remote and local interactive terminal sessions.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Default maximum number of pending interactive messages.
pub const INTERACTIVE_QUEUE_CAPACITY: usize = 256;
/// Default maximum number of pending bulk jobs.
pub const BULK_QUEUE_CAPACITY: usize = 32;
/// Default maximum number of simultaneously registered terminal sessions.
pub const DEFAULT_SESSION_LIMIT: usize = 32;
/// Maximum terminal input accepted in one IPC operation.
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
/// Maximum terminal dimensions accepted from a renderer.
pub const MAX_TERMINAL_CELLS: u16 = 2_000;

/// Stable opaque terminal-session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Creates a new unguessable session identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Public lifecycle state owned by a session actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// A transport is being established.
    Connecting,
    /// The transport is authenticating or starting its shell.
    Authenticating,
    /// Terminal input and resize requests are accepted.
    Ready,
    /// The transport ended and remains eligible for explicit reconnect.
    Disconnected,
    /// A user-requested reconnect is in progress.
    Reconnecting,
    /// The actor is shutting down.
    Closing,
    /// The actor has terminated permanently.
    Closed,
}

/// Validated terminal dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSize {
    /// Character columns.
    pub cols: u16,
    /// Character rows.
    pub rows: u16,
    /// Logical pixel width when known.
    pub pixel_width: u16,
    /// Logical pixel height when known.
    pub pixel_height: u16,
}

impl TerminalSize {
    /// Validates dimensions before they reach a PTY implementation.
    ///
    /// # Errors
    /// Returns [`SessionError::InvalidSize`] for zero or unreasonable dimensions.
    pub fn validate(self) -> Result<Self, SessionError> {
        if self.cols == 0
            || self.rows == 0
            || self.cols > MAX_TERMINAL_CELLS
            || self.rows > MAX_TERMINAL_CELLS
        {
            return Err(SessionError::InvalidSize);
        }
        Ok(self)
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 100,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// Redacted event emitted to a terminal renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    /// Lifecycle state changed.
    State {
        /// New public state.
        state: SessionState,
    },
    /// Raw terminal output bytes.
    Output {
        /// Bounded raw bytes from the terminal.
        data: Vec<u8>,
    },
    /// A bounded authentication banner safe for display.
    Banner {
        /// Sanitized banner text.
        text: String,
    },
    /// The transport reported an exit status.
    Exit {
        /// Process or remote exit status when supplied.
        code: Option<u32>,
    },
    /// A redacted diagnostic safe for display.
    Error {
        /// Stable machine-readable code.
        code: String,
        /// Redacted user-facing message.
        message: String,
    },
}

/// One command accepted by a connected terminal transport.
#[derive(Debug)]
pub enum TransportCommand {
    /// User input bytes. These are never logged by this crate.
    Input(Vec<u8>),
    /// Updated terminal dimensions.
    Resize(TerminalSize),
    /// Graceful shutdown request.
    Close,
}

/// Connected transport endpoints owned by a session actor.
pub struct ConnectedTransport {
    /// Commands sent toward the transport implementation.
    pub commands: mpsc::Sender<TransportCommand>,
    /// Events received from the transport implementation.
    pub events: mpsc::Receiver<SessionEvent>,
}

/// Factory used for initial connection and explicit reconnect.
#[async_trait]
pub trait SessionFactory: Send + Sync {
    /// Establishes and authenticates a fresh transport.
    ///
    /// # Errors
    /// Returns a redacted session failure without embedding credentials.
    async fn connect(&self, size: TerminalSize) -> Result<ConnectedTransport, SessionError>;
}

/// Sink receiving one session's public event stream.
pub trait SessionEventSink: Send + Sync {
    /// Delivers an event. Implementations must not block the actor.
    fn send(&self, session_id: SessionId, event: SessionEvent);
}

impl<F> SessionEventSink for F
where
    F: Fn(SessionId, SessionEvent) + Send + Sync,
{
    fn send(&self, session_id: SessionId, event: SessionEvent) {
        self(session_id, event);
    }
}

/// Public session metadata without transport internals or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    /// Opaque session identifier.
    pub id: SessionId,
    /// Current actor-owned lifecycle state.
    pub state: SessionState,
    /// Last accepted terminal dimensions.
    pub size: TerminalSize,
}

#[derive(Debug)]
enum ActorCommand {
    Input(Vec<u8>),
    Resize(TerminalSize),
    Reconnect,
    Close,
}

struct SessionEntry {
    commands: mpsc::Sender<ActorCommand>,
    snapshot: Arc<RwLock<SessionSnapshot>>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

/// Bounded registry and lifecycle owner for all terminal actors.
pub struct SessionRegistry {
    limit: usize,
    entries: AsyncMutex<HashMap<SessionId, SessionEntry>>,
}

impl SessionRegistry {
    /// Creates a registry with the production resource limit.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_SESSION_LIMIT)
    }

    /// Creates a registry with an explicit limit for tests or constrained devices.
    #[must_use]
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            entries: AsyncMutex::new(HashMap::new()),
        }
    }

    /// Starts a session actor and immediately returns its opaque identity.
    ///
    /// # Errors
    /// Returns [`SessionError::ResourceExhausted`] at the configured limit.
    pub async fn open(
        &self,
        factory: Arc<dyn SessionFactory>,
        size: TerminalSize,
        sink: Arc<dyn SessionEventSink>,
    ) -> Result<SessionSnapshot, SessionError> {
        let size = size.validate()?;
        let mut entries = self.entries.lock().await;
        entries.retain(|_, entry| !entry.task.is_finished());
        if entries.len() >= self.limit {
            return Err(SessionError::ResourceExhausted);
        }

        let id = SessionId::new();
        let snapshot = Arc::new(RwLock::new(SessionSnapshot {
            id,
            state: SessionState::Connecting,
            size,
        }));
        let (commands, receiver) = mpsc::channel(INTERACTIVE_QUEUE_CAPACITY);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_actor(
            id,
            Arc::clone(&factory),
            Arc::clone(&snapshot),
            Arc::clone(&sink),
            cancel.clone(),
            receiver,
        ));
        let public = snapshot.read().await.clone();
        entries.insert(
            id,
            SessionEntry {
                commands,
                snapshot,
                cancel,
                task,
            },
        );
        Ok(public)
    }

    /// Returns current metadata for every active or disconnected actor.
    pub async fn list(&self) -> Vec<SessionSnapshot> {
        let snapshots = {
            let entries = self.entries.lock().await;
            entries
                .values()
                .map(|entry| Arc::clone(&entry.snapshot))
                .collect::<Vec<_>>()
        };
        let mut result = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            result.push(snapshot.read().await.clone());
        }
        result
    }

    /// Sends bounded user input without waiting for queue capacity.
    ///
    /// # Errors
    /// Rejects oversized input, missing sessions, or saturated queues.
    pub async fn send_input(&self, id: SessionId, data: Vec<u8>) -> Result<(), SessionError> {
        if data.len() > MAX_INPUT_BYTES {
            return Err(SessionError::InputTooLarge);
        }
        self.try_send(id, ActorCommand::Input(data)).await
    }

    /// Updates terminal dimensions.
    ///
    /// # Errors
    /// Rejects invalid dimensions, missing sessions, or saturated queues.
    pub async fn resize(&self, id: SessionId, size: TerminalSize) -> Result<(), SessionError> {
        self.try_send(id, ActorCommand::Resize(size.validate()?))
            .await
    }

    /// Explicitly reconnects a disconnected session without replaying old input.
    ///
    /// # Errors
    /// Rejects missing sessions or saturated queues.
    pub async fn reconnect(&self, id: SessionId) -> Result<(), SessionError> {
        self.try_send(id, ActorCommand::Reconnect).await
    }

    /// Requests graceful closure and removes the public handle.
    ///
    /// # Errors
    /// Returns [`SessionError::NotFound`] for an unknown session.
    pub async fn close(&self, id: SessionId) -> Result<(), SessionError> {
        let entry = self
            .entries
            .lock()
            .await
            .remove(&id)
            .ok_or(SessionError::NotFound)?;
        if entry.commands.try_send(ActorCommand::Close).is_err() {
            entry.cancel.cancel();
        }
        Ok(())
    }

    /// Closes and removes every registered session, for example on vault lock.
    pub async fn close_all(&self) {
        let entries = {
            let mut guard = self.entries.lock().await;
            guard.drain().map(|(_, entry)| entry).collect::<Vec<_>>()
        };
        for entry in entries {
            if entry.commands.try_send(ActorCommand::Close).is_err() {
                entry.cancel.cancel();
            }
        }
    }

    async fn try_send(&self, id: SessionId, command: ActorCommand) -> Result<(), SessionError> {
        let entries = self.entries.lock().await;
        let entry = entries.get(&id).ok_or(SessionError::NotFound)?;
        entry
            .commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => SessionError::ResourceExhausted,
                mpsc::error::TrySendError::Closed(_) => SessionError::Closed,
            })
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SessionRegistry {
    fn drop(&mut self) {
        if let Ok(entries) = self.entries.try_lock() {
            for entry in entries.values() {
                entry.cancel.cancel();
            }
        }
    }
}

async fn run_actor(
    id: SessionId,
    factory: Arc<dyn SessionFactory>,
    snapshot: Arc<RwLock<SessionSnapshot>>,
    sink: Arc<dyn SessionEventSink>,
    cancel: CancellationToken,
    mut actor_commands: mpsc::Receiver<ActorCommand>,
) {
    let mut size = snapshot.read().await.size;
    let mut transport = connect_transport(
        id,
        factory.as_ref(),
        size,
        snapshot.as_ref(),
        sink.as_ref(),
        SessionState::Connecting,
    )
    .await;

    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            command = actor_commands.recv() => {
                match command {
                    Some(ActorCommand::Input(data)) => {
                        if let Some(connected) = transport.as_ref()
                            && connected.commands.try_send(TransportCommand::Input(data)).is_err()
                        {
                            emit_error(id, sink.as_ref(), "INPUT_BACKPRESSURE", "terminal input queue is busy");
                        }
                    }
                    Some(ActorCommand::Resize(next)) => {
                        size = next;
                        snapshot.write().await.size = next;
                        if let Some(connected) = transport.as_ref() {
                            let _ = connected.commands.try_send(TransportCommand::Resize(next));
                        }
                    }
                    Some(ActorCommand::Reconnect) if transport.is_none() => {
                        transport = connect_transport(
                            id,
                            factory.as_ref(),
                            size,
                            snapshot.as_ref(),
                            sink.as_ref(),
                            SessionState::Reconnecting,
                        ).await;
                    }
                    Some(ActorCommand::Close) | None => break,
                    Some(ActorCommand::Reconnect) => {}
                }
            }
            event = receive_transport_event(&mut transport), if transport.is_some() => {
                if let Some(event) = event {
                    let ended = matches!(event, SessionEvent::Exit { .. });
                    sink.send(id, event);
                    if ended {
                        transport = None;
                        set_state(id, snapshot.as_ref(), sink.as_ref(), SessionState::Disconnected).await;
                    }
                } else {
                    transport = None;
                    set_state(id, snapshot.as_ref(), sink.as_ref(), SessionState::Disconnected).await;
                }
            }
        }
    }

    if let Some(connected) = transport {
        let _ = connected.commands.send(TransportCommand::Close).await;
    }
    set_state(id, snapshot.as_ref(), sink.as_ref(), SessionState::Closed).await;
}

async fn connect_transport(
    id: SessionId,
    factory: &dyn SessionFactory,
    size: TerminalSize,
    snapshot: &RwLock<SessionSnapshot>,
    sink: &dyn SessionEventSink,
    state: SessionState,
) -> Option<ConnectedTransport> {
    set_state(id, snapshot, sink, state).await;
    set_state(id, snapshot, sink, SessionState::Authenticating).await;
    match tokio::time::timeout(Duration::from_secs(30), factory.connect(size)).await {
        Ok(Ok(transport)) => {
            set_state(id, snapshot, sink, SessionState::Ready).await;
            Some(transport)
        }
        Ok(Err(error)) => {
            emit_error(id, sink, error.code(), error.public_message());
            set_state(id, snapshot, sink, SessionState::Disconnected).await;
            None
        }
        Err(_) => {
            emit_error(id, sink, "CONNECT_TIMEOUT", "session connection timed out");
            set_state(id, snapshot, sink, SessionState::Disconnected).await;
            None
        }
    }
}

async fn receive_transport_event(
    transport: &mut Option<ConnectedTransport>,
) -> Option<SessionEvent> {
    transport.as_mut()?.events.recv().await
}

async fn set_state(
    id: SessionId,
    snapshot: &RwLock<SessionSnapshot>,
    sink: &dyn SessionEventSink,
    state: SessionState,
) {
    snapshot.write().await.state = state;
    sink.send(id, SessionEvent::State { state });
}

fn emit_error(id: SessionId, sink: &dyn SessionEventSink, code: &str, message: &str) {
    sink.send(
        id,
        SessionEvent::Error {
            code: code.to_owned(),
            message: message.to_owned(),
        },
    );
}

/// Session-layer failure with a stable public diagnostic.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Configured concurrent-session limit was reached.
    #[error("session resource limit reached")]
    ResourceExhausted,
    /// Terminal input exceeded the per-message boundary.
    #[error("terminal input is too large")]
    InputTooLarge,
    /// Terminal dimensions were invalid.
    #[error("terminal dimensions are invalid")]
    InvalidSize,
    /// Requested session does not exist.
    #[error("session not found")]
    NotFound,
    /// Session actor has already stopped.
    #[error("session is closed")]
    Closed,
    /// Transport could not connect or authenticate.
    #[error("{message}")]
    Transport {
        /// Stable transport-specific code.
        code: &'static str,
        /// Redacted message safe for display.
        message: String,
    },
    /// Local PTY failed without exposing command input.
    #[error("local terminal failed: {0}")]
    Local(String),
}

impl SessionError {
    /// Creates a redacted transport failure.
    #[must_use]
    pub fn transport(code: &'static str, message: impl Into<String>) -> Self {
        Self::Transport {
            code,
            message: message.into(),
        }
    }

    /// Returns the stable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ResourceExhausted => "RESOURCE_EXHAUSTED",
            Self::InputTooLarge => "INPUT_TOO_LARGE",
            Self::InvalidSize => "INVALID_SIZE",
            Self::NotFound => "NOT_FOUND",
            Self::Closed => "SESSION_CLOSED",
            Self::Transport { code, .. } => code,
            Self::Local(_) => "LOCAL_TERMINAL_FAILED",
        }
    }

    /// Returns a message safe for renderer display.
    #[must_use]
    pub fn public_message(&self) -> &str {
        match self {
            Self::Transport { message, .. } | Self::Local(message) => message,
            _ => self.code(),
        }
    }
}

/// Whitelisted local shell selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalShell {
    /// `PowerShell` 7 when installed.
    PowerShell,
    /// Windows `PowerShell`.
    WindowsPowerShell,
    /// Windows Command Prompt.
    CommandPrompt,
    /// POSIX Bourne-compatible shell.
    Bash,
    /// Z shell.
    Zsh,
    /// Platform OpenSSH client used only for a secret-free compatibility fallback.
    OpenSsh,
}

impl LocalShell {
    fn executable(self) -> &'static str {
        match self {
            Self::PowerShell => "pwsh.exe",
            Self::WindowsPowerShell => "powershell.exe",
            Self::CommandPrompt => "cmd.exe",
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::OpenSsh => {
                if cfg!(windows) {
                    "ssh.exe"
                } else {
                    "ssh"
                }
            }
        }
    }
}

/// Factory for a platform PTY with a fixed executable and controlled environment.
pub struct LocalTerminalFactory {
    shell: LocalShell,
    arguments: Vec<String>,
    working_directory: Option<PathBuf>,
}

impl LocalTerminalFactory {
    /// Creates a local terminal factory for one whitelisted shell.
    #[must_use]
    pub fn new(shell: LocalShell, working_directory: Option<PathBuf>) -> Self {
        Self {
            shell,
            arguments: Vec::new(),
            working_directory,
        }
    }

    /// Creates a platform OpenSSH PTY with validated, non-secret arguments.
    ///
    /// # Errors
    /// Rejects excessive arguments or embedded NUL bytes.
    pub fn openssh(arguments: Vec<String>) -> Result<Self, SessionError> {
        if arguments.len() > 32
            || arguments
                .iter()
                .any(|argument| argument.len() > 4096 || argument.contains('\0'))
        {
            return Err(SessionError::Local(
                "OpenSSH fallback arguments exceed limits".to_owned(),
            ));
        }
        Ok(Self {
            shell: LocalShell::OpenSsh,
            arguments,
            working_directory: None,
        })
    }
}

#[async_trait]
impl SessionFactory for LocalTerminalFactory {
    async fn connect(&self, size: TerminalSize) -> Result<ConnectedTransport, SessionError> {
        let shell = self.shell;
        let arguments = self.arguments.clone();
        let working_directory = self.working_directory.clone();
        tokio::task::spawn_blocking(move || {
            spawn_local_pty(shell, arguments, working_directory, size)
        })
        .await
        .map_err(|_| SessionError::Local("local terminal worker stopped".to_owned()))?
    }
}

fn spawn_local_pty(
    shell: LocalShell,
    arguments: Vec<String>,
    working_directory: Option<PathBuf>,
    size: TerminalSize,
) -> Result<ConnectedTransport, SessionError> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        })
        .map_err(|error| SessionError::Local(error.to_string()))?;
    let command = local_command(shell, arguments, working_directory);
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| SessionError::Local(error.to_string()))?;
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| SessionError::Local(error.to_string()))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| SessionError::Local(error.to_string()))?;
    let (command_tx, mut command_rx) = mpsc::channel(INTERACTIVE_QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel(INTERACTIVE_QUEUE_CAPACITY);

    let read_events = event_tx.clone();
    std::thread::Builder::new()
        .name("firstshell-local-pty-read".to_owned())
        .spawn(move || read_pty(reader, &read_events))
        .map_err(|error| SessionError::Local(error.to_string()))?;

    std::thread::Builder::new()
        .name("firstshell-local-pty-command".to_owned())
        .spawn(move || {
            while let Some(command) = command_rx.blocking_recv() {
                let keep_running = match command {
                    TransportCommand::Input(data) => {
                        match writer.write_all(&data).and_then(|()| writer.flush()) {
                            Ok(()) => true,
                            Err(error) => {
                                send_local_pty_error(&event_tx, "LOCAL_PTY_INPUT_FAILED", &error);
                                false
                            }
                        }
                    }
                    TransportCommand::Resize(size) => match pair.master.resize(PtySize {
                        rows: size.rows,
                        cols: size.cols,
                        pixel_width: size.pixel_width,
                        pixel_height: size.pixel_height,
                    }) {
                        Ok(()) => true,
                        Err(error) => {
                            send_local_pty_error(&event_tx, "LOCAL_PTY_RESIZE_FAILED", &error);
                            false
                        }
                    },
                    TransportCommand::Close => false,
                };
                if !keep_running {
                    break;
                }
            }
            let _ = child.kill();
            drop(writer);
            drop(pair.master);
            let _ = event_tx.blocking_send(SessionEvent::Exit { code: None });
        })
        .map_err(|error| SessionError::Local(error.to_string()))?;

    Ok(ConnectedTransport {
        commands: command_tx,
        events: event_rx,
    })
}

fn local_command(
    shell: LocalShell,
    arguments: Vec<String>,
    working_directory: Option<PathBuf>,
) -> CommandBuilder {
    let mut command = CommandBuilder::new(shell.executable());
    command.args(arguments);
    if let Some(directory) = working_directory {
        command.cwd(directory);
    }
    command.env_clear();
    for name in [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "TERM",
        "SSH_AUTH_SOCK",
        "COMSPEC",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PSModulePath",
        "USERNAME",
        "USERDOMAIN",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("TERM", "xterm-256color");
    command
}

fn read_pty(mut reader: Box<dyn Read + Send>, events: &mpsc::Sender<SessionEvent>) {
    let mut buffer = vec![0_u8; 32 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if events
                    .blocking_send(SessionEvent::Output {
                        data: buffer[..read].to_vec(),
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(error) => {
                send_local_pty_error(events, "LOCAL_PTY_READ_FAILED", &error);
                break;
            }
        }
    }
}

fn send_local_pty_error(
    events: &mpsc::Sender<SessionEvent>,
    code: &'static str,
    error: &dyn std::fmt::Display,
) {
    let _ = events.blocking_send(SessionEvent::Error {
        code: code.to_owned(),
        message: format!("local terminal operation failed: {error}"),
    });
}

/// Work submitted to one lane of a logical `SSH` session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneMessage {
    /// Opaque payload owned by the session implementation.
    pub payload: Vec<u8>,
}

/// Failure returned when a bounded lane cannot accept work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("session lane is at capacity")]
pub struct LaneFull;

/// Cloneable handles for latency-sensitive and bulk work queues.
#[derive(Debug, Clone)]
pub struct SessionLanes {
    interactive: mpsc::Sender<LaneMessage>,
    bulk: mpsc::Sender<LaneMessage>,
}

/// Receivers owned exclusively by the session actor.
#[derive(Debug)]
pub struct SessionLaneReceivers {
    /// Latency-sensitive terminal and control messages.
    pub interactive: mpsc::Receiver<LaneMessage>,
    /// Throughput-oriented `SFTP` and transfer messages.
    pub bulk: mpsc::Receiver<LaneMessage>,
}

impl SessionLanes {
    /// Creates the default bounded dual-lane scheduler.
    #[must_use]
    pub fn bounded() -> (Self, SessionLaneReceivers) {
        Self::with_capacity(INTERACTIVE_QUEUE_CAPACITY, BULK_QUEUE_CAPACITY)
    }

    /// Creates a scheduler with explicit queue capacities.
    #[must_use]
    pub fn with_capacity(interactive: usize, bulk: usize) -> (Self, SessionLaneReceivers) {
        let (interactive_tx, interactive_rx) = mpsc::channel(interactive);
        let (bulk_tx, bulk_rx) = mpsc::channel(bulk);
        (
            Self {
                interactive: interactive_tx,
                bulk: bulk_tx,
            },
            SessionLaneReceivers {
                interactive: interactive_rx,
                bulk: bulk_rx,
            },
        )
    }

    /// Enqueues terminal/control work without waiting for capacity.
    ///
    /// # Errors
    /// Returns [`LaneFull`] when the lane is full or closed.
    pub fn try_send_interactive(&self, message: LaneMessage) -> Result<(), LaneFull> {
        self.interactive.try_send(message).map_err(|_| LaneFull)
    }

    /// Enqueues `SFTP`/transfer work without waiting for capacity.
    ///
    /// # Errors
    /// Returns [`LaneFull`] when the lane is full or closed.
    pub fn try_send_bulk(&self, message: LaneMessage) -> Result<(), LaneFull> {
        self.bulk.try_send(message).map_err(|_| LaneFull)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct EchoFactory;

    struct ReconnectFactory {
        connections: Arc<Mutex<usize>>,
        received: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    #[async_trait]
    impl SessionFactory for EchoFactory {
        async fn connect(&self, _size: TerminalSize) -> Result<ConnectedTransport, SessionError> {
            let (commands, mut receiver) = mpsc::channel(4);
            let (events, output) = mpsc::channel(4);
            tokio::spawn(async move {
                while let Some(command) = receiver.recv().await {
                    match command {
                        TransportCommand::Input(data) => {
                            let _ = events.send(SessionEvent::Output { data }).await;
                        }
                        TransportCommand::Close => break,
                        TransportCommand::Resize(_) => {}
                    }
                }
            });
            Ok(ConnectedTransport {
                commands,
                events: output,
            })
        }
    }

    #[async_trait]
    impl SessionFactory for ReconnectFactory {
        async fn connect(&self, _size: TerminalSize) -> Result<ConnectedTransport, SessionError> {
            let connection_number = {
                let mut connections = self.connections.lock().expect("connection counter lock");
                *connections += 1;
                *connections
            };
            let (commands, mut receiver) = mpsc::channel(4);
            let (events, output) = mpsc::channel(4);
            if connection_number == 1 {
                drop(events);
            } else {
                let captured_input = Arc::clone(&self.received);
                tokio::spawn(async move {
                    let _events = events;
                    while let Some(command) = receiver.recv().await {
                        if let TransportCommand::Input(data) = command {
                            captured_input
                                .lock()
                                .expect("received input lock")
                                .push(data);
                        }
                    }
                });
            }
            Ok(ConnectedTransport {
                commands,
                events: output,
            })
        }
    }

    fn message() -> LaneMessage {
        LaneMessage { payload: vec![1] }
    }

    #[test]
    fn bulk_lane_should_apply_backpressure_at_capacity() {
        let (lanes, _receivers) = SessionLanes::with_capacity(1, 1);
        assert!(lanes.try_send_bulk(message()).is_ok());
        assert_eq!(lanes.try_send_bulk(message()), Err(LaneFull));
    }

    #[test]
    fn interactive_lane_should_remain_available_when_bulk_is_full() {
        let (lanes, _receivers) = SessionLanes::with_capacity(1, 1);
        assert!(lanes.try_send_bulk(message()).is_ok());
        assert!(lanes.try_send_interactive(message()).is_ok());
    }

    #[tokio::test]
    async fn registry_should_enforce_limit_and_drive_ten_sessions() {
        let registry = SessionRegistry::with_limit(10);
        let events = Arc::new(Mutex::new(Vec::new()));
        for index in 0..10_u8 {
            let captured = Arc::clone(&events);
            let snapshot = registry
                .open(
                    Arc::new(EchoFactory),
                    TerminalSize::default(),
                    Arc::new(move |id, event| {
                        captured
                            .lock()
                            .expect("event collection lock")
                            .push((id, event));
                    }),
                )
                .await
                .expect("session should open");
            registry
                .send_input(snapshot.id, vec![index])
                .await
                .expect("input should enqueue");
        }
        assert!(matches!(
            registry
                .open(
                    Arc::new(EchoFactory),
                    TerminalSize::default(),
                    Arc::new(|_, _| {}),
                )
                .await,
            Err(SessionError::ResourceExhausted)
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        let output_count = events
            .lock()
            .expect("event collection lock")
            .iter()
            .filter(|(_, event)| matches!(event, SessionEvent::Output { .. }))
            .count();
        assert_eq!(output_count, 10);
    }

    #[tokio::test]
    async fn invalid_input_and_dimensions_should_fail_before_transport() {
        let registry = SessionRegistry::new();
        assert!(matches!(
            registry
                .open(
                    Arc::new(EchoFactory),
                    TerminalSize {
                        cols: 0,
                        ..TerminalSize::default()
                    },
                    Arc::new(|_, _| {}),
                )
                .await,
            Err(SessionError::InvalidSize)
        ));
        let session = registry
            .open(
                Arc::new(EchoFactory),
                TerminalSize::default(),
                Arc::new(|_, _| {}),
            )
            .await
            .expect("valid session should open");
        assert!(matches!(
            registry
                .send_input(session.id, vec![0; MAX_INPUT_BYTES + 1])
                .await,
            Err(SessionError::InputTooLarge)
        ));
    }

    #[tokio::test]
    async fn explicit_reconnect_should_not_replay_disconnected_input() {
        let registry = SessionRegistry::new();
        let connections = Arc::new(Mutex::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let session = registry
            .open(
                Arc::new(ReconnectFactory {
                    connections: Arc::clone(&connections),
                    received: Arc::clone(&received),
                }),
                TerminalSize::default(),
                Arc::new(|_, _| {}),
            )
            .await
            .expect("session should open");
        tokio::time::sleep(Duration::from_millis(20)).await;
        registry
            .send_input(session.id, b"sensitive-before-reconnect".to_vec())
            .await
            .expect("disconnected input command should be consumed");
        registry
            .reconnect(session.id)
            .await
            .expect("explicit reconnect should enqueue");
        tokio::time::sleep(Duration::from_millis(20)).await;
        registry
            .send_input(session.id, b"after-reconnect".to_vec())
            .await
            .expect("new input should enqueue");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            received.lock().expect("received input lock").as_slice(),
            &[b"after-reconnect".to_vec()]
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_local_terminal_should_start_whitelisted_shell() {
        let factory = LocalTerminalFactory::new(LocalShell::CommandPrompt, None);
        let mut transport = factory
            .connect(TerminalSize::default())
            .await
            .expect("Command Prompt PTY should start");
        transport
            .commands
            .send(TransportCommand::Input(
                b"echo FIRSTSHELL_LOCAL_PTY\r\n".to_vec(),
            ))
            .await
            .expect("PTY input should send");
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        for _ in 0..10 {
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(500), transport.events.recv()).await
            {
                match event {
                    SessionEvent::Output { data } => {
                        let requests_cursor_position = data
                            .windows(b"\x1b[6n".len())
                            .any(|window| window == b"\x1b[6n");
                        output.extend_from_slice(&data);
                        if requests_cursor_position {
                            transport
                                .commands
                                .send(TransportCommand::Input(b"\x1b[1;1R".to_vec()))
                                .await
                                .expect("cursor-position response should send");
                        }
                    }
                    other => diagnostics.push(other),
                }
            }
            if String::from_utf8_lossy(&output).contains("FIRSTSHELL_LOCAL_PTY") {
                break;
            }
        }
        let _ = transport.commands.send(TransportCommand::Close).await;
        assert!(
            String::from_utf8_lossy(&output).contains("FIRSTSHELL_LOCAL_PTY"),
            "output={:?}, diagnostics={diagnostics:?}",
            String::from_utf8_lossy(&output)
        );
    }
}
