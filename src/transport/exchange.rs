use std::net::SocketAddr;

use coap_lite::Packet;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::reliability::RetransmitState;
use crate::transport::TransportError;

pub struct Exchange {
    token: Vec<u8>,
    peer: SocketAddr,
    response_tx: oneshot::Sender<Result<Packet, TransportError>>,
    /// MID assigned to the outbound CON. None for NON exchanges.
    message_id: Option<u16>,
    /// Retransmission state, present only for CON exchanges.
    retransmit: Option<RetransmitState>,
}

impl Exchange {
    /// Create an exchange for a CON request (with retransmission tracking).
    pub fn new_con(
        token: Vec<u8>,
        peer: SocketAddr,
        response_tx: oneshot::Sender<Result<Packet, TransportError>>,
        message_id: u16,
        packet_bytes: Vec<u8>,
        now: Instant,
    ) -> Self {
        Self {
            token,
            peer,
            response_tx,
            message_id: Some(message_id),
            retransmit: Some(RetransmitState::new(packet_bytes, now)),
        }
    }

    /// Create an exchange for a NON request (no retransmission).
    pub fn new_non(
        token: Vec<u8>,
        peer: SocketAddr,
        response_tx: oneshot::Sender<Result<Packet, TransportError>>,
    ) -> Self {
        Self {
            token,
            peer,
            response_tx,
            message_id: None,
            retransmit: None,
        }
    }

    pub fn token(&self) -> &[u8] {
        &self.token
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    pub fn message_id(&self) -> Option<u16> {
        self.message_id
    }

    /// Cancel retransmission (called when ACK received). Exchange stays open
    /// for a separate response.
    pub fn cancel_retransmission(&mut self) {
        self.retransmit = None;
    }

    /// Check if this exchange has a pending retransmission that is due.
    pub fn needs_retransmit(&self, now: Instant) -> bool {
        self.retransmit.as_ref().is_some_and(|r| r.is_due(now))
    }

    /// Advance to the next retransmission. Returns the packet bytes to re-send,
    /// or None if MAX_RETRANSMIT has been exceeded.
    pub fn advance_retransmit(&mut self, now: Instant) -> Option<Vec<u8>> {
        let state = self.retransmit.as_mut()?;
        if state.advance(now) {
            Some(state.packet_bytes().to_vec())
        } else {
            None
        }
    }

    /// Return the retransmission deadline, if one is active.
    pub fn retransmit_deadline(&self) -> Option<Instant> {
        self.retransmit.as_ref().map(|r| r.deadline())
    }

    /// Return the retransmit count (for error reporting).
    pub fn retransmit_count(&self) -> u32 {
        self.retransmit
            .as_ref()
            .map(|r| r.retransmit_count())
            .unwrap_or(0)
    }

    /// Deliver a successful response to the waiting caller. Consumes the exchange.
    pub fn complete(self, response: Packet) {
        let _ = self.response_tx.send(Ok(response));
    }

    /// Fail the exchange with an error. Consumes the exchange.
    pub fn fail(self, error: TransportError) {
        let _ = self.response_tx.send(Err(error));
    }
}
