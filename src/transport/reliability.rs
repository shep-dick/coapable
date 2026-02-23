use std::time::Duration;

use rand::Rng;
use tokio::time::Instant;

// RFC 7252 Section 4.8 — Transmission Parameters
pub const ACK_TIMEOUT: Duration = Duration::from_secs(2);
pub const ACK_RANDOM_FACTOR: f64 = 1.5;
pub const MAX_RETRANSMIT: u32 = 4;
pub const MAX_LATENCY: Duration = Duration::from_secs(100);
pub const EXCHANGE_LIFETIME: Duration = Duration::from_secs(247);
pub const NON_LIFETIME: Duration = Duration::from_secs(145);

// Self-defined parameters
pub const IDLE_SESSION_CLEANUP_INTERVAL_SECS: u64 = 30;

/// Returns a random ACK timeout in [ACK_TIMEOUT, ACK_TIMEOUT * ACK_RANDOM_FACTOR].
fn random_ack_timeout() -> Duration {
    let lo = ACK_TIMEOUT.as_secs_f64();
    let hi = lo * ACK_RANDOM_FACTOR;
    let secs = rand::rng().random_range(lo..=hi);
    Duration::from_secs_f64(secs)
}

/// Per-peer message ID allocator. Wrapping u16 counter with random start.
pub struct MessageIdAllocator {
    next: u16,
}

impl MessageIdAllocator {
    pub fn new() -> Self {
        Self {
            next: rand::rng().random(),
        }
    }

    pub fn allocate(&mut self) -> u16 {
        let mid = self.next;
        self.next = self.next.wrapping_add(1);
        mid
    }
}

/// Retransmission state for a single CON exchange.
pub struct RetransmitState {
    /// Serialized packet bytes, ready to re-send.
    packet_bytes: Vec<u8>,
    /// How many retransmissions have been sent (0 after initial send).
    retransmit_count: u32,
    /// Current timeout before next retransmit (doubles each time).
    current_timeout: Duration,
    /// When the next retransmit should fire.
    next_retransmit: Instant,
}

impl RetransmitState {
    pub fn new(packet_bytes: Vec<u8>, now: Instant) -> Self {
        let initial_timeout = random_ack_timeout();
        Self {
            packet_bytes,
            retransmit_count: 0,
            current_timeout: initial_timeout,
            next_retransmit: now + initial_timeout,
        }
    }

    /// Advance to the next retransmission. Returns false if MAX_RETRANSMIT exceeded.
    pub fn advance(&mut self, now: Instant) -> bool {
        if self.retransmit_count >= MAX_RETRANSMIT {
            return false;
        }
        self.retransmit_count += 1;
        self.current_timeout *= 2;
        self.next_retransmit = now + self.current_timeout;
        true
    }

    pub fn retransmit_count(&self) -> u32 {
        self.retransmit_count
    }

    pub fn packet_bytes(&self) -> &[u8] {
        &self.packet_bytes
    }

    pub fn deadline(&self) -> Instant {
        self.next_retransmit
    }

    pub fn is_due(&self, now: Instant) -> bool {
        now >= self.next_retransmit
    }
}

/// Dedup entry for inbound messages.
pub struct DedupEntry {
    /// Cached response bytes for re-sending to duplicate CON requests.
    pub cached_response: Option<Vec<u8>>,
    /// When this entry was created.
    pub created: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_allocator_wraps() {
        let mut alloc = MessageIdAllocator { next: u16::MAX - 1 };
        assert_eq!(alloc.allocate(), u16::MAX - 1);
        assert_eq!(alloc.allocate(), u16::MAX);
        assert_eq!(alloc.allocate(), 0);
        assert_eq!(alloc.allocate(), 1);
    }

    #[tokio::test]
    async fn retransmit_state_backoff() {
        tokio::time::pause();
        let now = Instant::now();
        let mut state = RetransmitState::new(vec![1, 2, 3], now);

        // Initial timeout is in [2s, 3s]
        let initial = state.current_timeout;
        assert!(initial >= ACK_TIMEOUT);
        assert!(initial <= ACK_TIMEOUT.mul_f64(ACK_RANDOM_FACTOR));

        // Each advance doubles the timeout
        for i in 0..MAX_RETRANSMIT {
            assert!(state.advance(now), "advance should succeed at count {}", i);
            assert_eq!(state.retransmit_count(), i + 1);
        }

        // Next advance should fail — MAX_RETRANSMIT exceeded
        assert!(!state.advance(now));
    }

    #[test]
    fn random_ack_timeout_in_range() {
        for _ in 0..100 {
            let t = random_ack_timeout();
            assert!(t >= ACK_TIMEOUT);
            assert!(t <= ACK_TIMEOUT.mul_f64(ACK_RANDOM_FACTOR));
        }
    }
}
