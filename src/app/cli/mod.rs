use clap_derive::{Parser, Subcommand};
use homedir::my_home;
use log::warn;
use std::env;
use std::path::PathBuf;

fn default_base_path() -> PathBuf {
    match my_home() {
        Ok(Some(path)) => path,
        _ => {
            warn!("No Home directory found. Using current directory.");
            env::current_dir().expect("No Current directory found")
        }
    }
    .join(".dfs")
}

#[derive(Subcommand)]
pub enum Command {
    Start,
}

#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[arg(short, long, default_value = default_base_path().into_os_string())]
    pub base_path: PathBuf,
    #[arg(short, long, default_value_t = 9999)]
    pub grpc_port: u16,
    #[command(subcommand)]
    pub command: Command,
}
