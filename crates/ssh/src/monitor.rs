//! Bounded agentless host probes executed through an authenticated SSH chain.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use firstshell_session::{INTERACTIVE_QUEUE_CAPACITY, SessionEvent};
use russh::ChannelMsg;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

use super::{AuthenticatedChain, SshError, SshSessionFactory};

const PROBE_VERSION: u16 = 1;
const MAX_PROBE_OUTPUT: usize = 512 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_TARGET_BYTES: usize = 253;

/// Supported fixed-template agentless probes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ProbeKind {
    /// Linux CPU, memory, filesystem, network, and load counters.
    HostMetrics,
    /// Top processes with bounded rows.
    Processes,
    /// Listening TCP and UDP sockets with bounded rows.
    ListeningPorts,
    /// ICMP reachability to a validated hostname or address.
    Ping {
        /// Target that cannot contain shell metacharacters.
        target: String,
        /// Packet count from one through ten.
        count: u8,
    },
    /// Hop discovery to a validated hostname or address.
    Traceroute {
        /// Target that cannot contain shell metacharacters.
        target: String,
        /// Maximum hop count from one through 32.
        max_hops: u8,
    },
}

/// One versioned scalar value emitted by a probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeMetric {
    /// Stable machine-readable key.
    pub key: String,
    /// Numeric value when parsing succeeded.
    pub value: f64,
    /// Stable unit such as `bytes`, `percent`, or `count`.
    pub unit: String,
}

/// One bounded structured table row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRow {
    /// Stable row category.
    pub kind: String,
    /// Sanitized cells with terminal controls removed.
    pub cells: Vec<String>,
}

/// Public result of one fixed-template probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// Probe schema version.
    pub version: u16,
    /// Probe request that produced this result.
    pub probe: ProbeKind,
    /// Local capture time in Unix milliseconds.
    pub captured_at_ms: u64,
    /// Remote process exit code when supplied by the server.
    pub exit_code: Option<u32>,
    /// Bounded parsed scalar metrics.
    pub metrics: Vec<ProbeMetric>,
    /// Bounded parsed table rows.
    pub rows: Vec<ProbeRow>,
    /// Stable diagnostic state instead of unbounded remote stderr.
    pub diagnostic: Option<String>,
}

impl SshSessionFactory {
    /// Opens one authenticated low-priority probe connection for repeated bounded samples.
    ///
    /// # Errors
    /// Returns a host-key, authentication, or transport error.
    pub async fn open_probe_session(&self) -> Result<ProbeSession, ProbeError> {
        let (events, _receiver) = mpsc::channel::<SessionEvent>(INTERACTIVE_QUEUE_CAPACITY);
        Ok(ProbeSession {
            chain: self.connect_authenticated(events).await?,
        })
    }

    /// Runs one low-priority fixed-template probe without accepting arbitrary shell text.
    ///
    /// # Errors
    /// Returns a host-key, authentication, timeout, output-limit, or parse error.
    pub async fn run_probe(&self, probe: ProbeKind) -> Result<ProbeResult, ProbeError> {
        let session = self.open_probe_session().await?;
        let result = session.run(probe).await;
        session.close().await;
        result
    }
}

/// Authenticated agentless-probe connection reused across bounded samples.
pub struct ProbeSession {
    chain: AuthenticatedChain,
}

impl std::fmt::Debug for ProbeSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProbeSession")
            .finish_non_exhaustive()
    }
}

impl ProbeSession {
    /// Runs one fixed-template probe on a fresh SSH session channel.
    ///
    /// # Errors
    /// Returns a validation, timeout, output-limit, or transport error.
    pub async fn run(&self, probe: ProbeKind) -> Result<ProbeResult, ProbeError> {
        let command = probe_command(&probe)?;
        let result = async {
            let mut channel = self
                .chain
                .final_client
                .channel_open_session()
                .await
                .map_err(SshError::Russh)?;
            channel
                .exec(true, command.as_bytes())
                .await
                .map_err(SshError::Russh)?;
            collect_probe(&mut channel, probe).await
        };
        tokio::time::timeout(PROBE_TIMEOUT, result)
            .await
            .map_err(|_| ProbeError::Timeout)?
    }

