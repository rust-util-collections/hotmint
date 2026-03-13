//! Remote (distributed) deployment via SSH.

use ruc::*;
use std::path::Path;
use std::process;

use crate::cluster::ClusterState;
use serde::{Deserialize, Serialize};

/// Remote host configuration (from hosts.toml).
#[derive(Debug, Serialize, Deserialize)]
pub struct HostsConfig {
    pub hosts: Vec<HostEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HostEntry {
    /// Validator ID assigned to this host.
    pub validator_id: u64,
    /// SSH address: user@hostname or user@ip.
    pub ssh: String,
    /// External IP for P2P (if different from SSH host).
    pub external_ip: Option<String>,
    /// Remote home directory for the node.
    pub home: Option<String>,
    /// Whether the host has rsync (false = use tar|ssh).
    #[serde(default = "default_true")]
    pub has_rsync: bool,
}

fn default_true() -> bool {
    true
}

impl HostsConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path).c(d!("read hosts.toml"))?;
        toml::from_str(&contents).c(d!("parse hosts.toml"))
    }
}

/// Run an SSH command and return stdout.
fn ssh_exec(target: &str, cmd: &str) -> Result<String> {
    let output = process::Command::new("ssh")
        .args(["-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes"])
        .arg(target)
        .arg(cmd)
        .output()
        .c(d!("ssh to {}", target))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eg!("ssh {} failed: {}", target, stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Sync source code to a remote host.
fn sync_source(source: &Path, host: &HostEntry, remote_dir: &str) -> Result<()> {
    let target = &host.ssh;
    if host.has_rsync {
        let status = process::Command::new("rsync")
            .args([
                "-az",
                "--delete",
                "--exclude",
                "target/",
                "--exclude",
                ".git/",
            ])
            .arg(format!("{}/", source.display()))
            .arg(format!("{}:{}/", target, remote_dir))
            .status()
            .c(d!("rsync to {}", target))?;
        if !status.success() {
            return Err(eg!("rsync to {} failed", target));
        }
    } else {
        // Use tar|ssh for hosts without rsync (e.g., FreeBSD)
        let cmd = format!(
            "rm -rf {remote_dir} && mkdir -p {remote_dir} && cd {remote_dir} && tar xzf -"
        );
        let child = process::Command::new("sh")
            .args([
                "-c",
                &format!(
                    "tar czf - -C {} --exclude target --exclude .git . | ssh -o StrictHostKeyChecking=no -o BatchMode=yes {} '{}'",
                    source.display(),
                    target,
                    cmd,
                ),
            ])
            .status()
            .c(d!("tar|ssh to {}", target))?;
        if !child.success() {
            return Err(eg!("tar|ssh to {} failed", target));
        }
    }
    Ok(())
}

pub fn deploy(
    base_dir: &Path,
    hosts_path: &Path,
    source: &Path,
    package: &str,
) -> Result<()> {
    let state = ClusterState::load(base_dir)?;
    let hosts = HostsConfig::load(hosts_path)?;

    println!(
        "Deploying cluster '{}' to {} hosts",
        state.chain_id,
        hosts.hosts.len()
    );

    for host in &hosts.hosts {
        let vid = host.validator_id;
        let remote_home = host
            .home
            .clone()
            .unwrap_or_else(|| format!("~/hotmint-v{}", vid));
        let remote_src = "~/hotmint";

        // Find the corresponding validator entry
        let _v = state
            .validators
            .iter()
            .find(|v| v.id == vid)
            .ok_or_else(|| eg!("validator {} not found in cluster state", vid))?;

        println!("\n--- V{} ({}) ---", vid, host.ssh);

        // 1. Sync source code
        println!("  Syncing source to {}:{}...", host.ssh, remote_src);
        sync_source(source, host, remote_src)?;

        // 2. Sync node config
        let local_config = base_dir.join(format!("v{}", vid)).join("config");
        if local_config.exists() {
            println!("  Syncing config to {}:{}...", host.ssh, remote_home);
            ssh_exec(
                &host.ssh,
                &format!("mkdir -p {}/config", remote_home),
            )?;
            // Copy config files via scp
            for file in ["config.toml", "genesis.json", "priv_validator_key.json", "node_key.json"]
            {
                let local_file = local_config.join(file);
                if local_file.exists() {
                    let status = process::Command::new("scp")
                        .args(["-o", "StrictHostKeyChecking=no", "-o", "BatchMode=yes"])
                        .arg(local_file.to_str().unwrap())
                        .arg(format!("{}:{}/config/{}", host.ssh, remote_home, file))
                        .status()
                        .c(d!("scp config"))?;
                    if !status.success() {
                        return Err(eg!("scp {} to {} failed", file, host.ssh));
                    }
                }
            }
        }

        // 3. Build on remote
        println!(
            "  Building {} on {}...",
            package, host.ssh
        );
        let build_output = ssh_exec(
            &host.ssh,
            &format!(
                "cd {} && cargo build --release -p {} 2>&1 | tail -3",
                remote_src, package
            ),
        )?;
        println!("  {}", build_output.trim());

        // 4. Start the node
        println!("  Starting V{}...", vid);
        let bin_path = format!("{}/target/release/{}", remote_src, package);
        ssh_exec(
            &host.ssh,
            &format!(
                "pkill -f '{} --home {}' 2>/dev/null; sleep 1; nohup {} --home {} > /tmp/hotmint-v{}.log 2>&1 &",
                package, remote_home, bin_path, remote_home, vid,
            ),
        )?;
        println!("  V{}: started on {}", vid, host.ssh);
    }

    println!("\nDeployment complete.");
    Ok(())
}
