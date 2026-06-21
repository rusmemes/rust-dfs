use std::io::Result;

fn main() -> Result<()> {
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["proto/dfs.proto"], &["proto"])?;

    Ok(())
}
