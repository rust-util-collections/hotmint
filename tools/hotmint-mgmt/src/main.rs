//! hotmint-mgmt — Cluster deployment and management tool for Hotmint.
//!
//! Supports both local multi-node development and distributed deployment.
//!
//! Local cluster commands:
//!   hotmint-mgmt init --validators 4                  # Generate cluster config
//!   hotmint-mgmt start                                # Start all nodes
//!   hotmint-mgmt stop                                 # Stop all nodes
//!   hotmint-mgmt status                               # Show cluster status
//!   hotmint-mgmt clean                                # Clean data dirs
//!   hotmint-mgmt destroy                              # Remove everything
//!   hotmint-mgmt info                                 # Show node keys and peer IDs
//!
//! Remote deployment (git-based sync via SSH):
//!   hotmint-mgmt deploy --hosts hosts.toml [--repo URL] [--branch BRANCH]
//!   hotmint-mgmt exec --hosts hosts.toml -- CMD...    # Run command on all hosts
//!   hotmint-mgmt push --hosts hosts.toml --local F --remote P  # Push file to hosts
//!   hotmint-mgmt pull --hosts hosts.toml --remote P   # Pull file from hosts
//!   hotmint-mgmt logs --hosts hosts.toml [--lines N] [--grep PAT]  # Collect logs
//!   hotmint-mgmt remote-status --hosts hosts.toml     # Show remote node status

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
    /// Deploy cluster to remote machines via SSH + git sync.
    Deploy {
        /// Path to hosts configuration file (TOML).
        #[arg(long)]
        hosts: PathBuf,
        /// Binary crate to build (default: cluster-node).
        #[arg(long, default_value = "cluster-node")]
        package: String,
        /// Git remote URL (default: auto-detect from local origin).
        #[arg(long)]
        repo: Option<String>,
        /// Git branch to deploy (default: current branch).
        #[arg(long)]
        branch: Option<String>,
    },
    /// Show node info (keys, peer IDs).
    Info,

    // --- Remote cluster operations (chaindev-style) ---
    /// Execute a command on all remote hosts.
    Exec {
        /// Path to hosts configuration file (TOML).
        #[arg(long)]
        hosts: PathBuf,
        /// Command to execute on each remote host.
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
    /// Push a local file/directory to all remote hosts.
    Push {
        /// Path to hosts configuration file (TOML).
        #[arg(long)]
        hosts: PathBuf,
        /// Local file or directory to push.
        #[arg(long)]
        local: PathBuf,
        /// Remote destination path.
        #[arg(long)]
        remote: String,
    },
    /// Pull a file from all remote hosts into a local directory.
    Pull {
        /// Path to hosts configuration file (TOML).
        #[arg(long)]
        hosts: PathBuf,
        /// Remote file path to pull.
        #[arg(long)]
        remote: String,
        /// Local directory to store pulled files (suffixed with hostname).
        #[arg(long, default_value = ".")]
        local_dir: PathBuf,
    },
    /// Collect, tail, or grep logs from remote nodes.
    Logs {
        /// Path to hosts configuration file (TOML).
        #[arg(long)]
        hosts: PathBuf,
        /// Number of tail lines (default: 50).
        #[arg(long, default_value_t = 50)]
        lines: u32,
        /// Grep pattern to filter log lines.
        #[arg(long)]
        grep: Option<String>,
        /// Download all logs to a local directory.
        #[arg(long)]
        collect: Option<PathBuf>,
    },
    /// Show status of all remote nodes.
    RemoteStatus {
        /// Path to hosts configuration file (TOML).
        #[arg(long)]
        hosts: PathBuf,
    },
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
            package,
            repo,
            branch,
        } => (|| -> ruc::Result<()> {
            // Auto-detect repo URL from local origin
            let repo_url = match repo {
                Some(url) => url,
                None => {
                    let output = std::process::Command::new("git")
                        .args(["remote", "get-url", "origin"])
                        .output()
                        .map_err(|e| ruc::eg!("failed to get git remote: {}", e))?;
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                }
            };
            // Auto-detect current branch
            let branch_name = match branch {
                Some(b) => b,
                None => {
                    let output = std::process::Command::new("git")
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .output()
                        .map_err(|e| ruc::eg!("failed to get git branch: {}", e))?;
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                }
            };
            remote::deploy(&cli.base_dir, &hosts, &package, &repo_url, &branch_name)
        })(),
        Command::Info => cluster::info(&cli.base_dir),
        Command::Exec { hosts, cmd } => remote::exec_all(&hosts, &cmd.join(" ")),
        Command::Push {
            hosts,
            local,
            remote: dest,
        } => remote::push_all(&hosts, &local, &dest),
        Command::Pull {
            hosts,
            remote: src,
            local_dir,
        } => remote::pull_all(&hosts, &src, &local_dir),
        Command::Logs {
            hosts,
            lines,
            grep,
            collect,
        } => remote::logs(&hosts, lines, grep.as_deref(), collect.as_deref()),
        Command::RemoteStatus { hosts } => remote::remote_status(&cli.base_dir, &hosts),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
