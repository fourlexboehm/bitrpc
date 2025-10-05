pub use async_trait::async_trait;
pub use bitcode;

pub use bitrpc_macros::service;

use bitcode::{Decode, Encode};

#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
pub enum RpcError {
    Handler { message: String },
    Decode { message: String },
    Transport { message: String },
    Unexpected { expected: String, actual: String },
}

pub type Result<T, E = RpcError> = core::result::Result<T, E>;

impl RpcError {
    pub fn handler(message: impl Into<String>) -> Self {
        Self::Handler {
            message: message.into(),
        }
    }

    pub fn decode(message: impl Into<String>) -> Self {
        Self::Decode {
            message: message.into(),
        }
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
        }
    }

    pub fn unexpected(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        Self::Unexpected {
            expected: expected.into(),
            actual: actual.into(),
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl From<bitcode::Error> for RpcError {
    fn from(err: bitcode::Error) -> Self {
        Self::Decode {
            message: err.to_string(),
        }
    }
}

impl std::error::Error for RpcError {}

#[async_trait(?Send)]
pub trait RpcTransport {
    async fn call(&mut self, request: Vec<u8>) -> Result<Vec<u8>>;
}

pub type DecodeError = bitcode::Error;

#[cfg(feature = "cyper")]
pub mod cyper;

#[cfg(feature = "compio-quic")]
pub mod compio_quic;

#[cfg(feature = "compio-server")]
mod bufferpool;

#[cfg(feature = "compio-server")]
pub mod compio_server;
#[cfg(feature = "compio-server")]
pub use compio_server::{RpcRequestService, ServerBuilder};
