use coap_lite::{ContentFormat, MessageClass, Packet, ResponseType};

#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("packet is not a response")]
    NotAResponse,
}

/// A CoAP response received from a peer.
pub struct CoapResponse {
    packet: Packet,
}

impl CoapResponse {
    pub fn from_packet(packet: Packet) -> Result<Self, MessageError> {
        match packet.header.code {
            MessageClass::Response(_) => Ok(Self { packet }),
            _ => Err(MessageError::NotAResponse),
        }
    }

    pub fn not_implemented() -> Self {
        let mut packet = Packet::new();
        packet
            .header
            .set_code(&MessageClass::Response(ResponseType::NotImplemented).to_string());
        Self { packet }
    }

    /// Returns the response status code.
    pub fn status(&self) -> ResponseType {
        match self.packet.header.code {
            MessageClass::Response(code) => code,
            _ => unreachable!("non-response packet in CoapResponse"),
        }
    }

    /// Returns the response payload as bytes.
    pub fn payload(&self) -> &[u8] {
        &self.packet.payload
    }

    /// Returns the payload as a UTF-8 string, if valid.
    pub fn payload_string(&self) -> Option<&str> {
        std::str::from_utf8(&self.packet.payload).ok()
    }

    /// Returns the Content-Format of the response, if present.
    pub fn content_format(&self) -> Option<ContentFormat> {
        self.packet.get_content_format()
    }

    /// Consumes the response and returns the underlying packet.
    pub fn into_packet(self) -> Packet {
        self.packet
    }
}
