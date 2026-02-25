use std::net::SocketAddr;
use std::time::Duration;

use coap_lite::{
    CoapOption, ContentFormat, MessageClass, MessageType, Packet, RequestType, ResponseType,
};

use crate::CoapRequest;
use crate::message_types::{CoapRequestBuilder, CoapResponse};
use crate::transport::ClientInterface;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Transport(#[from] crate::transport::TransportError),

    #[error(transparent)]
    Message(#[from] crate::message_types::MessageError),

    #[error("response channel closed unexpectedly")]
    ChannelClosed,

    #[error("request timed out")]
    Timeout,
}

type Result<T> = std::result::Result<T, ClientError>;

/// A CoAP client for building and sending requests.
///
/// Cheaply cloneable — can be shared across tasks.
#[derive(Clone)]
pub struct CoapClient {
    interface: ClientInterface,
}

impl CoapClient {
    pub fn new(interface: ClientInterface) -> Self {
        Self { interface }
    }

    pub fn get(&self, peer: SocketAddr) -> ClientRequestBuilder {
        ClientRequestBuilder::new(self.interface.clone(), peer, RequestType::Get)
    }

    pub fn post(&self, peer: SocketAddr) -> ClientRequestBuilder {
        ClientRequestBuilder::new(self.interface.clone(), peer, RequestType::Post)
    }

    pub fn put(&self, peer: SocketAddr) -> ClientRequestBuilder {
        ClientRequestBuilder::new(self.interface.clone(), peer, RequestType::Put)
    }

    pub fn delete(&self, peer: SocketAddr) -> ClientRequestBuilder {
        ClientRequestBuilder::new(self.interface.clone(), peer, RequestType::Delete)
    }
}

pub struct ClientRequest {
    pub(crate) peer: SocketAddr,
    pub(crate) request: CoapRequest,
    pub(crate) timeout: Duration,
}

/// Builds a CoAP request with a fluent API.
///
/// Requests are Confirmable (reliable) by default.
pub struct ClientRequestBuilder {
    interface: ClientInterface,
    peer: SocketAddr,
    base: CoapRequestBuilder,
    timeout: Duration,
}

impl ClientRequestBuilder {
    fn new(interface: ClientInterface, peer: SocketAddr, method: RequestType) -> Self {
        Self {
            interface,
            peer,
            base: CoapRequestBuilder::new(method),
            timeout: Duration::from_secs(5), // @TODO: determine real default timeout
        }
    }

    /// Sets the URI path (e.g. "/sensors/temperature").
    pub fn path(&mut self, path: &str) -> &Self {
        self.base.path(path);
        self
    }

    /// Adds a query parameter encoded as "key=value" in Uri-Query.
    pub fn query(&mut self, key: &str, value: &str) -> &Self {
        self.base.query(&format!("{key}={value}"));
        self
    }

    /// Sets the request payload.
    pub fn payload(&mut self, data: &[u8]) -> &Self {
        self.base.payload(data);
        self
    }

    /// Sets the Content-Format option.
    pub fn content_format(&mut self, cf: ContentFormat) -> &Self {
        self.base.content_format(cf);
        self
    }

    /// Sets the Accept option.
    pub fn accept(&mut self, cf: ContentFormat) -> &Self {
        self.base.accept(cf);
        self
    }

    /// Sets whether the request is Confirmable (default: true).
    ///
    /// Confirmable requests are retransmitted until acknowledged.
    /// Non-confirmable requests are fire-and-forget at the transport layer.
    pub fn confirmable(&mut self, confirm: bool) -> &Self {
        self.base.confirmable(confirm);
        self
    }

    /// Sets the request timeout (default: 30 seconds).
    pub fn timeout(&mut self, timeout: Duration) -> &Self {
        self.timeout = timeout;
        self
    }

    /// Builds the packet, sends the request, and awaits the response.
    pub async fn send(self) -> Result<CoapResponse> {
        let request = self.base.build();

        let client_request = ClientRequest {
            peer: self.peer,
            request,
            timeout: self.timeout,
        };

        let response_rx = self.interface.send_request(client_request).await?;
        let packet = match tokio::time::timeout(self.timeout, response_rx).await {
            Ok(Ok(Ok(packet))) => packet,
            Ok(Ok(Err(transport_err))) => return Err(transport_err.into()),
            Ok(Err(_)) => return Err(ClientError::ChannelClosed),
            Err(_) => return Err(ClientError::Timeout),
        };

        Ok(CoapResponse::from_packet(&packet)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{CoapEndpoint, CoapStack};
    use coap_lite::ResponseType;
    use tokio::net::UdpSocket;

    #[tokio::test]
    async fn clone_client() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client_if, _server, _stack) = CoapStack::start(endpoint).await.unwrap();
        let client = CoapClient::new(client_if);
        let client2 = client.clone();
        let client3 = client.clone();
    }

    #[tokio::test]
    async fn get_with_non_response() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client_if, _server, stack) = CoapStack::start(endpoint).await.unwrap();
        let client = CoapClient::new(client_if);

        // Spawn the request in a task so we can simultaneously play the server role
        let handle = tokio::spawn(async move {
            client
                .get(addr_b)
                .path("/temperature")
                .confirmable(false)
                .send()
                .await
        });

        // Peer B receives the request
        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();

        // Verify path option
        let uri_path = received.get_option(CoapOption::UriPath).unwrap();
        let segments: Vec<&[u8]> = uri_path.iter().map(|v| v.as_slice()).collect();
        assert_eq!(segments, vec![b"temperature"]);

        // Peer B sends a NON response with matching token
        let mut response = Packet::new();
        response.header.set_type(MessageType::NonConfirmable);
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.set_token(received.get_token().to_vec());
        response.payload = b"22.5".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = handle.await.unwrap().unwrap();
        assert_eq!(resp.status(), ResponseType::Content);
        assert_eq!(resp.payload(), b"22.5");
        assert_eq!(resp.payload_string(), Some("22.5"));

        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn post_with_payload_and_content_format() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client_if, _server, stack) = CoapStack::start(endpoint).await.unwrap();
        let client = CoapClient::new(client_if);

        let handle = tokio::spawn(async move {
            client
                .post(addr_b)
                .path("/data")
                .content_format(ContentFormat::ApplicationJSON)
                .payload(b"{\"value\":42}".to_vec())
                .confirmable(false)
                .send()
                .await
        });

        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();

        assert!(matches!(
            received.header.code,
            MessageClass::Request(RequestType::Post)
        ));
        assert_eq!(received.payload, b"{\"value\":42}");
        assert_eq!(
            received.get_content_format(),
            Some(ContentFormat::ApplicationJSON)
        );

        // Send Created response
        let mut response = Packet::new();
        response.header.set_type(MessageType::NonConfirmable);
        response.header.code = MessageClass::Response(ResponseType::Created);
        response.set_token(received.get_token().to_vec());
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = handle.await.unwrap().unwrap();
        assert_eq!(resp.status(), ResponseType::Created);

        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn get_with_query_params() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client_if, _server, stack) = CoapStack::start(endpoint).await.unwrap();
        let client = CoapClient::new(client_if);

        let handle = tokio::spawn(async move {
            client
                .get(addr_b)
                .path("/search")
                .query("type", "sensor")
                .query("limit", "10")
                .confirmable(false)
                .send()
                .await
        });

        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();

        let queries = received.get_option(CoapOption::UriQuery).unwrap();
        let query_strs: Vec<String> = queries
            .iter()
            .map(|v| String::from_utf8(v.clone()).unwrap())
            .collect();
        assert_eq!(query_strs, vec!["type=sensor", "limit=10"]);

        let mut response = Packet::new();
        response.header.set_type(MessageType::NonConfirmable);
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.set_token(received.get_token().to_vec());
        response.payload = b"results".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = handle.await.unwrap().unwrap();
        assert_eq!(resp.status(), ResponseType::Content);
        assert_eq!(resp.payload(), b"results");

        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn con_get_with_piggybacked_response() {
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client_if, _server, stack) = CoapStack::start(endpoint).await.unwrap();
        let client = CoapClient::new(client_if);

        let handle = tokio::spawn(async move {
            client
                .get(addr_b)
                .path("/reliable")
                .send() // confirmable by default
                .await
        });

        let mut buf = [0u8; 2048];
        let (n, _) = sock_b.recv_from(&mut buf).await.unwrap();
        let received = Packet::from_bytes(&buf[..n]).unwrap();
        assert_eq!(received.header.get_type(), MessageType::Confirmable);

        // Piggybacked response: ACK with response code + same MID
        let mut response = Packet::new();
        response.header.set_type(MessageType::Acknowledgement);
        response.header.code = MessageClass::Response(ResponseType::Content);
        response.header.message_id = received.header.message_id;
        response.set_token(received.get_token().to_vec());
        response.payload = b"reliable data".to_vec();
        sock_b
            .send_to(&response.to_bytes().unwrap(), addr_a)
            .await
            .unwrap();

        let resp = handle.await.unwrap().unwrap();
        assert_eq!(resp.status(), ResponseType::Content);
        assert_eq!(resp.payload(), b"reliable data");

        drop(_server);
        stack.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn client_timeout_fires() {
        use tokio::task::yield_now;
        use tokio::time::Duration;

        // Bind two UDP sockets (peer never responds)
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        // Start stack
        let endpoint = CoapEndpoint::start(sock_a).await.unwrap();
        let (client_if, _server, stack) = CoapStack::start(endpoint).await.unwrap();

        // IMPORTANT: let background tasks start polling
        yield_now().await;

        let client = CoapClient::new(client_if);

        // Spawn the request so it is actively polled
        let handle = tokio::spawn(async move {
            client
                .get(addr_b)
                .path("/slow")
                .confirmable(false) // NON request
                .timeout(Duration::from_secs(2))
                .send()
                .await
        });

        let result = handle.await.unwrap();

        assert!(matches!(result, Err(ClientError::Timeout)));

        // Clean shutdown
        drop(_server);
        stack.shutdown().await.unwrap();
    }
}
