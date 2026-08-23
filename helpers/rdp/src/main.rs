//! Isolated native RDP helper entrypoint.

#[cfg(target_os = "windows")]
mod windows {
    use std::io::Read;

    use anyhow::{Context, Result, bail};
    use firstshell_rdp_protocol::{Handshake, HandshakeResponse, PROTOCOL_VERSION};
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;

    const MAX_FRAME_BYTES: usize = 4096;

    pub async fn run() -> Result<()> {
        let pipe_name = parse_pipe_argument()?;
        let token = read_launch_token()?;
        let mut pipe = ClientOptions::new()
            .open(&pipe_name)
            .with_context(|| format!("failed to connect to desktop pipe {pipe_name}"))?;

        write_frame(
            &mut pipe,
            &Handshake {
                version: PROTOCOL_VERSION,
                token,
            },
        )
        .await?;
        let response: HandshakeResponse = read_frame(&mut pipe).await?;
        if response.version != PROTOCOL_VERSION || !response.accepted {
            bail!("desktop rejected the RDP helper handshake");
        }
        Ok(())
    }

    fn parse_pipe_argument() -> Result<String> {
        let mut arguments = std::env::args().skip(1);
        let flag = arguments.next().context("missing --pipe argument")?;
        let pipe = arguments.next().context("missing named pipe value")?;
        if flag != "--pipe" || arguments.next().is_some() {
            bail!("usage: firstshell-rdp-helper --pipe <name>");
        }
        Ok(pipe)
    }

    fn read_launch_token() -> Result<String> {
        let mut token = String::new();
        std::io::stdin()
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_to_string(&mut token)
            .context("failed to read launch token from stdin")?;
        let token = token.trim_end_matches(['\r', '\n']).to_owned();
        if token.is_empty() || token.len() > MAX_FRAME_BYTES {
            bail!("invalid launch token length");
        }
        Ok(token)
    }

    async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
        writer: &mut W,
        value: &T,
    ) -> Result<()> {
        let payload = serde_json::to_vec(value).context("failed to encode helper IPC frame")?;
        if payload.len() > MAX_FRAME_BYTES {
            bail!("helper IPC frame exceeds limit");
        }
        let length = u32::try_from(payload.len()).context("helper IPC frame length overflow")?;
        writer.write_u32_le(length).await?;
        writer.write_all(&payload).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(reader: &mut R) -> Result<T> {
        let length = reader.read_u32_le().await? as usize;
        if length > MAX_FRAME_BYTES {
            bail!("desktop IPC frame exceeds limit");
        }
        let mut payload = vec![0_u8; length];
        reader.read_exact(&mut payload).await?;
        serde_json::from_slice(&payload).context("failed to decode desktop IPC frame")
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
