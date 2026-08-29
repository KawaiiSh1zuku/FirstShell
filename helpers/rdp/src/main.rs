//! Isolated native RDP helper entrypoint.

#[cfg(target_os = "windows")]
mod windows {
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    use anyhow::{Context, Result, bail};
    use firstshell_rdp_protocol::{
        CertificatePolicy, DesktopRequest, Handshake, HandshakeResponse, HelperEvent, HelperState,
        PROTOCOL_VERSION, StartSession, WindowMode, read_frame, write_frame,
    };
    use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use tokio::process::{Child, Command};
    use zeroize::Zeroize;

    const MAX_TOKEN_BYTES: usize = 256;

    pub async fn run() -> Result<()> {
        let pipe_name = parse_pipe_argument()?;
        let mut token = read_launch_token()?;
        let pipe = ClientOptions::new()
            .open(&pipe_name)
            .with_context(|| format!("failed to connect to desktop pipe {pipe_name}"))?;
        let (mut reader, mut writer) = tokio::io::split(pipe);

        let mut handshake = Handshake {
            version: PROTOCOL_VERSION,
            token: token.clone(),
        };
        let handshake_result = write_frame(&mut writer, &handshake).await;
        handshake.token.zeroize();
        handshake_result?;
        token.zeroize();
        let response: HandshakeResponse = read_frame(&mut reader).await?;
        if response.version != PROTOCOL_VERSION || !response.accepted {
            bail!("desktop rejected the RDP helper handshake");
        }

        let Some(mut start) = await_start(&mut reader, &mut writer).await? else {
            return Ok(());
        };
        start.validate()?;
        write_frame(
            &mut writer,
            &HelperEvent::State {
                state: HelperState::Starting,
            },
        )
        .await?;

        match launch_freerdp(&mut start).await {
            Ok(child) => supervise(child, reader, writer).await,
            Err(error) => {
                start
                    .password
                    .as_mut()
                    .iter_mut()
                    .for_each(|value| value.zeroize());
                let _ = write_frame(
                    &mut writer,
                    &HelperEvent::Error {
                        code: "RDP_BACKEND_UNAVAILABLE".to_owned(),
                        message: error.to_string(),
                    },
                )
                .await;
                Err(error)
            }
        }
    }

    async fn await_start(
        reader: &mut ReadHalf<NamedPipeClient>,
        writer: &mut WriteHalf<NamedPipeClient>,
    ) -> Result<Option<StartSession>> {
        loop {
            match read_frame::<_, DesktopRequest>(reader).await? {
                DesktopRequest::Start(start) => return Ok(Some(start)),
                DesktopRequest::Ping => write_frame(writer, &HelperEvent::Pong).await?,
                DesktopRequest::Stop => {
                    write_frame(
                        writer,
                        &HelperEvent::State {
                            state: HelperState::Stopped,
                        },
                    )
                    .await?;
                    return Ok(None);
                }
            }
        }
    }

    async fn supervise(
        mut child: Child,
        mut reader: ReadHalf<NamedPipeClient>,
        mut writer: WriteHalf<NamedPipeClient>,
    ) -> Result<()> {
        write_frame(
            &mut writer,
            &HelperEvent::State {
                state: HelperState::Running,
            },
        )
        .await?;
        loop {
            tokio::select! {
                status = child.wait() => {
                    let status = status.context("failed waiting for FreeRDP")?;
                    write_frame(&mut writer, &HelperEvent::Exited {
                        code: status.code(),
                        requested: false,
                    }).await?;
                    return if status.success() {
                        Ok(())
                    } else {
                        bail!("FreeRDP exited unsuccessfully")
                    };
                }
                message = read_frame::<_, DesktopRequest>(&mut reader) => {
                    match message? {
                        DesktopRequest::Stop => {
                            child.kill().await.context("failed to stop FreeRDP")?;
                            let status = child.wait().await.context("failed waiting for stopped FreeRDP")?;
                            write_frame(&mut writer, &HelperEvent::Exited {
                                code: status.code(),
                                requested: true,
                            }).await?;
                            return Ok(());
                        }
                        DesktopRequest::Ping => write_frame(&mut writer, &HelperEvent::Pong).await?,
                        DesktopRequest::Start(mut duplicate) => {
                            duplicate.password.as_mut().iter_mut().for_each(|value| value.zeroize());
                            write_frame(&mut writer, &HelperEvent::Error {
                                code: "RDP_INVALID_STATE".to_owned(),
                                message: "RDP helper already owns a session".to_owned(),
                            }).await?;
                        }
                    }
                }
            }
        }
    }

