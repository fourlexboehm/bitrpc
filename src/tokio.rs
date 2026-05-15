use crate::{Result, RpcError, RpcRequestService, RpcTransport};

/// Tokio-compatible HTTP transport for generated bitrpc clients.
///
/// This transport sends bitcode-encoded requests as `application/octet-stream`
/// over normal HTTP/1.1 or HTTP/2 using reqwest's Tokio runtime.
#[derive(Clone)]
pub struct TokioHttpTransport {
    client: reqwest::Client,
    url: String,
}

impl TokioHttpTransport {
    pub fn new(url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
        }
    }

    pub fn with_client(client: reqwest::Client, url: String) -> Self {
        Self { client, url }
    }
}

#[crate::async_trait(?Send)]
impl RpcTransport for TokioHttpTransport {
    async fn call(&mut self, request: Vec<u8>) -> Result<Vec<u8>> {
        let response = self
            .client
            .post(&self.url)
            .header(http::header::CONTENT_TYPE, "application/octet-stream")
            .header(http::header::CONTENT_LENGTH, request.len())
            .body(request)
            .send()
            .await
            .map_err(|err| RpcError::transport(err.to_string()))?;

        if !response.status().is_success() {
            return Err(RpcError::transport(format!(
                "unexpected HTTP status: {}",
                response.status()
            )));
        }

        let body = response
            .bytes()
            .await
            .map_err(|err| RpcError::transport(err.to_string()))?;

        Ok(body.to_vec())
    }
}

/// Dispatch one bitcode-encoded RPC request to a generated service wrapper.
///
/// Tokio HTTP servers can use this from any framework. Decode failures are
/// returned as [`RpcError`] so the caller can decide the HTTP status code.
pub async fn dispatch_bytes<S, Req>(service: &S, request: &[u8]) -> Result<Vec<u8>>
where
    S: RpcRequestService<Request = Req>,
    Req: for<'a> bitcode::Decode<'a>,
{
    let request = bitcode::decode::<Req>(request)?;
    let response = service.dispatch(request).await;
    Ok(bitcode::encode(&response))
}

/// Build a successful octet-stream HTTP response for an encoded RPC response.
pub fn response_from_bytes(bytes: Vec<u8>) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .header(http::header::CONTENT_TYPE, "application/octet-stream")
        .header(http::header::CONTENT_LENGTH, bytes.len())
        .body(bytes)
        .expect("static RPC response headers should be valid")
}
