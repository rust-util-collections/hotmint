//! hotmint-mgmt — Cluster deployment and management tool for Hotmint.
//!
//! Supports both local multi-node development and distributed deployment.
//!
//! Usage:
//!   hotmint-mgmt init --validators 4                  # Generate cluster config
//!   hotmint-mgmt start                                # Start all nodes
//!   hotmint-mgmt stop                                 # Stop all nodes
//!   hotmint-mgmt status                               # Show cluster status
//!   hotmint-mgmt clean                                # Clean data dirs
//!   hotmint-mgmt deploy --hosts hosts.toml            # Deploy to remote machines

mod cluster;
mod local;
mod remote;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(
    name = "hotmint-mgmt",
    about = "Hotmint cluster deployment and management tool"
)]
struct Cli {
    /// Base directory for cluster state.
    #[arg(long, default_value = "/tmp/hotmint-cluster")]
    base_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new cluster: generate keys, genesis, and per-node configs.
    Init {
        /// Number of validators.
        #[arg(long, short = 'n', default_value_t = 4)]
        validators: u32,
        /// Chain ID for the genesis.
        #[arg(long, default_value = "hotmint-testnet")]
        chain_id: String,
        /// Base P2P port (each validator gets base + validator_id).
        #[arg(long, default_value_t = 20000)]
        p2p_port: u16,
        /// Base RPC port (each validator gets base + validator_id).
        #[arg(long, default_value_t = 21000)]
        rpc_port: u16,
        /// IP address to bind/connect (default 127.0.0.1 for local).
        #[arg(long, default_value = "127.0.0.1")]
        bind_ip: String,
    },
    /// Start all (or specific) validator nodes.
    Start {
        /// Specific node ID to start (default: all).
        #[arg(long)]
        node: Option<u32>,
        /// Binary to use (default: cluster-node from workspace).
        #[arg(long)]
        binary: Option<PathBuf>,
    },
    /// Stop all (or specific) validator nodes.
    Stop {
        /// Specific node ID to stop (default: all).
        #[arg(long)]
        node: Option<u32>,
    },
    /// Show cluster status via RPC.
    Status,
    /// Clean data directories (preserve config).
    Clean,
    /// Destroy the entire cluster (remove everything).
    Destroy,
    /// Deploy cluster to remote machines via SSH.
    Deploy {
        /// Path to hosts configuration file (TOML).
        #[arg(long)]
        hosts: PathBuf,
        /// Path to hotmint source directory.
        #[arg(long, default_value = ".")]
        source: PathBuf,
        /// Binary crate to build (default: cluster-node).
        #[arg(long, default_value = "cluster-node")]
        package: String,
    },
    /// Show node info (keys, peer IDs).
    Info,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Init {
            validators,
            chain_id,
            p2p_port,
            rpc_port,
            bind_ip,
        } => cluster::init_cluster(
            &cli.base_dir,
            validators,
            &chain_id,
            p2p_port,
            rpc_port,
            &bind_ip,
        ),
        Command::Start { node, binary } => local::start(&cli.base_dir, node, binary.as_deref()),
        Command::Stop { node } => local::stop(&cli.base_dir, node),
        Command::Status => local::status(&cli.base_dir),
        Command::Clean => cluster::clean(&cli.base_dir),
        Command::Destroy => cluster::destroy(&cli.base_dir),
        Command::Deploy {
            hosts,
            source,
            package,
        } => remote::deploy(&cli.base_dir, &hosts, &source, &package),
        Command::Info => cluster::info(&cli.base_dir),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
