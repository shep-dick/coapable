use coap_lite::ResponseType;
use coapable::{
    CoapClient, CoapEndpoint, CoapRequest, CoapResponse, CoapServer, CoapStack, Router, get,
};
use tokio::net::UdpSocket;

#[tokio::main]
async fn main() {
    let sock = UdpSocket::bind("127.0.0.1:5683").await.unwrap();
    let ep = CoapEndpoint::start(sock).await.unwrap();
    let (client_if, server_if, task) = CoapStack::start(ep).await.unwrap();

    let client = CoapClient::new(client_if);

    let router = Router::new().route("/info", get(get_info));
    let server = CoapServer::new(server_if, router);

    let server_task = tokio::spawn(server.run());

    let response = client
        .get("127.0.0.1:5683".parse().unwrap())
        .path("/info")
        .send()
        .await
        .unwrap();

    println!("Received response with code {:?}", response.status());

    server_task.abort();
}

async fn get_info(req: CoapRequest) -> CoapResponse {
    CoapResponse::new(ResponseType::MethodNotAllowed).build()
}
