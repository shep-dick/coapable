use coap_lite::{ContentFormat, ResponseType};
use coapable::{
    CoapClient, CoapEndpoint, CoapRequest, CoapResponse, CoapServer, CoapStack, RequestContext,
    Router, get,
};
use tokio::net::UdpSocket;

#[tokio::main]
async fn main() {
    let sock = UdpSocket::bind("127.0.0.1:5683").await.unwrap();
    let ep = CoapEndpoint::start(sock).await.unwrap();
    let (client_if, server_if, _) = CoapStack::start(ep).await.unwrap();

    let client = CoapClient::new(client_if);

    let router = Router::new().route("/info", get(get_info));
    let server = CoapServer::new(server_if, router);

    tokio::spawn(server.run());

    let response = client
        .get("127.0.0.1:5683".parse().unwrap())
        .path("/info")
        .send()
        .await
        .unwrap();

    println!("Response code: {:?}", response.status());
    println!("    Content Format: {:?}", response.content_format());
    println!("    Payload: {:?}", response.payload());
}

async fn get_info(_: CoapRequest, _: RequestContext) -> CoapResponse {
    CoapResponse::new(ResponseType::Content)
        .content_format(ContentFormat::TextPlain)
        .payload(b"response")
        .build()
}
