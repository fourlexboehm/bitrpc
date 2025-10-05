use bitrpc::bitcode::{Decode, Encode};

pub mod tls;

#[derive(Encode, Decode, Debug, Clone)]
pub struct AddResponse {
    pub value: u32,
    pub echoed: String,
}

#[bitrpc::service(request = RpcRequest, response = RpcResponse, client = RpcClient)]
pub trait RpcService {
    async fn add(&self, x: u32, y: String) -> bitrpc::Result<AddResponse>;
}
