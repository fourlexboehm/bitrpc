# bitrpc

A simple Rust RPC library using [bitcode](https://github.com/SoftbearStudios/bitcode) serialization with optional high-performance transports.

## Features

- **Runtime-agnostic core** - No async runtime dependency, bring your own transport
- **Procedural macros** to generate type-safe RPC client/server code
- **Pluggable transports**:
  - `cyper` - HTTP3 client transport
  - `compio-quic` - QUIC/HTTP3 with io_uring via compio
  - `compio-server` - Multi-threaded server with io_uring

## Usage

### Define your service

```rust
use bitrpc::bitcode::{Encode, Decode};

#[derive(Encode, Decode)]
pub struct AddResponse {
    pub value: u32,
}

#[bitrpc::service(request = RpcRequest, response = RpcResponse, client = RpcClient)]
pub trait RpcService {
    async fn add(&self, x: u32, y: u32) -> bitrpc::Result<AddResponse>;
}
```

### Implement the server

```rust
#[derive(Clone)]
struct Handler;

#[bitrpc::async_trait]
impl RpcService for Handler {
    async fn add(&self, x: u32, y: u32) -> bitrpc::Result<AddResponse> {
        Ok(AddResponse { value: x + y })
    }
}

// With compio-server feature:
let server = bitrpc::ServerBuilder::new(quic_config, "localhost:4433")
    .serve(RpcRequestServiceWrapper(Handler))
    .await?;
```

### Call from client

```rust
use bitrpc::cyper::CyperTransport;

let mut client = RpcClient::new(CyperTransport::new("https://localhost:4433"));
let response = client.add(10, 20).await?;
```

### Implement custom transport

The core library is runtime-agnostic. Implement the `RpcTransport` trait for any async runtime:

```rust
use bitrpc::{RpcTransport, Result};

struct MyTransport { /* ... */ }

#[bitrpc::async_trait(?Send)]
impl RpcTransport for MyTransport {
    async fn call(&mut self, request: Vec<u8>) -> Result<Vec<u8>> {
        // Your transport logic here
    }
}
```

## Features

Enable optional transports in `Cargo.toml`:

```toml
[dependencies]
bitrpc = { version = "0.2", features = ["cyper"] }           # HTTP3 client
bitrpc = { version = "0.2", features = ["compio-server"] }   # io_uring server
```

## License

MIT
