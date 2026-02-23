use crate::transport::ServerInterface;

pub struct CoapServer {
    interface: ServerInterface,
}

impl CoapServer {
    async fn run(&mut self) {
        loop {
            let _ = self.interface.recv_request().await.unwrap();
        }
    }
}