    /// Closes the probe connection and every jump in reverse order.
    pub async fn close(self) {
        self.chain
            .disconnect("agentless probe session closed")
            .await;
    }
}

async fn collect_probe(
    channel: &mut russh::Channel<russh::client::Msg>,
    probe: ProbeKind,
) -> Result<ProbeResult, ProbeError> {
    let mut output = Vec::new();
    let mut exit_code = None;
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                if output.len().saturating_add(data.len()) > MAX_PROBE_OUTPUT {
                    return Err(ProbeError::OutputLimit);
                }
                output.extend_from_slice(&data);
            }
            ChannelMsg::ExitStatus { exit_status } => exit_code = Some(exit_status),
            _ => {}
        }
    }
    Ok(parse_probe(probe, &output, exit_code))
}

fn probe_command(probe: &ProbeKind) -> Result<String, ProbeError> {
    match probe {
        ProbeKind::HostMetrics => Ok(concat!(
            "LC_ALL=C sh -c '",
            "printf \"FS1\\n\"; ",
            "awk '\"'\"'/^cpu /{t=0; for(i=2;i<=NF;i++)t+=$i; print \"metric cpu_total count \"t; print \"metric cpu_idle count \"$5+$6}'\"'\"' /proc/stat 2>/dev/null; ",
            "awk '\"'\"'/^MemTotal:/{print \"metric memory_total bytes \"$2*1024}/^MemAvailable:/{print \"metric memory_available bytes \"$2*1024}'\"'\"' /proc/meminfo 2>/dev/null; ",
            "awk '\"'\"'{print \"metric load_1m count \"$1}'\"'\"' /proc/loadavg 2>/dev/null; ",
            "df -Pk 2>/dev/null | awk '\"'\"'NR>1 && n++<32 {print \"row disk\\t\"$6\"\\t\"$2*1024\"\\t\"$4*1024}'\"'\"'; ",
            "awk '\"'\"'NR>2 && n++<32 {gsub(/:/,\"\",$1); print \"row network\\t\"$1\"\\t\"$2\"\\t\"$10}'\"'\"' /proc/net/dev 2>/dev/null'"
        )
        .to_owned()),
        ProbeKind::Processes => Ok(
            "LC_ALL=C sh -c 'printf \"FS1\\n\"; ps -eo pid=,pcpu=,pmem=,comm= --sort=-pcpu 2>/dev/null | head -n 64 | sed \"s/^/row process\\t/\"'"
                .to_owned(),
        ),
        ProbeKind::ListeningPorts => Ok(
            "LC_ALL=C sh -c 'printf \"FS1\\n\"; ss -H -lntu 2>/dev/null | head -n 256 | sed \"s/^/row port\\t/\"'"
                .to_owned(),
        ),
        ProbeKind::Ping { target, count } => {
            validate_target(target)?;
            if !(1..=10).contains(count) {
                return Err(ProbeError::InvalidInput);
            }
            Ok(format!(
                "LC_ALL=C sh -c 'printf \"FS1\\n\"; ping -n -c {count} -W 2 -- {target} 2>&1 | head -n 64 | sed \"s/^/row ping\\t/\"'"
            ))
        }
        ProbeKind::Traceroute { target, max_hops } => {
            validate_target(target)?;
            if !(1..=32).contains(max_hops) {
                return Err(ProbeError::InvalidInput);
            }
            Ok(format!(
                "LC_ALL=C sh -c 'printf \"FS1\\n\"; traceroute -n -m {max_hops} -w 2 -- {target} 2>&1 | head -n 64 | sed \"s/^/row hop\\t/\"'"
            ))
        }
    }
}

