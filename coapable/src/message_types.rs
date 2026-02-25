use coap_lite::{
    CoapOption, ContentFormat, MessageClass, MessageType, Packet, RequestType, ResponseType,
    option_value::OptionValueU16,
};

type Result<T> = std::result::Result<T, MessageError>;

#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("packet is not a response")]
    NotAResponse,

    #[error("packet is not a request")]
    NotARequest,

    #[error("invalid URI path")]
    InvalidUriPath,
}

pub struct CoapResponseBuilder {
    response_type: ResponseType,
    content_format: Option<ContentFormat>,
    payload: Vec<u8>,
}

impl CoapResponseBuilder {
    pub fn new(response_type: ResponseType) -> Self {
        Self {
            response_type,
            content_format: None,
            payload: Vec::new(),
        }
    }

    pub fn content_format(mut self, content_format: ContentFormat) -> Self {
        self.content_format = Some(content_format);
        self
    }

    pub fn payload(mut self, payload: &[u8]) -> Self {
        self.payload = payload.to_vec();
        self
    }

    pub fn build(self) -> CoapResponse {
        CoapResponse {
            response_type: self.response_type,
            content_format: self.content_format,
            payload: self.payload,
        }
    }
}

/// A CoAP response received from a peer.
pub struct CoapResponse {
    response_type: ResponseType,
    content_format: Option<ContentFormat>,
    payload: Vec<u8>,
}

impl CoapResponse {
    pub fn new(response_type: ResponseType) -> CoapResponseBuilder {
        CoapResponseBuilder::new(response_type)
    }

    pub fn from_packet(packet: &Packet) -> Result<Self> {
        let response_type = match packet.header.code {
            MessageClass::Response(code) => Ok(code),
            _ => Err(MessageError::NotAResponse),
        }?;

        let content_format = packet.get_content_format();

        let payload = packet.payload.to_owned();

        Ok(Self {
            response_type,
            content_format,
            payload,
        })
    }

    pub fn status(&self) -> ResponseType {
        self.response_type
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

    pub fn to_packet(self, token: Vec<u8>, mid: u16) -> Packet {
        let mut pkt = Packet::new();

        pkt.set_token(token);
        pkt.header.message_id = mid;

        pkt.header
            .set_code(&MessageClass::Response(self.response_type).to_string());

        pkt.payload = self.payload;

        if let Some(cf) = self.content_format {
            pkt.set_content_format(cf);
        }

        pkt
    }
}

pub struct CoapRequestBuilder {
    method: RequestType,
    path: String,
    queries: Vec<String>,
    content_format: Option<ContentFormat>,
    payload: Vec<u8>,
    accept: Option<ContentFormat>,
    confirmable: bool,
}

impl CoapRequestBuilder {
    pub fn new(method: RequestType) -> Self {
        Self {
            method,
            path: String::new(),
            queries: Vec::new(),
            content_format: None,
            payload: Vec::new(),
            accept: None,
            confirmable: true,
        }
    }

    pub fn path(mut self, path: &str) -> Self {
        self.path.push_str(path);
        self
    }

    pub fn query(mut self, query: &str) -> Self {
        self.queries.push(query.to_string());
        self
    }

    pub fn content_format(mut self, content_format: ContentFormat) -> Self {
        self.content_format = Some(content_format);
        self
    }

    pub fn payload(mut self, payload: &[u8]) -> Self {
        self.payload = payload.to_vec();
        self
    }

    pub fn accept(mut self, content_format: ContentFormat) -> Self {
        self.accept = Some(content_format);
        self
    }

    pub fn confirmable(mut self, confirmable: bool) -> Self {
        self.confirmable = confirmable;
        self
    }

    pub fn build(self) -> CoapRequest {
        CoapRequest {
            method: self.method,
            path: self.path,
            queries: self.queries,
            content_format: self.content_format,
            payload: self.payload,
            accept: self.accept,
            confirmable: self.confirmable,
        }
    }
}

pub struct CoapRequest {
    method: RequestType,
    path: String,
    queries: Vec<String>,
    content_format: Option<ContentFormat>,
    payload: Vec<u8>,
    accept: Option<ContentFormat>,
    confirmable: bool,
}

impl CoapRequest {
    pub fn new(method: RequestType) -> CoapRequestBuilder {
        CoapRequestBuilder::new(method)
    }

    pub fn from_packet(packet: &Packet) -> Result<Self> {
        let method = match packet.header.code {
            MessageClass::Request(code) => Ok(code),
            _ => Err(MessageError::NotARequest),
        }?;

        let path = packet
            .get_option(CoapOption::UriPath)
            .map(|opts| {
                let segments: Vec<&str> = opts
                    .iter()
                    .filter_map(|v| std::str::from_utf8(v).ok())
                    .collect();
                format!("/{}", segments.join("/"))
            })
            .unwrap_or_default();

        let queries = packet
            .get_option(CoapOption::UriQuery)
            .map(|opts| {
                opts.iter()
                    .filter_map(|v| String::from_utf8(v.clone()).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let content_format = packet.get_content_format();

        let accept = match packet.get_option(CoapOption::Accept) {
            // I don't know why coap_lite uses a LinkedList here... RFC 7252 implies only one CF is contained in the Accept option.
            // For now we'll just use the front element of the list.
            Some(l) => match l.front() {
                Some(raw) => {
                    let mut bytes = [0u8; 8];
                    bytes.clone_from_slice(&raw);
                    let cf_code = usize::from_be_bytes(bytes);
                    ContentFormat::try_from(cf_code).ok()
                }
                None => None,
            },
            None => None,
        };

        let payload = packet.payload.to_owned();
        let confirmable = packet.header.get_type() == MessageType::Confirmable;

        Ok(Self {
            method,
            path,
            queries,
            content_format,
            payload,
            accept,
            confirmable,
        })
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

    pub fn accept(&self) -> Option<ContentFormat> {
        self.accept
    }

    pub fn confirmable(&self) -> bool {
        self.confirmable
    }

    pub fn to_packet(self, token: Vec<u8>, mid: u16) -> Packet {
        let mut pkt = Packet::new();

        pkt.set_token(token);
        pkt.header.message_id = mid;

        pkt.header.code = MessageClass::Request(self.method);

        if self.path.as_bytes() != [] {
            for segment in self.path.split('/').filter(|s| !s.is_empty()) {
                pkt.add_option(CoapOption::UriPath, segment.as_bytes().to_vec());
            }
        }

        for query in &self.queries {
            pkt.add_option(CoapOption::UriQuery, query.as_bytes().to_vec());
        }

        if let Some(cf) = self.content_format {
            pkt.set_content_format(cf);
        }

        if let Some(cf) = self.accept {
            let value = usize::from(cf) as u16;
            pkt.add_option_as(CoapOption::Accept, OptionValueU16(value));
        }

        if self.payload.as_slice() != [] {
            pkt.payload = self.payload
        }

        pkt
    }
}
