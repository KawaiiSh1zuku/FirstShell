//! `SSH` boundary and M0 capability declarations.

/// One capability exercised by the M0 `russh` spike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshSpikeCapability {
    /// Interactive PTY channel is available.
    Terminal,
    /// Server identity can be verified before authentication.
    HostKey,
    /// Password and public-key authentication hooks are available.
    Authentication,
    /// `SFTP` can run on a separately scheduled channel.
    Sftp,
}

const REQUIRED_CAPABILITIES: [SshSpikeCapability; 4] = [
    SshSpikeCapability::Terminal,
    SshSpikeCapability::HostKey,
    SshSpikeCapability::Authentication,
    SshSpikeCapability::Sftp,
];

/// Capability set verified by an `SSH` compatibility spike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshSpikeCapabilities {
    supported: &'static [SshSpikeCapability],
}

impl SshSpikeCapabilities {
    /// Returns the capability set required by the frozen architecture.
    #[must_use]
    pub const fn required() -> Self {
        Self {
            supported: &REQUIRED_CAPABILITIES,
        }
    }

    /// Returns true only when all critical capabilities are present.
    #[must_use]
    pub fn is_complete(self) -> bool {
        REQUIRED_CAPABILITIES
            .iter()
            .all(|required| self.supported.contains(required))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_spike_capabilities_should_be_complete() {
        assert!(SshSpikeCapabilities::required().is_complete());
    }
}
