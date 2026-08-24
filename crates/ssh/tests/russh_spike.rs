//! End-to-end M0 russh terminal and SFTP compatibility spike.

use std::collections::HashMap;
use std::collections::HashSet;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use firstshell_session::{
    ConnectedTransport, SessionEvent, SessionFactory, TerminalSize, TransportCommand,
};
use firstshell_ssh::{
    ConflictPolicy, JumpHostSpec, ProxyRoute, RemoteEntryKind, SftpError, SftpWorkspace,
    SshAuthentication, SshConnectSpec, SshSessionFactory, TerminalEncoding, TransferOptions,
    TunnelKind, TunnelSpec, VerifyMode,
};
use russh::client;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, PrivateKey, PublicKeyOrCertificate};
use russh::server::{Auth, Msg, Response, Server as _, Session};
use russh::{Channel, ChannelId, ChannelMsg, Disconnect, Pty};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct SpikeServer {
    fixture: Arc<SpikeFixture>,
}

impl russh::server::Server for SpikeServer {
    type Handler = SpikeSession;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        SpikeSession {
            channels: Arc::new(Mutex::new(HashMap::new())),
            forwards: Arc::new(Mutex::new(HashMap::new())),
            fixture: Arc::clone(&self.fixture),
        }
    }
}

struct SpikeSession {
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    forwards: Arc<Mutex<HashMap<u32, CancellationToken>>>,
    fixture: Arc<SpikeFixture>,
}

impl SpikeSession {
    async fn take_channel(&self, channel_id: ChannelId) -> Result<Channel<Msg>> {
        self.channels
            .lock()
            .await
            .remove(&channel_id)
            .context("SFTP channel was not registered")
    }
}

