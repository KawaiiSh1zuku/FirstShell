//! End-to-end M0 russh terminal and SFTP compatibility spike.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use russh::client;
use russh::keys::{Algorithm, PrivateKey, PublicKeyOrCertificate};
use russh::server::{Auth, Msg, Server as _, Session};
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