    async fn launch_freerdp(request: &mut StartSession) -> Result<Child> {
        let executable = freerdp_executable()?;
        let mut command = Command::new(executable);
        command
            .arg("/args-from:stdin")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .context("failed to launch the bundled FreeRDP backend")?;
        let mut stdin = child.stdin.take().context("FreeRDP stdin was not piped")?;
        let arguments = freerdp_arguments(request)?;
        for mut argument in arguments {
            stdin.write_all(argument.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            argument.zeroize();
        }
        stdin.shutdown().await?;
        request
            .password
            .as_mut()
            .iter_mut()
            .for_each(|value| value.zeroize());
        Ok(child)
    }

    fn freerdp_arguments(request: &StartSession) -> Result<Vec<String>> {
        request.validate()?;
        let mut arguments = vec![
            format!("/v:{}:{}", request.host, request.port),
            format!("/u:{}", request.username),
            format!("/w:{}", request.width),
            format!("/h:{}", request.height),
            "+dynamic-resolution".to_owned(),
            "+clipboard".to_owned(),
            "/network:auto".to_owned(),
            "/title:FirstShell RDP".to_owned(),
        ];
        if let Some(domain) = request.domain.as_ref().filter(|value| !value.is_empty()) {
            arguments.push(format!("/d:{domain}"));
        }
        if let Some(password) = &request.password {
            arguments.push(format!("/p:{password}"));
        } else {
            arguments.push("/p".to_owned());
        }
        arguments.push(match &request.certificate {
            CertificatePolicy::Deny => "/cert:deny".to_owned(),
            CertificatePolicy::Tofu => "/cert:tofu".to_owned(),
            CertificatePolicy::Fingerprint(value) => format!("/cert:fingerprint:{value}"),
        });
        if let WindowMode::Embedded { parent_window } = request.window_mode {
            arguments.push(format!("/parent-window:{parent_window}"));
            arguments.push("-decorations".to_owned());
        }
        Ok(arguments)
    }

    fn freerdp_executable() -> Result<PathBuf> {
        let helper = std::env::current_exe().context("failed to resolve the RDP helper path")?;
        let directory = helper
            .parent()
            .context("RDP helper has no parent directory")?;
        let candidate = directory.join("wfreerdp.exe");
        ensure_regular_file(&candidate)?;
        Ok(candidate)
    }

    fn ensure_regular_file(path: &Path) -> Result<()> {
        let metadata = std::fs::metadata(path)
            .with_context(|| "bundled wfreerdp.exe was not found beside the RDP helper")?;
        if !metadata.is_file() {
            bail!("bundled FreeRDP backend is not a regular file");
        }
        Ok(())
    }

    fn parse_pipe_argument() -> Result<String> {
        let mut arguments = std::env::args().skip(1);
        let flag = arguments.next().context("missing --pipe argument")?;
        let pipe = arguments.next().context("missing named pipe value")?;
        if flag != "--pipe"
            || arguments.next().is_some()
            || !pipe.starts_with(r"\\.\pipe\firstshell-rdp-")
        {
            bail!("usage: firstshell-rdp-helper --pipe <FirstShell pipe>");
        }
        Ok(pipe)
    }

    fn read_launch_token() -> Result<String> {
        let mut token = String::new();
        std::io::stdin()
            .take((MAX_TOKEN_BYTES + 1) as u64)
            .read_to_string(&mut token)
            .context("failed to read launch token from stdin")?;
        let token = token.trim_end_matches(['\r', '\n']).to_owned();
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES || token.contains(['\r', '\n', '\0']) {
            bail!("invalid launch token");
        }
        Ok(token)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn request(password: Option<&str>) -> StartSession {
            StartSession {
                session_id: "session-1".to_owned(),
                host: "rdp.example".to_owned(),
                port: 3389,
                username: "operator".to_owned(),
                domain: Some("EXAMPLE".to_owned()),
                password: password.map(str::to_owned),
                width: 1280,
                height: 720,
                window_mode: WindowMode::Companion,
                certificate: CertificatePolicy::Tofu,
            }
        }

        #[test]
        fn arguments_should_include_password_only_in_stdin_payload() -> Result<()> {
            let values = freerdp_arguments(&request(Some("never-print-this")))?;
            assert!(values.iter().any(|value| value == "/p:never-print-this"));
            Ok(())
        }

        #[test]
        fn embedded_mode_should_use_valid_parent_handle() -> Result<()> {
            let mut value = request(None);
            value.window_mode = WindowMode::Embedded { parent_window: 42 };
            let values = freerdp_arguments(&value)?;
            assert!(values.iter().any(|value| value == "/parent-window:42"));
            Ok(())
        }
    }
}

#[cfg(target_os = "windows")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    windows::run().await
}

#[cfg(not(target_os = "windows"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("the RDP helper is only supported on Windows")
}