fn validate_target(target: &str) -> Result<(), ProbeError> {
    if target.is_empty()
        || target.len() > MAX_TARGET_BYTES
        || target.starts_with('-')
        || target
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'.' | b'-' | b':' | b'_'))
    {
        Err(ProbeError::InvalidInput)
    } else {
        Ok(())
    }
}

fn parse_probe(probe: ProbeKind, output: &[u8], exit_code: Option<u32>) -> ProbeResult {
    let text = String::from_utf8_lossy(output);
    let mut metrics = Vec::new();
    let mut rows = Vec::new();
    let mut version_valid = false;
    for line in text.lines().take(512) {
        let line = sanitize_cell(line);
        if line == "FS1" {
            version_valid = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("metric ") {
            let mut parts = rest.split_whitespace();
            if let (Some(key), Some(unit), Some(value)) = (parts.next(), parts.next(), parts.next())
                && let Ok(value) = value.parse::<f64>()
                && metrics.len() < 128
            {
                metrics.push(ProbeMetric {
                    key: key.to_owned(),
                    value,
                    unit: unit.to_owned(),
                });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("row ")
            && rows.len() < 256
        {
            let mut cells = rest.split('\t');
            if let Some(kind) = cells.next() {
                rows.push(ProbeRow {
                    kind: kind.to_owned(),
                    cells: cells.map(sanitize_cell).take(12).collect(),
                });
            }
        }
    }
    let diagnostic = if !version_valid {
        Some("PROBE_SCHEMA_MISSING".to_owned())
    } else if exit_code.is_some_and(|code| code != 0) {
        Some("REMOTE_COMMAND_FAILED".to_owned())
    } else if metrics.is_empty() && rows.is_empty() {
        Some("PROBE_UNAVAILABLE".to_owned())
    } else {
        None
    };
    ProbeResult {
        version: PROBE_VERSION,
        probe,
        captured_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        exit_code,
        metrics,
        rows,
        diagnostic,
    }
}

fn sanitize_cell(value: &str) -> String {
    value
        .chars()
        .filter(|character| *character == '\t' || !character.is_control())
        .take(1024)
        .collect()
}

/// Stable agentless-probe failure.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// SSH authentication, host-key, or transport setup failed.
    #[error("probe SSH setup failed")]
    Ssh(#[from] SshError),
    /// Target or bounded probe option is invalid.
    #[error("invalid probe input")]
    InvalidInput,
    /// Probe exceeded its fixed deadline.
    #[error("probe timed out")]
    Timeout,
    /// Remote output exceeded the fixed memory cap.
    #[error("probe output limit exceeded")]
    OutputLimit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_reject_shell_injection_and_option_smuggling() {
        assert!(validate_target("example.com").is_ok());
        assert!(validate_target("2001:db8::1").is_ok());
        assert!(matches!(
            validate_target("-f/etc/passwd"),
            Err(ProbeError::InvalidInput)
        ));
        assert!(matches!(
            validate_target("host;id"),
            Err(ProbeError::InvalidInput)
        ));
        assert!(matches!(
            validate_target("$(id)"),
            Err(ProbeError::InvalidInput)
        ));
    }

    #[test]
    fn parser_emits_versioned_bounded_sanitized_values() {
        let result = parse_probe(
            ProbeKind::HostMetrics,
            b"FS1\nmetric cpu_total count 100\nrow disk\t/\t4096\t2048\x1b\n",
            Some(0),
        );
        assert_eq!(result.version, 1);
        assert_eq!(result.metrics[0].key, "cpu_total");
        assert_eq!(result.rows[0].cells, ["/", "4096", "2048"]);
        assert_eq!(result.diagnostic, None);
    }

    #[test]
    fn missing_schema_is_diagnosable_without_echoing_remote_output() {
        let result = parse_probe(ProbeKind::Processes, b"\x1b[31m hostile\n", Some(127));
        assert_eq!(result.diagnostic.as_deref(), Some("PROBE_SCHEMA_MISSING"));
        assert!(result.metrics.is_empty());
        assert!(result.rows.is_empty());
    }
}
