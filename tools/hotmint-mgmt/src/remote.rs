//! Remote (distributed) deployment via SSH + git-based sync.

use ruc::*;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process;

use crate::cluster::ClusterState;
use serde::{Deserialize, Serialize};

/// Shell-escape a string by wrapping it in single quotes, with proper handling
/// of embedded single quotes.  e.g. `foo'bar` -> `'foo'\''bar'`
///
/// Leading `~/` is replaced with `$HOME/` so that tilde expansion works
/// even inside single quotes (the `$HOME` is spliced outside the quotes).
fn shell_escape(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        // $HOME must be outside quotes for expansion
        format!("\"$HOME\"/'{}'", rest.replace('\'', "'\\''"))
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

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
}

impl HostsConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path).c(d!("read hosts.toml"))?;
        toml::from_str(&contents).c(d!("parse hosts.toml"))
    }
}

/// Run an SSH command and return stdout.
fn ssh_exec(target: &str, cmd: &str) -> Result<String> {
    let output = process::Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
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

/// Ensure remote has the repo at the correct commit.
fn git_sync(
    target: &str,
    repo_url: &str,
    branch: &str,
    expected_commit: &str,
    remote_dir: &str,
) -> Result<()> {
    // Check if repo exists
    let has_repo = ssh_exec(
        target,
        &format!(
            "test -d {}/.git && echo yes || echo no",
            shell_escape(remote_dir)
        ),
    )?
    .trim()
    .to_string();

    if has_repo == "no" {
        // First time: clone
        println!("    Cloning repository...");
        ssh_exec(
            target,
            &format!(
                "git clone {} {}",
                shell_escape(repo_url),
                shell_escape(remote_dir)
            ),
        )?;
    } else {
        // Fetch latest
        ssh_exec(
            target,
            &format!("cd {} && git fetch origin", shell_escape(remote_dir)),
        )?;
    }

    // Checkout and reset to exact commit
    ssh_exec(
        target,
        &format!(
            "cd {} && git checkout {} && git reset --hard origin/{}",
            shell_escape(remote_dir),
            shell_escape(branch),
            shell_escape(branch)
        ),
    )?;

    // Verify commit matches
    let remote_commit = ssh_exec(
        target,
        &format!("cd {} && git rev-parse HEAD", shell_escape(remote_dir)),
    )?
    .trim()
    .to_string();

    if remote_commit != expected_commit {
        return Err(eg!(
            "commit mismatch on {}: local={} remote={}",
            target,
            expected_commit,
            remote_commit
        ));
    }

    Ok(())
}

/// Write file content to a remote path by piping through ssh stdin.
fn ssh_write_file(target: &str, remote_path: &str, content: &str) -> Result<()> {
    let mut child = process::Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .arg(target)
        .arg(format!("cat > {}", shell_escape(remote_path)))
        .stdin(process::Stdio::piped())
        .spawn()
        .c(d!("ssh to {}", target))?;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(content.as_bytes())
        .c(d!("write file content"))?;
    let status = child.wait().c(d!("wait ssh"))?;
    if !status.success() {
        return Err(eg!("ssh_write_file to {} failed", target));
    }
    Ok(())
}

/// Get the current local HEAD commit hash.
fn get_local_commit() -> Result<String> {
    let output = process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .c(d!("get local git commit"))?;
    if !output.status.success() {
        return Err(eg!("failed to get local git commit"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn deploy(
    base_dir: &Path,
    hosts_path: &Path,
    package: &str,
    repo_url: &str,
    branch: &str,
) -> Result<()> {
    let state = ClusterState::load(base_dir)?;
    let hosts = HostsConfig::load(hosts_path)?;

    // Get local commit hash for verification
    let local_commit = get_local_commit()?;

    println!(
        "Deploying {} (branch: {}, commit: {})",
        state.chain_id,
        branch,
        &local_commit[..8]
    );

    for host in &hosts.hosts {
        let vid = host.validator_id;
        let remote_home = host
            .home
            .clone()
            .unwrap_or_else(|| format!("~/hotmint-v{}", vid));
        let remote_src = "~/hotmint";

        // Validate
        let _v = state
            .validators
            .iter()
            .find(|v| v.id == vid)
            .ok_or_else(|| eg!("validator {} not found in cluster state", vid))?;

        println!("\n--- V{} ({}) ---", vid, host.ssh);

        // 1. Git sync
        println!("  Syncing via git (branch: {})...", branch);
        git_sync(&host.ssh, repo_url, branch, &local_commit, remote_src)?;

        // 2. Config files via ssh pipe
        let local_config = base_dir.join(format!("v{}", vid)).join("config");
        if local_config.exists() {
            println!("  Syncing config...");
            ssh_exec(
                &host.ssh,
                &format!("mkdir -p {}/config", shell_escape(&remote_home)),
            )?;
            for file in [
                "config.toml",
                "genesis.json",
                "priv_validator_key.json",
                "node_key.json",
            ] {
                let local_file = local_config.join(file);
                if local_file.exists() {
                    let content = fs::read_to_string(&local_file).c(d!("read config file"))?;
                    ssh_write_file(
                        &host.ssh,
                        &format!("{}/config/{}", remote_home, file),
                        &content,
                    )?;
                }
            }
        }

        // 3. Build on remote (use set -o pipefail so build failure propagates through pipe)
        println!("  Building {} on {}...", package, host.ssh);
        let build_output = ssh_exec(
            &host.ssh,
            &format!(
                "set -o pipefail; cd {} && cargo build --release -p {} 2>&1 | tail -5",
                shell_escape(remote_src),
                shell_escape(package),
            ),
        )?;
        println!("  {}", build_output.trim());

        // 4. Stop any existing node, then start
        println!("  Starting V{}...", vid);
        let bin_path = format!("{}/target/release/{}", remote_src, package);
        let pid_file = format!("/tmp/hotmint-v{}.pid", vid);
        let esc_pid = shell_escape(&pid_file);
        let esc_bin = shell_escape(&bin_path);
        let esc_home = shell_escape(&remote_home);
        ssh_exec(
            &host.ssh,
            &format!(
                "if [ -f {esc_pid} ]; then kill $(cat {esc_pid}) 2>/dev/null; sleep 1; fi; \
                 nohup {esc_bin} --home {esc_home} > /tmp/hotmint-v{vid}.log 2>&1 & echo $! > {esc_pid}",
            ),
        )?;
        println!("  V{}: started on {}", vid, host.ssh);
    }

    println!("\nDeployment complete.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Cluster-wide operations (chaindev-style)
// ---------------------------------------------------------------------------

/// Execute a command on all remote hosts and print results.
pub fn exec_all(hosts_path: &Path, cmd: &str) -> Result<()> {
    let hosts = HostsConfig::load(hosts_path)?;
    for host in &hosts.hosts {
        println!("--- V{} ({}) ---", host.validator_id, host.ssh);
        match ssh_exec(&host.ssh, cmd) {
            Ok(output) => print!("{}", output),
            Err(e) => eprintln!("  ERROR: {e}"),
        }
    }
    Ok(())
}

/// Push a local file or directory to all remote hosts.
pub fn push_all(hosts_path: &Path, local: &Path, remote_dest: &str) -> Result<()> {
    let hosts = HostsConfig::load(hosts_path)?;
    for host in &hosts.hosts {
        println!("--- V{} ({}) ---", host.validator_id, host.ssh);
        if local.is_dir() {
            // Use tar pipe for directories
            let status = process::Command::new("sh")
                .args([
                    "-c",
                    &format!(
                        "tar czf - -C {} . | ssh -o BatchMode=yes {} 'mkdir -p {} && cd {} && tar xzf -'",
                        local.display(),
                        &host.ssh,
                        shell_escape(remote_dest),
                        shell_escape(remote_dest),
                    ),
                ])
                .status()
                .c(d!("push dir to {}", host.ssh))?;
            if !status.success() {
                eprintln!("  ERROR: push to {} failed", host.ssh);
            } else {
                println!("  OK");
            }
        } else {
            // Single file: read and pipe through ssh
            let content = fs::read_to_string(local).c(d!("read local file"))?;
            match ssh_write_file(&host.ssh, remote_dest, &content) {
                Ok(()) => println!("  OK"),
                Err(e) => eprintln!("  ERROR: {e}"),
            }
        }
    }
    Ok(())
}

/// Pull a file from all remote hosts into a local directory.
/// Each file is saved as `{local_dir}/V{id}_{filename}`.
pub fn pull_all(hosts_path: &Path, remote_src: &str, local_dir: &Path) -> Result<()> {
    let hosts = HostsConfig::load(hosts_path)?;
    fs::create_dir_all(local_dir).c(d!("create local dir"))?;

    let filename = Path::new(remote_src)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());

    for host in &hosts.hosts {
        let dest = local_dir.join(format!("V{}_{}", host.validator_id, filename));
        println!(
            "--- V{} ({}) → {} ---",
            host.validator_id,
            host.ssh,
            dest.display()
        );
        let status = process::Command::new("scp")
            .args(["-o", "BatchMode=yes"])
            .arg(format!("{}:{}", host.ssh, remote_src))
            .arg(&dest)
            .status()
            .c(d!("scp from {}", host.ssh))?;
        if !status.success() {
            eprintln!("  ERROR: pull from {} failed", host.ssh);
        } else {
            println!("  OK");
        }
    }
    Ok(())
}

/// Collect, tail, or grep logs from remote nodes.
pub fn logs(
    hosts_path: &Path,
    lines: u32,
    grep: Option<&str>,
    collect_dir: Option<&Path>,
) -> Result<()> {
    let hosts = HostsConfig::load(hosts_path)?;

    if let Some(dir) = collect_dir {
        // Download all log files
        fs::create_dir_all(dir).c(d!("create collect dir"))?;
        for host in &hosts.hosts {
            let vid = host.validator_id;
            let remote_log = format!("/tmp/hotmint-v{}.log", vid);
            let local_path = dir.join(format!("V{}.log", vid));
            println!(
                "Collecting V{} ({}) → {}",
                vid,
                host.ssh,
                local_path.display()
            );
            let status = process::Command::new("scp")
                .args(["-o", "BatchMode=yes"])
                .arg(format!("{}:{}", host.ssh, remote_log))
                .arg(&local_path)
                .status()
                .c(d!("scp log from {}", host.ssh))?;
            if !status.success() {
                eprintln!("  WARNING: failed to collect log from {}", host.ssh);
            }
        }
        println!("Logs collected to {}", dir.display());
        return Ok(());
    }

    // Tail + optional grep
    for host in &hosts.hosts {
        let vid = host.validator_id;
        let remote_log = format!("/tmp/hotmint-v{}.log", vid);
        let cmd = if let Some(pattern) = grep {
            format!(
                "tail -n {} {} | grep --color=never {}",
                lines,
                shell_escape(&remote_log),
                shell_escape(pattern),
            )
        } else {
            format!("tail -n {} {}", lines, shell_escape(&remote_log))
        };

        println!("=== V{} ({}) ===", vid, host.ssh);
        match ssh_exec(&host.ssh, &cmd) {
            Ok(output) => print!("{}", output),
            Err(e) => eprintln!("  ERROR: {e}"),
        }
    }
    Ok(())
}

/// Show status of all remote nodes (process alive, RPC height/view/epoch).
pub fn remote_status(base_dir: &Path, hosts_path: &Path) -> Result<()> {
    let state = ClusterState::load(base_dir)?;
    let hosts = HostsConfig::load(hosts_path)?;

    println!(
        "{:<6} {:<20} {:<8} {:<10} {:<8} {:<8}",
        "NODE", "HOST", "PID", "HEIGHT", "VIEW", "EPOCH"
    );
    println!("{}", "-".repeat(62));

    for host in &hosts.hosts {
        let vid = host.validator_id;
        let pid_file = format!("/tmp/hotmint-v{}.pid", vid);

        // Check if process is alive
        let pid_info = match ssh_exec(
            &host.ssh,
            &format!(
                "if [ -f {} ]; then pid=$(cat {}); if kill -0 $pid 2>/dev/null; then echo \"alive:$pid\"; else echo dead; fi; else echo none; fi",
                shell_escape(&pid_file),
                shell_escape(&pid_file)
            ),
        ) {
            Ok(s) => s.trim().to_string(),
            Err(_) => "ssh-err".into(),
        };

        let (status_str, pid_str) = if pid_info.starts_with("alive:") {
            ("UP", pid_info.strip_prefix("alive:").unwrap_or("?"))
        } else if pid_info == "dead" {
            ("DOWN", "-")
        } else if pid_info == "none" {
            ("NONE", "-")
        } else {
            ("ERR", "-")
        };

        // Try RPC status if node is up and we know its port
        let (height, view, epoch) = if status_str == "UP" {
            if let Some(v) = state.validators.iter().find(|v| v.id == vid) {
                let rpc_host = host
                    .external_ip
                    .as_deref()
                    .unwrap_or_else(|| host.ssh.split('@').next_back().unwrap_or(&host.ssh));
                match query_rpc_status(rpc_host, v.rpc_port) {
                    Ok((h, v, e)) => (h, v, e),
                    Err(_) => ("-".into(), "-".into(), "-".into()),
                }
            } else {
                ("-".into(), "-".into(), "-".into())
            }
        } else {
            ("-".into(), "-".into(), "-".into())
        };

        println!(
            "{:<6} {:<20} {:<8} {:<10} {:<8} {:<8}",
            format!("V{}", vid),
            &host.ssh,
            if status_str == "UP" {
                pid_str
            } else {
                status_str
            },
            height,
            view,
            epoch,
        );
    }
    Ok(())
}

/// Query a node's JSON-RPC status endpoint.
fn query_rpc_status(host: &str, port: u16) -> Result<(String, String, String)> {
    use std::io::{Read, Write as IoWrite};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("{}:{}", host, port);
    let mut stream =
        TcpStream::connect_timeout(&addr.parse().c(d!("parse addr"))?, Duration::from_secs(2))
            .c(d!("connect rpc"))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();

    let req = r#"{"jsonrpc":"2.0","method":"status","params":[],"id":1}"#;
    // Raw TCP JSON-RPC: send JSON directly followed by newline
    stream
        .write_all(format!("{}\n", req).as_bytes())
        .c(d!("write rpc"))?;

    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok();

    let body = &buf;

    // Simple JSON parsing — look for height/view/epoch fields
    let extract = |key: &str| -> String {
        body.find(&format!("\"{}\":", key))
            .and_then(|i| {
                let rest = &body[i + key.len() + 3..];
                // Handle both number and string values
                if let Some(stripped) = rest.strip_prefix('"') {
                    stripped.split('"').next().map(|s| s.to_string())
                } else {
                    rest.split(|c: char| !c.is_ascii_digit())
                        .next()
                        .map(|s| s.to_string())
                }
            })
            .unwrap_or_else(|| "-".into())
    };

    Ok((
        extract("last_committed_height"),
        extract("current_view"),
        extract("epoch"),
    ))
}
