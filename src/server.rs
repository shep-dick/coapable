use crate::transport::ServerInterface;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {}

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
