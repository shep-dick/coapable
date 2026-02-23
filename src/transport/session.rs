use std::collections::HashMap;
use std::net::SocketAddr;

use coap_lite::{MessageClass, Packet};
use tokio::time::Instant;

use super::exchange::Exchange;
use super::reliability::{DedupEntry, EXCHANGE_LIFETIME, MessageIdAllocator, TokenAllocator};

pub struct PeerSession {
    peer: SocketAddr,
    exchanges: HashMap<Vec<u8>, Exchange>,
    /// Maps outbound MID → token, for ACK/RST matching.
    mid_to_token: HashMap<u16, Vec<u8>>,
    /// Per-peer MID allocator for outbound messages.
    mid_allocator: MessageIdAllocator,
    /// Per-peer token allocator for outbound requests.
    token_allocator: TokenAllocator,
    /// Dedup table for inbound messages, keyed by MID.
    inbound_dedup: HashMap<u16, DedupEntry>,
}

pub enum AckResult {
    /// Empty ACK: retransmission cancelled, exchange stays open for separate response.
    EmptyAck,
    /// Piggybacked response: exchange should be completed with this token.
    PiggybackedResponse(Vec<u8>),
    /// MID not recognized.
    Unknown,
}

impl PeerSession {
    pub fn new(peer: SocketAddr) -> Self {
        Self {
            peer,
            exchanges: HashMap::new(),
            mid_to_token: HashMap::new(),
            mid_allocator: MessageIdAllocator::new(),
            token_allocator: TokenAllocator::new(),
            inbound_dedup: HashMap::new(),
        }
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Returns true if this session has no active state and can be evicted.
    pub fn is_idle(&self) -> bool {
        self.exchanges.is_empty()
            && self.mid_to_token.is_empty()
            && self.inbound_dedup.is_empty()
    }

    /// Allocate the next MID for an outbound message.
    pub fn allocate_mid(&mut self) -> u16 {
        self.mid_allocator.allocate()
    }

    /// Allocate the next token for an outbound request.
    pub fn allocate_token(&mut self) -> Vec<u8> {
        self.token_allocator.allocate()
    }

    /// Register an exchange for a NON request (token-based lookup only).
    pub fn insert_exchange(&mut self, token: Vec<u8>, exchange: Exchange) {
        self.exchanges.insert(token, exchange);
    }

    /// Register an exchange for a CON request (both token and MID lookup).
    pub fn insert_con_exchange(&mut self, token: Vec<u8>, mid: u16, exchange: Exchange) {
        self.mid_to_token.insert(mid, token.clone());
        self.exchanges.insert(token, exchange);
    }

    /// Match an inbound response to an outstanding exchange by token.
    /// Delivers the response and removes the exchange.
    pub fn complete_exchange(&mut self, token: &[u8], response: Packet) -> bool {
        if let Some(exchange) = self.exchanges.remove(token) {
            // Also clean up the MID index if present
            if let Some(mid) = exchange.message_id() {
                self.mid_to_token.remove(&mid);
            }
            exchange.complete(response);
            true
        } else {
            false
        }
    }

    /// Handle an inbound ACK by MID.
    pub fn handle_ack(&mut self, mid: u16, packet: &Packet) -> AckResult {
        let token = match self.mid_to_token.remove(&mid) {
            Some(t) => t,
            None => return AckResult::Unknown,
        };

        match packet.header.code {
            MessageClass::Empty => {
                // Empty ACK — cancel retransmission, exchange stays open
                if let Some(exchange) = self.exchanges.get_mut(&token) {
                    exchange.cancel_retransmission();
                }
                AckResult::EmptyAck
            }
            MessageClass::Response(_) => {
                // Piggybacked response — caller should complete the exchange
                AckResult::PiggybackedResponse(token)
            }
            _ => AckResult::Unknown,
        }
    }

    /// Handle an inbound RST by MID. Fails the exchange.
    pub fn handle_rst(&mut self, mid: u16) -> bool {
        if let Some(token) = self.mid_to_token.remove(&mid) {
            if let Some(exchange) = self.exchanges.remove(&token) {
                exchange.fail(super::TransportError::Reset);
                return true;
            }
        }
        false
    }

    /// Check if an inbound MID is a duplicate.
    pub fn is_duplicate(&self, mid: u16) -> bool {
        self.inbound_dedup.contains_key(&mid)
    }

    /// Record an inbound MID in the dedup table.
    pub fn record_inbound_mid(&mut self, mid: u16, now: Instant) {
        self.inbound_dedup.insert(
            mid,
            DedupEntry {
                cached_response: None,
                created: now,
            },
        );
    }

    /// Cache a response for an inbound MID (for re-ACKing duplicate CON).
    pub fn cache_response_for_mid(&mut self, mid: u16, response_bytes: Vec<u8>) {
        if let Some(entry) = self.inbound_dedup.get_mut(&mid) {
            entry.cached_response = Some(response_bytes);
        }
    }

    /// Get cached response for a duplicate CON, if available.
    pub fn get_cached_response(&self, mid: u16) -> Option<&[u8]> {
        self.inbound_dedup
            .get(&mid)
            .and_then(|e| e.cached_response.as_deref())
    }

    /// Collect exchanges that need retransmission. Returns bytes to re-send.
    /// Fails exchanges that have exceeded MAX_RETRANSMIT or NON_LIFETIME.
    pub fn collect_retransmissions(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut retransmits = Vec::new();
        let mut expired_tokens = Vec::new();

        for (token, exchange) in self.exchanges.iter_mut() {
            if exchange.needs_retransmit(now) {
                match exchange.advance_retransmit(now) {
                    Some(bytes) => retransmits.push(bytes),
                    None => expired_tokens.push(token.clone()),
                }
            } else if exchange.is_expired(now) {
                expired_tokens.push(token.clone());
            }
        }

        for token in expired_tokens {
            if let Some(exchange) = self.exchanges.remove(&token) {
                if let Some(mid) = exchange.message_id() {
                    self.mid_to_token.remove(&mid);
                }
                let count = exchange.retransmit_count();
                exchange.fail(super::TransportError::Timeout { retransmits: count });
            }
        }

        retransmits
    }

    /// Evict expired entries from the dedup table.
    pub fn evict_stale_dedup(&mut self, now: Instant) {
        self.inbound_dedup
            .retain(|_, entry| now.duration_since(entry.created) < EXCHANGE_LIFETIME);
    }

    /// Return the soonest deadline across all exchanges (retransmit or NON expiry).
    pub fn next_retransmit_deadline(&self) -> Option<Instant> {
        self.exchanges
            .values()
            .filter_map(|e| e.retransmit_deadline().or(e.expiry_deadline()))
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_idle_lifecycle() {
        tokio::time::pause();
        let peer = "127.0.0.1:5683".parse().unwrap();
        let mut session = PeerSession::new(peer);

        // Fresh session is idle
        assert!(session.is_idle());

        // Recording an inbound MID makes it non-idle
        let now = Instant::now();
        session.record_inbound_mid(1000, now);
        assert!(!session.is_idle());

        // Evicting dedup entries after EXCHANGE_LIFETIME makes it idle again
        tokio::time::advance(EXCHANGE_LIFETIME).await;
        session.evict_stale_dedup(Instant::now());
        assert!(session.is_idle());
    }
}
