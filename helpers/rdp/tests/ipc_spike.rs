//! M0 real-process named-pipe authentication spike for the `Windows` RDP helper.

#![cfg(target_os = "windows")]

use std::process::Stdio;

use firstshell_rdp_protocol::{Handshake, HandshakeResponse, PROTOCOL_VERSION};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::process::Command;
use uuid::Uuid;

const MAX_FRAME_BYTES: usize = 4096;

async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let payload = serde_json::to_vec(value)?;
    assert!(payload.len() <= MAX_FRAME_BYTES);
    writer.write_u32_le(u32::try_from(payload.len())?).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
) -> Result<T, Box<dyn std::error::Error>> {
    let length = reader.read_u32_le().await? as usize;
    assert!(length <= MAX_FRAME_BYTES);
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
}

async fn run_handshake_case(
    supplied_token: &str,
    expected_token: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let pipe_name = format!(r"\\.\pipe\firstshell-m0-{}", Uuid::new_v4());
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&pipe_name)?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_firstshell-rdp-helper"))
        .arg("--pipe")
        .arg(&pipe_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut child_stdin = child.stdin.take().ok_or("helper stdin was not piped")?;
    child_stdin.write_all(supplied_token.as_bytes()).await?;
    drop(child_stdin);

    tokio::time::timeout(std::time::Duration::from_secs(5), server.connect()).await??;
    let handshake: Handshake = read_frame(&mut server).await?;
    let accepted = handshake.version == PROTOCOL_VERSION && handshake.token == expected_token;
    write_frame(
        &mut server,
        &HandshakeResponse {
            version: PROTOCOL_VERSION,
            accepted,
        },
    )
    .await?;
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await??;
    Ok(status.success())
}

#[tokio::test]
async fn helper_should_accept_the_one_time_launch_token() -> Result<(), Box<dyn std::error::Error>>
{
    let token = Uuid::new_v4().to_string();
    assert!(run_handshake_case(&token, &token).await?);
    Ok(())
}

#[tokio::test]
async fn helper_should_reject_an_unauthorized_launch_token()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = Uuid::new_v4().to_string();
    let supplied = Uuid::new_v4().to_string();
    assert!(!run_handshake_case(&supplied, &expected).await?);
    Ok(())
}
