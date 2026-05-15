use crate::{Result, RpcError, RpcTransport};
use cyper::Client;
use http::Version;
use std::rc::Rc;

#[derive(Clone)]
pub struct CyperTransport {
    client: Rc<Client>,
    url: String,
}

impl CyperTransport {
    pub fn new(url: String) -> Self {
        Self {
            client: Rc::new(Client::new()),
            url,
        }
    }
}

#[crate::async_trait(?Send)]
impl RpcTransport for CyperTransport {
    async fn call(&mut self, request: Vec<u8>) -> Result<Vec<u8>> {
        let content_length = request.len();

        let builder = self
            .client
            .post(&self.url)
            .map_err(|err| RpcError::transport(err.to_string()))?;

        let builder = builder
            .version(Version::HTTP_3)
            .header("content-type", "application/octet-stream")
            .map_err(|err| RpcError::transport(err.to_string()))?;

        let builder = builder
            .header("content-length", content_length.to_string())
            .map_err(|err| RpcError::transport(err.to_string()))?;

        let response = builder
            .body(request)
            .send()
            .await
            .map_err(|err| RpcError::transport(err.to_string()))?;

        let body = response
            .bytes()
            .await
            .map_err(|err| RpcError::transport(err.to_string()))?;

        Ok(body.to_vec())
    }
}