impl russh::server::Handler for SpikeSession {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth> {
        Ok(if user == "spike" && password == "m0-password" {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            }
        })
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        _public_key: &russh::keys::PublicKey,
    ) -> Result<Auth> {
        Ok(if user == "spike" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth> {
        if user != "spike" {
            return Ok(Auth::reject());
        }
        let Some(mut response) = response else {
            return Ok(Auth::Partial {
                name: "M2 verification".into(),
                instructions: "Enter the one-time response".into(),
                prompts: vec![("Response: ".into(), false)].into(),
            });
        };
        Ok(if response.next().as_deref() == Some(b"m2-response") {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn authentication_banner(&mut self) -> Result<Option<String>> {
        if !self.fixture.show_banner.load(Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(format!(
            "Authorized use only\u{1b}[31m{}",
            "x".repeat(9_000)
        )))
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<()> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<()> {
        let Ok(port) = u16::try_from(port_to_connect) else {
            return Ok(());
        };
        let Ok(mut target) = TcpStream::connect((host_to_connect, port)).await else {
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let mut forwarded = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut forwarded, &mut target).await;
            let _ = forwarded.shutdown().await;
        });
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool> {
        let Ok(requested_port) = u16::try_from(*port) else {
            return Ok(false);
        };
        let listener = TcpListener::bind((address, requested_port)).await?;
        let local = listener.local_addr()?;
        *port = u32::from(local.port());
        let bound_port = local.port();
        let bound_address = address.to_owned();
        let handle = session.handle();
        let cancellation = CancellationToken::new();
        self.forwards
            .lock()
            .await
            .insert(u32::from(bound_port), cancellation.clone());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancellation.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((mut stream, peer)) = accepted else { break };
                        let Ok(channel) = handle.channel_open_forwarded_tcpip(
                            bound_address.clone(),
                            u32::from(bound_port),
                            peer.ip().to_string(),
                            u32::from(peer.port()),
                        ).await else { continue };
                        tokio::spawn(async move {
                            let mut forwarded = channel.into_stream();
                            let _ = tokio::io::copy_bidirectional(&mut stream, &mut forwarded).await;
                            let _ = forwarded.shutdown().await;
                        });
                    }
                }
            }
        });
        Ok(true)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        _address: &str,
        port: u32,
        _session: &mut Session,
    ) -> Result<bool> {
        if let Some(cancellation) = self.forwards.lock().await.remove(&port) {
            cancellation.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<()> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<()> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<()> {
        if self.channels.lock().await.contains_key(&channel) {
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<()> {
        if name != "sftp" {
            session.channel_failure(channel)?;
            return Ok(());
        }

        let sftp_channel = self.take_channel(channel).await?;
        session.channel_success(channel)?;
        let fixture = Arc::clone(&self.fixture);
        tokio::spawn(async move {
            russh_sftp::server::run(sftp_channel.into_stream(), SpikeSftp::new(fixture)).await;
        });
        Ok(())
    }
}

struct SpikeFixture {
    root: tempfile::TempDir,
    permissions: Mutex<HashMap<String, u32>>,
    symlinks: Mutex<HashMap<String, String>>,
    show_banner: AtomicBool,
}

impl SpikeFixture {
    fn new() -> Result<Self> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("probe.txt"), b"sftp-probe")?;
        Ok(Self {
            root,
            permissions: Mutex::new(HashMap::new()),
            symlinks: Mutex::new(HashMap::new()),
            show_banner: AtomicBool::new(true),
        })
    }

    fn local_path(&self, remote: &str) -> Result<(String, PathBuf), StatusCode> {
        let normalized = normalize_remote(remote)?;
        let local = self.root.path().join(normalized.trim_start_matches('/'));
        Ok((normalized, local))
    }

    fn remote_path(&self, local: &Path) -> Result<String, StatusCode> {
        let canonical_root =
            std::fs::canonicalize(self.root.path()).map_err(|error| io_status(&error))?;
        let relative = local
            .strip_prefix(canonical_root)
            .map_err(|_| StatusCode::PermissionDenied)?;
        let components = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>();
        Ok(if components.is_empty() {
            "/".to_owned()
        } else {
            format!("/{}", components.join("/"))
        })
    }
}

struct SpikeSftp {
    fixture: Arc<SpikeFixture>,
    read_directories: HashSet<String>,
}

impl SpikeSftp {
    fn new(fixture: Arc<SpikeFixture>) -> Self {
        Self {
            fixture,
            read_directories: HashSet::new(),
        }
    }

    async fn attrs(&self, remote: &str, follow: bool) -> Result<FileAttributes, StatusCode> {
        let (normalized, local) = self.fixture.local_path(remote)?;
        if let Some(target) = self.fixture.symlinks.lock().await.get(&normalized) {
            if follow {
                return Err(StatusCode::PermissionDenied);
            }
            let mut attrs = FileAttributes {
                size: Some(target.len() as u64),
                permissions: Some(0o777),
                ..FileAttributes::default()
            };
            attrs.set_symlink(true);
            return Ok(attrs);
        }
        let metadata = if follow {
            std::fs::metadata(local)
        } else {
            std::fs::symlink_metadata(local)
        }
        .map_err(|error| io_status(&error))?;
        let mut attrs = FileAttributes::from(&metadata);
        if let Some(mode) = self.fixture.permissions.lock().await.get(&normalized) {
            attrs.permissions = Some((attrs.permissions.unwrap_or_default() & !0o7777) | mode);
        }
        Ok(attrs)
    }
}

impl russh_sftp::server::Handler for SpikeSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let (_, local) = self.fixture.local_path(&path)?;
        let canonical = std::fs::canonicalize(local).map_err(|error| io_status(&error))?;
        let remote = self.fixture.remote_path(&canonical)?;
        Ok(Name {
            id,
            files: vec![File::dummy(remote)],
        })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let (normalized, local) = self.fixture.local_path(&filename)?;
        if self.fixture.symlinks.lock().await.contains_key(&normalized) {
            return Err(StatusCode::PermissionDenied);
        }
        let options: std::fs::OpenOptions = pflags.into();
        options.open(local).map_err(|error| io_status(&error))?;
        Ok(Handle {
            id,
            handle: normalized,
        })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let (_, local) = self.fixture.local_path(&handle)?;
        let mut file = std::fs::File::open(local).map_err(|error| io_status(&error))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| io_status(&error))?;
        let mut data = vec![0_u8; len as usize];
        let read = file.read(&mut data).map_err(|error| io_status(&error))?;
        if read == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(read);
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let (_, local) = self.fixture.local_path(&handle)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(local)
            .map_err(|error| io_status(&error))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| io_status(&error))?;
        file.write_all(&data).map_err(|error| io_status(&error))?;
        file.sync_data().map_err(|error| io_status(&error))?;
        Ok(status_ok(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        Ok(Attrs {
            id,
            attrs: self.attrs(&path, false).await?,
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        Ok(Attrs {
            id,
            attrs: self.attrs(&handle, true).await?,
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        Ok(Attrs {
            id,
            attrs: self.attrs(&path, true).await?,
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let (normalized, local) = self.fixture.local_path(&path)?;
        std::fs::metadata(local).map_err(|error| io_status(&error))?;
        if let Some(mode) = attrs.permissions {
            self.fixture
                .permissions
                .lock()
                .await
                .insert(normalized, mode & 0o7777);
        }
        Ok(status_ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        self.setstat(id, handle, attrs).await
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let normalized = normalize_remote(&path)?;
        if !self.attrs(&normalized, true).await?.is_dir() {
            return Err(StatusCode::Failure);
        }
        self.read_directories.remove(&normalized);
        Ok(Handle {
            id,
            handle: normalized,
        })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if !self.read_directories.insert(handle.clone()) {
            return Err(StatusCode::Eof);
        }
        let (_, local) = self.fixture.local_path(&handle)?;
        let mut files = Vec::new();
        for entry in std::fs::read_dir(local).map_err(|error| io_status(&error))? {
            let entry = entry.map_err(|error| io_status(&error))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let remote = join_remote_test(&handle, &name);
            files.push(File::new(name, self.attrs(&remote, false).await?));
        }
        for (remote, target) in self.fixture.symlinks.lock().await.iter() {
            if remote_parent(remote) == handle {
                let name = remote.rsplit('/').next().ok_or(StatusCode::BadMessage)?;
                let mut attrs = FileAttributes {
                    size: Some(target.len() as u64),
                    permissions: Some(0o777),
                    ..FileAttributes::default()
                };
                attrs.set_symlink(true);
                files.push(File::new(name, attrs));
            }
        }
        if files.is_empty() {
            Err(StatusCode::Eof)
        } else {
            Ok(Name { id, files })
        }
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(status_ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let (normalized, local) = self.fixture.local_path(&filename)?;
        if self
            .fixture
            .symlinks
            .lock()
            .await
            .remove(&normalized)
            .is_none()
        {
            std::fs::remove_file(local).map_err(|error| io_status(&error))?;
        }
        self.fixture.permissions.lock().await.remove(&normalized);
        Ok(status_ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let (_, local) = self.fixture.local_path(&path)?;
        std::fs::create_dir(local).map_err(|error| io_status(&error))?;
        Ok(status_ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let (_, local) = self.fixture.local_path(&path)?;
        std::fs::remove_dir(local).map_err(|error| io_status(&error))?;
        Ok(status_ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let (old_normalized, old_local) = self.fixture.local_path(&oldpath)?;
        let (new_normalized, new_local) = self.fixture.local_path(&newpath)?;
        std::fs::rename(old_local, new_local).map_err(|error| io_status(&error))?;
        if let Some(mode) = self
            .fixture
            .permissions
            .lock()
            .await
            .remove(&old_normalized)
        {
            self.fixture
                .permissions
                .lock()
                .await
                .insert(new_normalized, mode);
        }
        Ok(status_ok(id))
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let normalized = normalize_remote(&path)?;
        let target = self
            .fixture
            .symlinks
            .lock()
            .await
            .get(&normalized)
            .cloned()
            .ok_or(StatusCode::NoSuchFile)?;
        Ok(Name {
            id,
            files: vec![File::dummy(target)],
        })
    }
}

fn normalize_remote(path: &str) -> Result<String, StatusCode> {
    if path.contains('\0') || path.contains('\\') {
        return Err(StatusCode::BadMessage);
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(StatusCode::PermissionDenied),
            value => components.push(value),
        }
    }
    Ok(if components.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", components.join("/"))
    })
}

fn join_remote_test(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn remote_parent(path: &str) -> String {
    path.rsplit_once('/').map_or_else(
        || "/".to_owned(),
        |(parent, _)| {
            if parent.is_empty() {
                "/".to_owned()
            } else {
                parent.to_owned()
            }
        },
    )
}

fn io_status(error: &std::io::Error) -> StatusCode {
    match error.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn status_ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: "en-US".to_owned(),
    }
}

struct SpikeClient {
    host_key_checked: Arc<AtomicBool>,
}

impl client::Handler for SpikeClient {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, _: &PublicKeyOrCertificate) -> Result<bool> {
        self.host_key_checked.store(true, Ordering::SeqCst);
        Ok(true)
    }
}

async fn exercise_client(addr: SocketAddr, host_key_checked: Arc<AtomicBool>) -> Result<()> {
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(5)),
        ..client::Config::default()
    });
    let mut client = client::connect(
        config,
        addr,
        SpikeClient {
            host_key_checked: host_key_checked.clone(),
        },
    )
    .await?;

    let auth = client.authenticate_password("spike", "m0-password").await?;
    ensure!(auth.success(), "password authentication was rejected");
    ensure!(
        host_key_checked.load(Ordering::SeqCst),
        "host key callback was not invoked"
    );

    let mut terminal = client.channel_open_session().await?;
    terminal
        .request_pty(true, "xterm-256color", 100, 30, 0, 0, &[])
        .await?;
    terminal.request_shell(true).await?;
    terminal.data(&b"terminal-probe"[..]).await?;

    let echoed = loop {
        let message = terminal
            .wait()
            .await
            .context("terminal channel closed before echo")?;
        if let ChannelMsg::Data { data } = message {
            break data;
        }
    };
    ensure!(
        echoed.as_ref() == b"terminal-probe",
        "terminal echo did not match input"
    );

    let sftp_channel = client.channel_open_session().await?;
    sftp_channel.request_subsystem(true, "sftp").await?;
    let sftp = SftpSession::new(sftp_channel.into_stream()).await?;
    ensure!(
        sftp.canonicalize(".").await? == "/",
        "unexpected SFTP canonical path"
    );
    let entries = sftp.read_dir("/").await?;
    ensure!(
        entries
            .into_iter()
            .any(|entry| entry.file_name() == "probe.txt"),
        "SFTP probe file was not listed"
    );

    client
        .disconnect(Disconnect::ByApplication, "M0 complete", "en-US")
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn russh_should_support_authenticated_terminal_and_sftp_channels() -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let config = Arc::new(russh::server::Config {
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![key],
        ..russh::server::Config::default()
    });

    let mut server = SpikeServer {
        fixture: Arc::new(SpikeFixture::new()?),
    };
    let running = server.run_on_socket(config, &listener);
    let shutdown = running.handle();
    let checked = Arc::new(AtomicBool::new(false));
    let client = async {
        let result =
            tokio::time::timeout(Duration::from_secs(10), exercise_client(addr, checked)).await;
        shutdown.shutdown("M0 spike complete".to_owned());
        result.context("SSH spike timed out")?
    };

    let (server_result, client_result) = tokio::join!(running, client);
    server_result?;
    client_result?;
    Ok(())
}

async fn start_server(key: PrivateKey) -> Result<(SocketAddr, russh::server::RunningServerHandle)> {
    let (address, handle, _) = start_server_with_fixture(key).await?;
    Ok((address, handle))
}

async fn start_server_with_fixture(
    key: PrivateKey,
) -> Result<(
    SocketAddr,
    russh::server::RunningServerHandle,
    Arc<SpikeFixture>,
)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let fixture = Arc::new(SpikeFixture::new()?);
    let config = Arc::new(russh::server::Config {
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![key],
        ..russh::server::Config::default()
    });
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    let server_fixture = Arc::clone(&fixture);
    tokio::spawn(async move {
        let mut server = SpikeServer {
            fixture: server_fixture,
        };
        let running = server.run_on_socket(config, &listener);
        let _ = handle_tx.send(running.handle());
        let _ = running.await;
    });
    Ok((
        addr,
        handle_rx.await.context("server handle unavailable")?,
        fixture,
    ))
}

fn m2_factory(
    addr: SocketAddr,
    known_hosts: &std::path::Path,
    password: &str,
) -> SshSessionFactory {
    m2_factory_with_auth(
        addr,
        known_hosts,
        SshAuthentication::Password(password.to_owned().into()),
    )
}

fn m2_factory_with_auth(
    addr: SocketAddr,
    known_hosts: &std::path::Path,
    authentication: SshAuthentication,
) -> SshSessionFactory {
    SshSessionFactory::new(SshConnectSpec {
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "spike".to_owned(),
        authentication,
        known_hosts: known_hosts.to_path_buf(),
        encoding: TerminalEncoding::Utf8,
        allow_legacy_algorithms: false,
        route: ProxyRoute::Direct,
        jump_hosts: Vec::new(),
    })
}

async fn approve_unknown_host(factory: &SshSessionFactory) -> Result<()> {
    ensure!(
        factory.connect(TerminalSize::default()).await.is_err(),
        "first connection must require TOFU approval"
    );
    let evidence = factory
        .pending_host_key()
        .context("pending TOFU evidence")?;
    factory.approve_unknown(&evidence.fingerprint)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m2_transport_should_require_tofu_then_stream_a_bounded_terminal() -> Result<()> {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let (addr, server) = start_server(key).await?;
    let directory = tempfile::tempdir()?;
    let factory = m2_factory(addr, &directory.path().join("known_hosts"), "m0-password");

    let first = factory.connect(TerminalSize::default()).await;
    ensure!(
        matches!(
            first,
            Err(firstshell_session::SessionError::Transport {
                code: "HOST_KEY_UNKNOWN",
                ..
            })
        ),
        "first connection must require explicit trust"
    );
    let evidence = factory
        .pending_host_key()
        .context("pending TOFU evidence")?;
    ensure!(
        !evidence.changed,
        "first key must not be classified as changed"
    );
    factory.approve_unknown(&evidence.fingerprint)?;

    let mut transport = factory.connect(TerminalSize::default()).await?;
    transport
        .commands
        .send(TransportCommand::Input(b"m2-terminal".to_vec()))
        .await?;
    let mut saw_bounded_banner = false;
    let mut saw_echo = false;
    for _ in 0..8 {
        let event = tokio::time::timeout(Duration::from_secs(2), transport.events.recv())
            .await?
            .context("transport event stream ended")?;
        match event {
            SessionEvent::Banner { text } => {
                ensure!(!text.contains('\u{1b}'), "banner controls must be removed");
                ensure!(text.chars().count() <= 8 * 1024, "banner must be bounded");
                saw_bounded_banner = true;
            }
            SessionEvent::Output { data } if data == b"m2-terminal" => saw_echo = true,
            _ => {}
        }
        if saw_bounded_banner && saw_echo {
            break;
        }
    }
    ensure!(
        saw_bounded_banner,
        "sanitized banner event was not received"
    );
    ensure!(saw_echo, "terminal input was not echoed");
    server.shutdown("M2 transport complete".to_owned());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m2_transport_should_fail_closed_for_changed_key_and_bad_password() -> Result<()> {
    let old_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let new_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let (addr, server) = start_server(new_key).await?;
    let directory = tempfile::tempdir()?;
    let known_hosts = directory.path().join("known_hosts");
    russh::keys::known_hosts::learn_known_hosts_path(
        &addr.ip().to_string(),
        addr.port(),
        old_key.public_key(),
        &known_hosts,
    )?;
    let changed = m2_factory(addr, &known_hosts, "m0-password");
    ensure!(
        matches!(
            changed.connect(TerminalSize::default()).await,
            Err(firstshell_session::SessionError::Transport {
                code: "HOST_KEY_CHANGED",
                ..
            })
        ),
        "changed host key must be rejected"
    );
    let changed_evidence = changed.pending_host_key().context("changed evidence")?;
    ensure!(changed_evidence.changed, "change flag must be visible");

    std::fs::remove_file(&known_hosts)?;
    let bad_password = m2_factory(addr, &known_hosts, "wrong-password");
    let unknown = bad_password.connect(TerminalSize::default()).await;
    ensure!(unknown.is_err(), "TOFU must run before authentication");
    let evidence = bad_password
        .pending_host_key()
        .context("unknown evidence")?;
    bad_password.approve_unknown(&evidence.fingerprint)?;
    ensure!(
        matches!(
            bad_password.connect(TerminalSize::default()).await,
            Err(firstshell_session::SessionError::Transport {
                code: "AUTHENTICATION_FAILED",
                ..
            })
        ),
        "bad password must return a redacted authentication failure"
    );
    server.shutdown("M2 failure tests complete".to_owned());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m2_transport_should_authenticate_with_private_key_and_keyboard_interactive() -> Result<()>
{
    let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let (addr, server) = start_server(host_key).await?;
    let directory = tempfile::tempdir()?;

    let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let private_key_path = directory.path().join("id_ed25519");
    client_key.write_openssh_file(&private_key_path, LineEnding::default())?;
    let private_key = m2_factory_with_auth(
        addr,
        &directory.path().join("known_hosts_private_key"),
        SshAuthentication::PrivateKey {
            path: private_key_path,
            passphrase: None,
        },
    );
    approve_unknown_host(&private_key).await?;
    let private_key_transport = private_key.connect(TerminalSize::default()).await?;
    private_key_transport
        .commands
        .send(TransportCommand::Close)
        .await?;

    let keyboard_interactive = m2_factory_with_auth(
        addr,
        &directory.path().join("known_hosts_keyboard_interactive"),
        SshAuthentication::KeyboardInteractive("m2-response".to_owned().into()),
    );
    approve_unknown_host(&keyboard_interactive).await?;
    let keyboard_transport = keyboard_interactive
        .connect(TerminalSize::default())
        .await?;
    keyboard_transport
        .commands
        .send(TransportCommand::Close)
        .await?;

    server.shutdown("M2 authentication methods complete".to_owned());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn m3_jump_chain_should_verify_and_authenticate_each_hop() -> Result<()> {
    let (destination, destination_server) =
        start_server(PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?).await?;
    let (jump, jump_server) =
        start_server(PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?).await?;
    let directory = tempfile::tempdir()?;
    let factory = SshSessionFactory::new(SshConnectSpec {
        host: destination.ip().to_string(),
        port: destination.port(),
        username: "spike".to_owned(),
        authentication: SshAuthentication::Password("m0-password".to_owned().into()),
        known_hosts: directory.path().join("known_hosts"),
        encoding: TerminalEncoding::Utf8,
        allow_legacy_algorithms: false,
        route: ProxyRoute::Direct,
        jump_hosts: vec![JumpHostSpec {
            host: jump.ip().to_string(),
            port: jump.port(),
            username: "spike".to_owned(),
            authentication: SshAuthentication::Password("m0-password".to_owned().into()),
            allow_legacy_algorithms: false,
        }],
    });

    for expected in [jump, destination] {
        let attempt = factory.connect(TerminalSize::default()).await;
        ensure!(attempt.is_err(), "each untrusted hop must fail closed");
        let evidence = factory.pending_host_key().context("jump TOFU evidence")?;
        ensure!(
            evidence.host == expected.ip().to_string() && evidence.port == expected.port(),
            "host-key evidence must identify the current hop"
        );
        factory.approve_unknown(&evidence.fingerprint)?;
    }

    let mut transport = factory.connect(TerminalSize::default()).await?;
    transport
        .commands
        .send(TransportCommand::Input(b"jump-chain".to_vec()))
        .await?;
    let echoed = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = transport.events.recv().await {
            if matches!(event, SessionEvent::Output { ref data } if data == b"jump-chain") {
                return true;
            }
        }
        false
    })
    .await?;
    ensure!(echoed, "terminal data did not cross the jump host");
    transport.commands.send(TransportCommand::Close).await?;
    jump_server.shutdown("M3 jump test complete".to_owned());
    destination_server.shutdown("M3 jump test complete".to_owned());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn m3_sftp_workspace_should_manage_files_resume_and_reject_symlinks() -> Result<()> {
    let (ssh, server, fixture) =
        start_server_with_fixture(PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?)
            .await?;
    let local = tempfile::tempdir()?;
    let factory = m2_factory(ssh, &local.path().join("known_hosts"), "m0-password");
    approve_unknown_host(&factory).await?;
    let workspace = Arc::new(factory.open_sftp(".").await?);

    verify_sftp_safety_and_metadata(&workspace, &fixture, local.path()).await?;
    verify_resumable_transfers(Arc::clone(&workspace), local.path()).await?;
    workspace.delete("ops/escape-link").await?;
    workspace.delete("ops/renamed.bin").await?;
    workspace.delete("ops/large.bin").await?;
    workspace.delete("ops").await?;
    workspace.close().await;
    server.shutdown("M3 SFTP product flow complete".to_owned());
    Ok(())
}

async fn verify_sftp_safety_and_metadata(
    workspace: &SftpWorkspace,
    fixture: &SpikeFixture,
    local: &Path,
) -> Result<()> {
    workspace.create_dir("", "ops").await?;
    fixture
        .symlinks
        .lock()
        .await
        .insert("/ops/escape-link".to_owned(), "../../outside".to_owned());
    let entries = workspace.list("ops").await?;
    ensure!(
        entries.iter().any(|entry| {
            entry.name == "escape-link"
                && entry.kind == RemoteEntryKind::Symlink
                && entry.link_target.as_deref() == Some("../../outside")
        }),
        "symbolic link metadata was not surfaced without following it"
    );
    ensure!(
        matches!(
            workspace.list("ops/escape-link").await,
            Err(SftpError::SymlinkTraversal)
        ),
        "symbolic link traversal must fail closed"
    );
    ensure!(
        matches!(workspace.list("../").await, Err(SftpError::PathTraversal)),
        "relative traversal must be rejected before an SFTP request"
    );

    let small_source = local.join("small-source.bin");
    std::fs::write(&small_source, b"verified-sftp-payload")?;
    let (progress, _progress_rx) = mpsc::channel(16);
    let receipt = workspace
        .upload(
            &small_source,
            "ops/small.bin",
            TransferOptions {
                conflict: ConflictPolicy::Deny,
                verify: VerifyMode::Sha256,
            },
            &progress,
            &CancellationToken::new(),
        )
        .await?;
    ensure!(receipt.sha256.is_some(), "SHA-256 verification was omitted");
    let renamed = workspace.rename("ops/small.bin", "renamed.bin").await?;
    ensure!(renamed == "ops/renamed.bin", "unexpected renamed path");
    workspace.chmod(&renamed, 0o640).await?;
    let renamed_entry = workspace
        .list("ops")
        .await?
        .into_iter()
        .find(|entry| entry.name == "renamed.bin")
        .context("renamed upload missing from directory")?;
    ensure!(
        renamed_entry.permissions.map(|mode| mode & 0o7777) == Some(0o640),
        "chmod result was not observable"
    );
    Ok(())
}

async fn verify_resumable_transfers(workspace: Arc<SftpWorkspace>, local: &Path) -> Result<()> {
    let large_source = local.join("large-source.bin");
    std::fs::write(&large_source, vec![0x5a; 16 * 1024 * 1024])?;
    let upload_options = TransferOptions {
        conflict: ConflictPolicy::Resume,
        verify: VerifyMode::Size,
    };
    let (upload_progress, mut upload_events) = mpsc::channel(128);
    let upload_cancel = CancellationToken::new();
    let upload_task = {
        let workspace = Arc::clone(&workspace);
        let source = large_source.clone();
        let cancellation = upload_cancel.clone();
        tokio::spawn(async move {
            workspace
                .upload(
                    &source,
                    "ops/large.bin",
                    upload_options,
                    &upload_progress,
                    &cancellation,
                )
                .await
        })
    };
    wait_for_partial_progress(&mut upload_events).await?;
    upload_cancel.cancel();
    ensure!(
        matches!(upload_task.await?, Err(SftpError::Cancelled)),
        "upload cancellation must preserve a resumable partial"
    );
    let (resume_progress, _resume_events) = mpsc::channel(128);
    let resumed_upload = workspace
        .upload(
            &large_source,
            "ops/large.bin",
            upload_options,
            &resume_progress,
            &CancellationToken::new(),
        )
        .await?;
    ensure!(resumed_upload.resumed, "upload did not resume its partial");

    let download_destination = local.join("large-download.bin");
    let (download_progress, mut download_events) = mpsc::channel(128);
    let download_cancel = CancellationToken::new();
    let download_task = {
        let workspace = Arc::clone(&workspace);
        let destination = download_destination.clone();
        let cancellation = download_cancel.clone();
        tokio::spawn(async move {
            workspace
                .download(
                    "ops/large.bin",
                    &destination,
                    upload_options,
                    &download_progress,
                    &cancellation,
                )
                .await
        })
    };
    wait_for_partial_progress(&mut download_events).await?;
    download_cancel.cancel();
    ensure!(
        matches!(download_task.await?, Err(SftpError::Cancelled)),
        "download cancellation must preserve a resumable partial"
    );
    let (resume_download_progress, _resume_download_events) = mpsc::channel(128);
    let resumed_download = workspace
        .download(
            "ops/large.bin",
            &download_destination,
            upload_options,
            &resume_download_progress,
            &CancellationToken::new(),
        )
        .await?;
    ensure!(
        resumed_download.resumed,
        "download did not resume its local partial"
    );
    ensure!(
        std::fs::read(&download_destination)? == std::fs::read(&large_source)?,
        "resumed download bytes changed"
    );

    Ok(())
}

async fn wait_for_partial_progress(
    events: &mut mpsc::Receiver<firstshell_ssh::TransferProgress>,
) -> Result<()> {
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if event.transferred > 0 && event.transferred < event.total {
                return true;
            }
        }
        false
    })
    .await
    .context("transfer did not expose cancellable partial progress")?;
    ensure!(
        observed,
        "transfer ended before partial progress was observed"
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "M3 release acceptance benchmark"]
async fn m3_release_sftp_throughput_and_terminal_fairness() -> Result<()> {
    let (ssh, server, fixture) =
        start_server_with_fixture(PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?)
            .await?;
    let local = tempfile::tempdir()?;
    let source = local.path().join("benchmark-source.bin");
    let payload_bytes = 64 * 1024 * 1024;
    std::fs::write(&source, vec![0xa5; payload_bytes])?;
    std::fs::copy(&source, fixture.root.path().join("benchmark-remote.bin"))?;
    fixture.show_banner.store(false, Ordering::Relaxed);
    let openssh_elapsed = run_openssh_sftp_benchmark(local.path(), ssh).await?;
    let factory = m2_factory(ssh, &local.path().join("known_hosts"), "m0-password");
    approve_unknown_host(&factory).await?;
    let product_elapsed = run_product_sftp_benchmark(&factory, &source, local.path()).await?;
    let throughput_ratio = openssh_elapsed.as_secs_f64() / product_elapsed.as_secs_f64();
    println!(
        "M3_SFTP_BENCHMARK bytes_each={payload_bytes} openssh_ms={} product_ms={} ratio={throughput_ratio:.3}",
        openssh_elapsed.as_millis(),
        product_elapsed.as_millis()
    );
    ensure!(
        throughput_ratio >= 0.85,
        "product SFTP throughput ratio {throughput_ratio:.3} is below 0.85"
    );
    run_terminal_fairness_benchmark(&factory, &source, local.path()).await?;
    server.shutdown("M3 release benchmark complete".to_owned());
    Ok(())
}

#[cfg(windows)]
async fn run_openssh_sftp_benchmark(local: &Path, ssh: SocketAddr) -> Result<Duration> {
    let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let key_path = local.join("benchmark-id-ed25519");
    client_key.write_openssh_file(&key_path, LineEnding::default())?;
    let identity_output = tokio::process::Command::new("whoami.exe").output().await?;
    ensure!(
        identity_output.status.success(),
        "whoami failed for key ACL"
    );
    let identity = String::from_utf8(identity_output.stdout)?.trim().to_owned();
    let key_acl = tokio::process::Command::new("icacls.exe")
        .arg(&key_path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{identity}:(R)"))
        .output()
        .await?;
    ensure!(
        key_acl.status.success(),
        "failed to restrict benchmark private key ACL: {}",
        String::from_utf8_lossy(&key_acl.stderr)
    );
    let batch_path = local.join("benchmark.sftp");
    std::fs::write(
        &batch_path,
        b"put benchmark-source.bin openssh-upload.bin\nget benchmark-remote.bin openssh-download.bin\nquit\n",
    )?;
    let openssh_started = Instant::now();
    let openssh = tokio::process::Command::new("sftp.exe")
        .current_dir(local)
        .arg("-b")
        .arg(&batch_path)
        .arg("-i")
        .arg(&key_path)
        .arg("-P")
        .arg(ssh.port().to_string())
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=NUL")
        .arg(format!("spike@{}", ssh.ip()))
        .output()
        .await?;
    let openssh_elapsed = openssh_started.elapsed();
    ensure!(
        openssh.status.success(),
        "OpenSSH sftp baseline failed: {}",
        String::from_utf8_lossy(&openssh.stderr)
    );
    Ok(openssh_elapsed)
}

async fn run_product_sftp_benchmark(
    factory: &SshSessionFactory,
    source: &Path,
    local: &Path,
) -> Result<Duration> {
    let product_workspace = factory.open_sftp(".").await?;
    let (progress, _events) = mpsc::channel(256);
    let product_started = Instant::now();
    product_workspace
        .upload(
            source,
            "product-upload.bin",
            TransferOptions {
                conflict: ConflictPolicy::Replace,
                verify: VerifyMode::Size,
            },
            &progress,
            &CancellationToken::new(),
        )
        .await?;
    product_workspace
        .download(
            "benchmark-remote.bin",
            &local.join("product-download.bin"),
            TransferOptions {
                conflict: ConflictPolicy::Replace,
                verify: VerifyMode::Size,
            },
            &progress,
            &CancellationToken::new(),
        )
        .await?;
    let product_elapsed = product_started.elapsed();
    product_workspace.close().await;
    Ok(product_elapsed)
}

async fn run_terminal_fairness_benchmark(
    factory: &SshSessionFactory,
    source: &Path,
    local: &Path,
) -> Result<()> {
    let mut terminals = Vec::with_capacity(10);
    for _ in 0..10 {
        terminals.push(factory.connect(TerminalSize::default()).await?);
    }
    let first_workspace = Arc::new(factory.open_sftp(".").await?);
    let second_workspace = Arc::new(factory.open_sftp(".").await?);
    let first_source = local.join("parallel-first.bin");
    let second_source = local.join("parallel-second.bin");
    std::fs::copy(source, &first_source)?;
    std::fs::copy(source, &second_source)?;
    let (first_transfer, mut first_events) = spawn_benchmark_upload(
        Arc::clone(&first_workspace),
        first_source,
        "parallel-first.bin",
    );
    let (second_transfer, mut second_events) = spawn_benchmark_upload(
        Arc::clone(&second_workspace),
        second_source,
        "parallel-second.bin",
    );
    wait_for_partial_progress(&mut first_events).await?;
    wait_for_partial_progress(&mut second_events).await?;

    let mut terminal_tasks = Vec::with_capacity(10);
    for (index, terminal) in terminals.into_iter().enumerate() {
        terminal_tasks.push(tokio::spawn(measure_terminal_latency(terminal, index)));
    }
    let mut latencies = Vec::with_capacity(10);
    for task in terminal_tasks {
        latencies.push(task.await??);
    }
    first_transfer.await??;
    second_transfer.await??;
    latencies.sort_unstable();
    let p95 = latencies[9];
    println!(
        "M3_INTERACTION_BENCHMARK terminals=10 transfers=2 p95_us={} max_us={} input_loss=0",
        p95.as_micros(),
        latencies.last().expect("latency sample").as_micros()
    );
    ensure!(
        p95 < Duration::from_secs(1),
        "terminal p95 exceeded the one-second interaction guardrail"
    );
    first_workspace.close().await;
    second_workspace.close().await;
    Ok(())
}

fn spawn_benchmark_upload(
    workspace: Arc<SftpWorkspace>,
    source: PathBuf,
    remote: &'static str,
) -> (
    tokio::task::JoinHandle<Result<firstshell_ssh::TransferReceipt, SftpError>>,
    mpsc::Receiver<firstshell_ssh::TransferProgress>,
) {
    let (progress, events) = mpsc::channel(256);
    let task = tokio::spawn(async move {
        workspace
            .upload(
                &source,
                remote,
                TransferOptions {
                    conflict: ConflictPolicy::Replace,
                    verify: VerifyMode::Size,
                },
                &progress,
                &CancellationToken::new(),
            )
            .await
    });
    (task, events)
}

async fn measure_terminal_latency(
    mut terminal: ConnectedTransport,
    index: usize,
) -> Result<Duration> {
    let payload = format!("latency-probe-{index}").into_bytes();
    let started = Instant::now();
    terminal
        .commands
        .send(TransportCommand::Input(payload.clone()))
        .await?;
    let received = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = terminal.events.recv().await {
            if matches!(event, SessionEvent::Output { ref data } if data == &payload) {
                return true;
            }
        }
        false
    })
    .await?;
    terminal.commands.send(TransportCommand::Close).await?;
    ensure!(received, "terminal {index} lost its latency probe");
    Ok(started.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn m3_local_dynamic_and_remote_tunnels_should_forward_and_close() -> Result<()> {
    let (ssh, ssh_server) =
        start_server(PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?).await?;
    let echo_listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let echo = echo_listener.local_addr()?;
    let echo_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = echo_listener.accept().await {
            tokio::spawn(async move {
                let (mut reader, mut writer) = stream.split();
                let _ = tokio::io::copy(&mut reader, &mut writer).await;
            });
        }
    });
    let directory = tempfile::tempdir()?;
    let known_hosts = directory.path().join("known_hosts");
    let approve = m2_factory(ssh, &known_hosts, "m0-password");
    approve_unknown_host(&approve).await?;
    let approved_transport = approve.connect(TerminalSize::default()).await?;
    approved_transport
        .commands
        .send(TransportCommand::Close)
        .await?;

    let local = m2_factory(ssh, &known_hosts, "m0-password")
        .start_tunnel(TunnelSpec {
            kind: TunnelKind::Local,
            bind_host: "127.0.0.1".to_owned(),
            bind_port: 0,
            target_host: echo.ip().to_string(),
            target_port: echo.port(),
            allow_non_loopback: false,
        })
        .await?;
    assert_echo(local.snapshot().bind_port, b"local-forward").await?;
    local.close().await;

    let dynamic = m2_factory(ssh, &known_hosts, "m0-password")
        .start_tunnel(TunnelSpec {
            kind: TunnelKind::Dynamic,
            bind_host: "127.0.0.1".to_owned(),
            bind_port: 0,
            target_host: String::new(),
            target_port: 0,
            allow_non_loopback: false,
        })
        .await?;
    let mut socks = TcpStream::connect(("127.0.0.1", dynamic.snapshot().bind_port)).await?;
    socks.write_all(&[5, 1, 0]).await?;
    let mut method = [0_u8; 2];
    socks.read_exact(&mut method).await?;
    ensure!(method == [5, 0], "dynamic tunnel rejected no-auth SOCKS5");
    let IpAddr::V4(address) = echo.ip() else {
        anyhow::bail!("test echo listener must use IPv4");
    };
    let mut request = vec![5, 1, 0, 1];
    request.extend_from_slice(&address.octets());
    request.extend_from_slice(&echo.port().to_be_bytes());
    socks.write_all(&request).await?;
    let mut reply = [0_u8; 10];
    socks.read_exact(&mut reply).await?;
    ensure!(reply[1] == 0, "dynamic SOCKS5 CONNECT failed");
    assert_stream_echo(&mut socks, b"dynamic-forward").await?;
    dynamic.close().await;

    let remote = m2_factory(ssh, &known_hosts, "m0-password")
        .start_tunnel(TunnelSpec {
            kind: TunnelKind::Remote,
            bind_host: "127.0.0.1".to_owned(),
            bind_port: 0,
            target_host: echo.ip().to_string(),
            target_port: echo.port(),
            allow_non_loopback: false,
        })
        .await?;
    assert_echo(remote.snapshot().bind_port, b"remote-forward").await?;
    remote.close().await;

    echo_task.abort();
    ssh_server.shutdown("M3 tunnel test complete".to_owned());
    Ok(())
}

async fn assert_echo(port: u16, payload: &[u8]) -> Result<()> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    assert_stream_echo(&mut stream, payload).await
}

async fn assert_stream_echo(stream: &mut TcpStream, payload: &[u8]) -> Result<()> {
    stream.write_all(payload).await?;
    let mut response = vec![0_u8; payload.len()];
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut response)).await??;
    ensure!(response == payload, "forwarded echo did not match");
    Ok(())
}
