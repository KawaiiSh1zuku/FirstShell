//! M0 real-process named-pipe authentication spike for the `Windows` RDP helper.

#![cfg(target_os = "windows")]

use std::process::Stdio;

use firstshell_rdp_protocol::{
    DesktopRequest, Handshake, HandshakeResponse, HelperEvent, HelperState, PROTOCOL_VERSION,
    read_frame, write_frame,
};
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::process::Command;
use uuid::Uuid;

async fn run_handshake_case(
    supplied_token: &str,
    expected_token: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let pipe_name = format!(r"\\.\pipe\firstshell-rdp-{}", Uuid::new_v4());
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&pipe_name)?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_firstshell-rdp-helper"))
        .arg("--pipe")
        .arg(&pipe_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
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
    if accepted {
        write_frame(&mut server, &DesktopRequest::Ping).await?;
        assert_eq!(
            read_frame::<_, HelperEvent>(&mut server).await?,
            HelperEvent::Pong
        );
        write_frame(&mut server, &DesktopRequest::Stop).await?;
        assert_eq!(
            read_frame::<_, HelperEvent>(&mut server).await?,
            HelperEvent::State {
                state: HelperState::Stopped,
            }
        );
    }
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
