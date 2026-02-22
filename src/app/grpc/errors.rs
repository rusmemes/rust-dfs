use std::net::AddrParseError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GrpcServerError {
    #[error("Failed to parse address: {0}")]
    AddressParse(#[from] AddrParseError),
    #[error("GRPC transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
}
