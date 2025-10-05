use bitrpc::ServerBuilder;
use compio::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use compio_quic::ServerBuilder as QuicServerBuilder;

use compio_h3_rpc::{tls, AddResponse, RpcRequestServiceWrapper, RpcService};

const LISTEN_ADDR: &str = "localhost:4433";

#[derive(Clone)]
struct Handlers;

#[bitrpc::async_trait]
impl RpcService for Handlers {
    async fn add(&self, x: u32, y: String) -> bitrpc::Result<AddResponse> {
        Ok(AddResponse {
            value: x + 1,
            echoed: y,
        })
    }
}

#[compio_macros::main]
async fn main() {
    let cert = CertificateDer::from_slice(tls::CERT_DER).into_owned();
    let key = PrivateKeyDer::try_from(tls::KEY_DER).expect("invalid private key");

    let server_config = QuicServerBuilder::new_with_single_cert(vec![cert], key)
        .unwrap()
        .with_key_log()
        .with_alpn_protocols(&["h3"])
        .build();

    ServerBuilder::new(server_config, LISTEN_ADDR)
        .serve(RpcRequestServiceWrapper(Handlers))
        .await
        .expect("server failed");
}
