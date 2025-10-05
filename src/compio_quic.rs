use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use compio_buf::bytes::{Buf, Bytes};
use compio_net::ToSocketAddrsAsync;
use compio_quic::ClientBuilder;
use http::{Method, Request, Uri, Version};
use std::fmt::Write;

use crate::{Result, RpcError, RpcTransport};

/// Transport that issues HTTP/3 requests over QUIC using `compio-quic`.
///
/// This transport intentionally skips certificate verification, which keeps the
/// example simple and removes the need for installing local trust anchors.
/// **Never use this in production.**
pub struct CompioQuicTransport {
    uri: Uri,
    host: String,
    port: u16,
    connection: Option<ActiveConnection>,
    // Reusable buffer for header values
    header_buf: String,
}

impl CompioQuicTransport {
    /// Create a transport targeting the provided HTTPS URI.
    pub fn new(uri: String) -> Result<Self> {
        let uri = Uri::from_str(&uri)
            .map_err(|err| RpcError::transport(format!("invalid URI: {err}")))?;

        if uri.scheme_str() != Some("https") {
            return Err(RpcError::transport(
                "compio-quic transport requires an https URI",
            ));
        }

        let host = uri
            .host()
            .ok_or_else(|| RpcError::transport("URI missing host"))?
            .to_string();
        let port = uri.port_u16().unwrap_or(443);

        Ok(Self {
            uri,
            host,
            port,
            connection: None,
            header_buf: String::with_capacity(32),
        })
    }

    async fn open_connection(&self) -> Result<ActiveConnection> {
        let mut resolved = (self.host.as_str(), self.port)
            .to_socket_addrs_async()
            .await
            .map_err(|err| RpcError::transport(format!("resolve error: {err}")))?;
        let remote = resolved
            .next()
            .ok_or_else(|| RpcError::transport("no addresses resolved"))?;

        let bind_ip = if remote.is_ipv6() {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        };

        let endpoint = ClientBuilder::new_with_no_server_verification()
            .with_alpn_protocols(&["h3"])
            .bind(SocketAddr::new(bind_ip, 0))
            .await
            .map_err(|err| RpcError::transport(format!("failed to bind QUIC endpoint: {err}")))?;

        let connecting = endpoint
            .connect(remote, &self.host, None)
            .map_err(|err| RpcError::transport(format!("connect error: {err}")))?;
        let connection = connecting
            .await
            .map_err(|err| RpcError::transport(format!("handshake error: {err}")))?;

        let (mut driver, send_request) = compio_quic::h3::client::new(connection)
            .await
            .map_err(|err| RpcError::transport(format!("HTTP/3 setup failed: {err}")))?;

        let driver_task = compio_runtime::spawn(async move {
            let _result = driver.wait_idle().await;
        });
        driver_task.detach();

        Ok(ActiveConnection {
            _endpoint: endpoint,
            send_request,
        })
    }
}

#[crate::async_trait(?Send)]
impl RpcTransport for CompioQuicTransport {
    async fn call(&mut self, request: Vec<u8>) -> Result<Vec<u8>> {
        let mut connection = match self.connection.take() {
            Some(conn) => conn,
            None => self.open_connection().await?,
        };

        let result =
            perform_request(&mut connection, &self.uri, &mut self.header_buf, request).await;

        match result {
            Ok(body) => {
                self.connection = Some(connection);
                Ok(body)
            }
            Err(err) => {
                // Connection failed, don't store it back
                Err(err)
            }
        }
    }
}

async fn perform_request(
    connection: &mut ActiveConnection,
    uri: &Uri,
    header_buf: &mut String,
    mut request: Vec<u8>,
) -> Result<Vec<u8>> {
    let send_request = &mut connection.send_request;

    request.shrink_to_fit();
    let body = Bytes::from(request);
    let content_length = body.len();

    // Reuse buffer for content-length header
    header_buf.clear();
    let _ = write!(header_buf, "{}", content_length);

    let http_request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .version(Version::HTTP_3)
        .header("content-type", "application/octet-stream")
        .header("content-length", header_buf.as_str())
        .body(())
        .map_err(|err| RpcError::transport(format!("request build error: {err}")))?;

    let mut stream = send_request
        .send_request(http_request)
        .await
        .map_err(|err| RpcError::transport(format!("send request failed: {err}")))?;

    if content_length > 0 {
        stream
            .send_data(body)
            .await
            .map_err(|err| RpcError::transport(format!("send body failed: {err}")))?;
    }

    stream
        .finish()
        .await
        .map_err(|err| RpcError::transport(format!("finish stream failed: {err}")))?;

    let response = stream
        .recv_response()
        .await
        .map_err(|err| RpcError::transport(format!("receive response failed: {err}")))?;

    if !response.status().is_success() {
        return Err(RpcError::transport(format!(
            "unexpected HTTP status: {}",
            response.status()
        )));
    }

    // Pre-allocate reasonable capacity to reduce reallocations
    let mut response_body = Vec::with_capacity(4096);
    while let Some(mut chunk) = stream
        .recv_data()
        .await
        .map_err(|err| RpcError::transport(format!("read body failed: {err}")))?
    {
        let len = chunk.remaining();
        if len == 0 {
            continue;
        }
        let bytes = chunk.copy_to_bytes(len);
        response_body.extend_from_slice(&bytes);
    }

    // Drain trailers if the server sent any, ignoring the contents.
    let _ = stream
        .recv_trailers()
        .await
        .map_err(|err| RpcError::transport(format!("read trailers failed: {err}")))?;

    Ok(response_body)
}

struct ActiveConnection {
    _endpoint: compio_quic::Endpoint,
    send_request: compio_quic::h3::client::SendRequest<compio_quic::h3::OpenStreams, Bytes>,
}
