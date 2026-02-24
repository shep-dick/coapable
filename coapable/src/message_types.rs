use std::net::SocketAddr;

use coap_lite::{CoapOption, ContentFormat, MessageClass, Packet, RequestType, ResponseType};

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

    pub fn not_found() -> Self {
        let mut packet = Packet::new();
        packet
            .header
            .set_code(&MessageClass::Response(ResponseType::NotFound).to_string());
        Self { packet }
    }

    pub fn method_not_allowed() -> Self {
        let mut packet = Packet::new();
        packet
            .header
            .set_code(&MessageClass::Response(ResponseType::MethodNotAllowed).to_string());
        Self { packet }
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

pub struct CoapRequest {
    packet: Packet,
    peer: SocketAddr,
}

impl CoapRequest {
    pub fn from_raw(packet: Packet, peer: SocketAddr) -> Self {
        Self { packet, peer }
    }

    pub fn method(&self) -> RequestType {
        match self.packet.header.code {
            MessageClass::Request(m) => m,
            _ => RequestType::UnKnown,
        }
    }

    /// Reconstructs the URI path from Uri-Path options.
    /// e.g. segments ["sensors", "temp"] → "/sensors/temp"
    pub fn path(&self) -> String {
        match self.packet.get_option(CoapOption::UriPath) {
            Some(segments) => {
                let mut path = String::new();
                for seg in segments {
                    path.push('/');
                    path.push_str(&String::from_utf8_lossy(seg));
                }
                if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                }
            }
            None => "/".to_string(),
        }
    }

    pub fn queries(&self) -> Vec<String> {
        self.packet
            .get_option(CoapOption::UriQuery)
            .map(|qs| {
                qs.iter()
                    .map(|q| String::from_utf8_lossy(q).into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn payload(&self) -> &[u8] {
        &self.packet.payload
    }

    pub fn payload_string(&self) -> Option<&str> {
        std::str::from_utf8(&self.packet.payload).ok()
    }

    pub fn content_format(&self) -> Option<ContentFormat> {
        self.packet.get_content_format()
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    pub fn packet(&self) -> &Packet {
        &self.packet
    }

    pub fn into_packet(self) -> Packet {
        self.packet
    }
}
