use std::{net::SocketAddr, str::FromStr};

use bytes::Bytes;
use coap_lite::{
    CoapOption, ContentFormat, MessageClass, MessageType, Packet, RequestType, ResponseType,
};

#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("packet is not a response")]
    NotAResponse,
}

pub struct CoapResponseBuilder {
    response_type: ResponseType,
    token: Vec<u8>,
    content_format: Option<ContentFormat>,
    payload: Vec<u8>,
}

impl CoapResponseBuilder {
    pub fn new(response_type: ResponseType, token: Vec<u8>) -> Self {
        Self {
            response_type,
            token,
            content_format: None,
            payload: Vec::new(),
        }
    }

    pub fn content_format(&mut self, content_format: ContentFormat) -> &Self {
        self.content_format = Some(content_format);
        self
    }

    pub fn payload(&mut self, payload: &[u8]) -> &Self {
        self.payload.clone_from_slice(payload);
        self
    }

    pub fn build(self) -> CoapResponse {
        CoapResponse {
            response_type: self.response_type,
            token: self.token,
            content_format: self.content_format,
            payload: self.payload,
        }
    }
}

/// A CoAP response received from a peer.
pub struct CoapResponse {
    response_type: ResponseType,
    token: Vec<u8>,
    content_format: Option<ContentFormat>,
    payload: Vec<u8>,
}

impl CoapResponse {
    // Constructor
    pub fn new(response_type: ResponseType, token: Vec<u8>) -> CoapResponseBuilder {
        CoapResponseBuilder::new(response_type, token)
    }

    // From raw packet constructor
    pub fn from_packet(packet: &Packet) -> Result<Self, MessageError> {
        let response_type = match packet.header.code {
            MessageClass::Response(code) => Ok(code),
            _ => Err(MessageError::NotAResponse),
        }?;

        let token = packet.get_token().to_vec();

        let content_format = packet.get_content_format();

        let payload = packet.payload.to_owned();

        Ok(Self {
            response_type,
            token,
            content_format,
            payload,
        })
    }

    /// Returns the response status code.
    pub fn status(&self) -> ResponseType {
        self.response_type
    }

    /// Returns the response payload as a slice.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns the payload as a UTF-8 string, if valid.
    pub fn payload_string(&self) -> Option<&str> {
        std::str::from_utf8(&self.payload).ok()
    }

    /// Returns the Content-Format of the response, if present.
    pub fn content_format(&self) -> Option<ContentFormat> {
        self.content_format
    }
}

pub struct CoapRequestBuilder {
    method: RequestType,
    path: String,
    queries: Vec<String>,
    content_format: Option<ContentFormat>,
    payload: Vec<u8>,
}

impl CoapRequestBuilder {
    pub fn new(method: RequestType) -> Self {
        Self {
            method,
            path: String::new(),
            queries: Vec::new(),
            content_format: None,
            payload: Vec::new(),
        }
    }

    pub fn path(&mut self, path: &str) -> &Self {
        self.path.push_str(path);
        self
    }

    pub fn query(&mut self, query: &str) -> &Self {
        self.queries.push(query.to_string());
        self
    }

    pub fn content_format(&mut self, content_format: ContentFormat) -> &Self {
        self.content_format = Some(content_format);
        self
    }

    pub fn payload(&mut self, payload: &[u8]) -> &Self {
        self.payload.clone_from_slice(payload);
        self
    }

    pub fn build(self) -> CoapRequest {
        CoapRequest {
            method: self.method,
            path: self.path,
            queries: self.queries,
            content_format: self.content_format,
            payload: self.payload,
        }
    }
}

pub struct CoapRequest {
    method: RequestType,
    path: String,
    queries: Vec<String>,
    content_format: Option<ContentFormat>,
    payload: Vec<u8>,
}

impl CoapRequest {
    pub fn new(method: RequestType) -> CoapRequestBuilder {
        CoapRequestBuilder::new(method)
    }

    pub fn method(&self) -> RequestType {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn queries(&self) -> Option<&Vec<String>> {
        match self.queries.as_slice() {
            [] => None,
            _ => Some(&self.queries),
        }
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn payload_string(&self) -> Option<&str> {
        std::str::from_utf8(&self.payload).ok()
    }

    pub fn content_format(&self) -> Option<ContentFormat> {
        self.content_format
    }
}
