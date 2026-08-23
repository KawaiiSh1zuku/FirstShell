//! Bounded scheduling primitives for interactive and bulk remote work.

use thiserror::Error;
use tokio::sync::mpsc;

/// Default maximum number of pending interactive messages.
pub const INTERACTIVE_QUEUE_CAPACITY: usize = 256;
/// Default maximum number of pending bulk jobs.
pub const BULK_QUEUE_CAPACITY: usize = 32;

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
}
