use crate::bufferpool::{
    BitcodeBufferPool, BytesMutPool, PooledBitcodeBuffer, PooledBytesMut, PooledEncodedBytes,
};
use compio_buf::bytes::{Buf, Bytes};
use compio_quic::{Endpoint, Incoming, ServerConfig};
use http::Response;
use std::future::pending;
use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use crate::{Result, RpcError, RpcRequestService, async_trait, bitcode};

/// Configuration for the RPC server
pub struct ServerBuilder {
    /// Server configuration for QUIC
    pub server_config: ServerConfig,
    /// Listen address
    pub listen_addr: String,
    /// Number of worker threads (defaults to physical CPU count)
    pub worker_threads: Option<NonZeroUsize>,
}

impl ServerBuilder {
    pub fn new(server_config: ServerConfig, listen_addr: impl Into<String>) -> Self {
        Self {
            server_config,
            listen_addr: listen_addr.into(),
            worker_threads: None,
        }
    }

    pub fn worker_threads(mut self, threads: NonZeroUsize) -> Self {
        self.worker_threads = Some(threads);
        self
    }

    pub async fn serve<S, Req>(self, service: S) -> io::Result<()>
    where
        S: RpcRequestService<Request = Req> + Clone + Send + 'static,
        Req: for<'a> bitcode::Decode<'a> + Send + 'static,
    {
        let request_count = Arc::new(AtomicU64::new(0));
        let start_time = Arc::new(std::time::Instant::now());

        println!("Starting RPC server on {}", self.listen_addr);

        let endpoint = Endpoint::server(&self.listen_addr, self.server_config).await?;

        loop {
            match endpoint.wait_incoming().await {
                Some(incoming) => {
                    let request_count = request_count.clone();
                    let start_time = start_time.clone();
                    let service = service.clone();

                    compio_runtime::spawn(async move {
                        let bitcode_pool = BitcodeBufferPool::new();
                        let body_pool = BytesMutPool::new();
                        handle_connection(
                            incoming,
                            request_count,
                            service,
                            start_time,
                            bitcode_pool,
                            body_pool,
                        )
                        .await;
                    })
                    .detach();
                }
                None => {
                    eprintln!("endpoint closed; stopping accept loop");
                    break;
                }
            }
        }

        pending::<()>().await;
        Ok(())
    }
}

async fn handle_connection<S, Req>(
    incoming: Incoming,
    request_count: Arc<AtomicU64>,
    service: S,
    start_time: Arc<std::time::Instant>,
    bitcode_pool: BitcodeBufferPool,
    body_pool: BytesMutPool,
) where
    S: RpcRequestService<Request = Req> + 'static,
    Req: for<'a> bitcode::Decode<'a>,
{
    let conn = match incoming.await {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("Failed to establish QUIC connection: {err}");
            return;
        }
    };

    let mut h3_conn = match compio_quic::h3::server::builder()
        .build::<_, Bytes>(conn)
        .await
    {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("Failed to build HTTP/3 connection: {err}");
            return;
        }
    };

    loop {
        let resolver = match h3_conn.accept().await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(err) => {
                eprintln!("Connection accept error: {err}");
                break;
            }
        };

        let request_count = request_count.clone();
        let start_time = start_time.clone();
        let bitcode_pool = bitcode_pool.clone();
        let body_pool = body_pool.clone();
        let service = service.clone();

        compio_runtime::spawn(async move {
            if let Err(err) = handle_request(
                resolver,
                request_count,
                service,
                start_time,
                bitcode_pool,
                body_pool,
            )
            .await
            {
                eprintln!("Request handling error: {err}");
            }
        })
        .detach();
    }
}

async fn handle_request<S, Req>(
    resolver: compio_quic::h3::server::RequestResolver<compio_quic::Connection, Bytes>,
    request_count: Arc<AtomicU64>,
    service: S,
    start_time: Arc<std::time::Instant>,
    bitcode_pool: BitcodeBufferPool,
    body_pool: BytesMutPool,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: RpcRequestService<Request = Req>,
    Req: for<'a> bitcode::Decode<'a>,
{
    let (_req, mut stream) = resolver.resolve_request().await?;

    let mut body = PooledBytesMut::new(body_pool.clone());

    while let Some(mut chunk) = stream.recv_data().await? {
        let chunk_len = chunk.remaining();
        if chunk_len == 0 {
            continue;
        }

        body.reserve(chunk_len);

        while chunk.has_remaining() {
            let data = chunk.chunk();
            body.extend_from_slice(data);
            let len = data.len();
            chunk.advance(len);
        }
    }

    let mut decode_buffer = PooledBitcodeBuffer::new(bitcode_pool.clone());
    let request = decode_buffer
        .decode::<Req>(&body[..])
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    drop(decode_buffer);
    drop(body);

    let count = request_count.fetch_add(1, Ordering::Relaxed) + 1;

    if count.is_multiple_of(10000) {
        let elapsed = start_time.elapsed().as_secs_f64();
        let rps = count as f64 / elapsed;
        println!("Processed: {} requests, RPS: {:.2}", count, rps);
    }

    let response = service.dispatch(request).await;
    let mut encode_buffer = PooledBitcodeBuffer::new(bitcode_pool);
    encode_buffer.encode(&response);
    let response_bytes = PooledEncodedBytes::from_encoded_buffer(encode_buffer);
    let response_len = response_bytes.len();

    let http_response = Response::builder()
        .header("content-type", "application/octet-stream")
        .header("content-length", response_len.to_string())
        .body(())?;

    stream.send_response(http_response).await?;
    stream.send_data(response_bytes.bytes()).await?;
    stream.finish().await?;

    Ok(())
}
