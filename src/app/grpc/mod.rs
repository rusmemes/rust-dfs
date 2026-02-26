pub mod server;
pub mod errors;

pub mod dfs_grpc {
    tonic::include_proto!("dfs_grpc");
}
