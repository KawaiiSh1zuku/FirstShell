//! End-to-end M0 russh terminal and SFTP compatibility spike.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use firstshell_session::{SessionEvent, SessionFactory, TerminalSize, TransportCommand};
use firstshell_ssh::{SshAuthentication, SshConnectSpec, SshSessionFactory, TerminalEncoding};
use russh::client;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, PrivateKey, PublicKeyOrCertificate};
use russh::server::{Auth, Msg, Response, Server as _, Session};
use russh::{Channel, ChannelId, ChannelMsg, Disconnect, Pty};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{File, FileAttributes, Handle, Name, Status, StatusCode, Version};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Clone)]
struct SpikeServer;

impl russh::server::Server for SpikeServer {
    type Handler = SpikeSession;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        SpikeSession::default()
    }
}

#[derive(Default)]
struct SpikeSession {
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
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
        session.data(channel, data.to_vec())?;
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
        tokio::spawn(async move {
            russh_sftp::server::run(sftp_channel.into_stream(), SpikeSftp::default()).await;
        });
        Ok(())
    }
}

#[derive(Default)]
struct SpikeSftp {
    root_read: bool,
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

    async fn realpath(&mut self, id: u32, _path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy("/")],
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        self.root_read = false;
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if handle == "/" && !self.root_read {
            self.root_read = true;
            return Ok(Name {
                id,
                files: vec![File::new("probe.txt", FileAttributes::default())],
            });
        }
        Err(StatusCode::Eof)
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: "en-US".to_owned(),
        })
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

    let mut server = SpikeServer;
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
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let config = Arc::new(russh::server::Config {
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![key],
        ..russh::server::Config::default()
    });
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut server = SpikeServer;
        let running = server.run_on_socket(config, &listener);
        let _ = handle_tx.send(running.handle());
        let _ = running.await;
    });
    Ok((addr, handle_rx.await.context("server handle unavailable")?))
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
