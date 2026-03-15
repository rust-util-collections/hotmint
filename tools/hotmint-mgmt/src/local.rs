//! Local multi-node cluster management: start, stop, status.

use std::io::{Read as _, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use ruc::*;

use crate::cluster::ClusterState;

/// Find the cluster-node binary: check --binary flag, workspace target, PATH.
fn find_binary(binary: Option<&Path>) -> Result<PathBuf> {
    if let Some(b) = binary {
        if b.exists() {
            return Ok(b.to_path_buf());
        }
        return Err(eg!("binary not found: {}", b.display()));
    }

    // Try workspace target/release
    let workspace_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/release/cluster-node");
    if workspace_bin.exists() {
        return workspace_bin.canonicalize().c(d!());
    }

    // Try PATH
    match which("cluster-node") {
        Some(p) => Ok(p),
        None => Err(eg!(
            "cluster-node binary not found. Build with: cargo build --release -p cluster-node"
        )),
    }
}

fn which(cmd: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(cmd);
            if full.is_file() { Some(full) } else { None }
        })
    })
}

/// Read PID from a pid file.
fn read_pid(base_dir: &Path, id: u64) -> Option<u32> {
    let pid_file = base_dir.join(format!("v{}.pid", id));
    std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Write PID to a pid file.
fn write_pid(base_dir: &Path, id: u64, pid: u32) {
    let pid_file = base_dir.join(format!("v{}.pid", id));
    let _ = std::fs::write(&pid_file, pid.to_string());
}

/// Check if a process is running.
fn is_running(pid: u32) -> bool {
    // Use kill(pid, 0) to check existence
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Verify a PID belongs to a cluster-node process (prevents killing unrelated processes
/// if the PID was recycled after the node exited).
fn is_cluster_node(pid: u32) -> bool {
    // On macOS/BSD/Linux, check /proc or ps for the process name
    process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .map(|out| {
            let name = String::from_utf8_lossy(&out.stdout);
            name.trim().contains("cluster-node") || name.trim().contains("hotmint")
        })
        .unwrap_or(false)
}

mod libc {
    unsafe extern "C" {
        pub fn kill(pid: i32, sig: i32) -> i32;
    }
}

pub fn start(base_dir: &Path, node: Option<u32>, binary: Option<&Path>) -> Result<()> {
    let state = ClusterState::load(base_dir)?;
    let bin = find_binary(binary)?;
    println!("Using binary: {}", bin.display());

    let nodes: Vec<&crate::cluster::ValidatorEntry> = if let Some(id) = node {
        state
            .validators
            .iter()
            .filter(|v| v.id == id as u64)
            .collect()
    } else {
        state.validators.iter().collect()
    };

    for v in &nodes {
        // Check if already running
        if let Some(pid) = read_pid(base_dir, v.id)
            && is_running(pid) {
                println!("V{}: already running (pid {})", v.id, pid);
                continue;
            }

        let log_file = base_dir.join(format!("v{}.log", v.id));
        let log = std::fs::File::create(&log_file).c(d!("create log file"))?;
        let log_err = log.try_clone().c(d!("clone log file"))?;

        let child = process::Command::new(&bin)
            .arg("--home")
            .arg(&v.home_dir)
            .stdout(log)
            .stderr(log_err)
            .spawn()
            .c(d!("spawn V{}", v.id))?;

        let pid = child.id();
        write_pid(base_dir, v.id, pid);
        println!(
            "V{}: started (pid {}, p2p={}, rpc={}, log={})",
            v.id,
            pid,
            v.p2p_port,
            v.rpc_port,
            log_file.display(),
        );
    }

    Ok(())
}

pub fn stop(base_dir: &Path, node: Option<u32>) -> Result<()> {
    let state = ClusterState::load(base_dir)?;

    let nodes: Vec<&crate::cluster::ValidatorEntry> = if let Some(id) = node {
        state
            .validators
            .iter()
            .filter(|v| v.id == id as u64)
            .collect()
    } else {
        state.validators.iter().collect()
    };

    for v in &nodes {
        if let Some(pid) = read_pid(base_dir, v.id) {
            if is_running(pid) && is_cluster_node(pid) {
                unsafe {
                    libc::kill(pid as i32, 15); // SIGTERM
                }
                println!("V{}: stopped (pid {})", v.id, pid);
            } else if is_running(pid) {
                println!("V{}: pid {} is not a cluster-node (stale pid file?)", v.id, pid);
            } else {
                println!("V{}: not running", v.id);
            }
            // Clean up pid file
            let pid_file = base_dir.join(format!("v{}.pid", v.id));
            let _ = std::fs::remove_file(&pid_file);
        } else {
            println!("V{}: not running (no pid file)", v.id);
        }
    }

    Ok(())
}

pub fn status(base_dir: &Path) -> Result<()> {
    let state = ClusterState::load(base_dir)?;

    println!(
        "Cluster: {} ({} validators)",
        state.chain_id, state.validator_count
    );
    println!();

    for v in &state.validators {
        let running = read_pid(base_dir, v.id)
            .map(is_running)
            .unwrap_or(false);

        let status_str = if running { "RUNNING" } else { "STOPPED" };

        // Try to get RPC status if running
        let rpc_info = if running {
            match query_rpc_status(&state.bind_ip, v.rpc_port) {
                Ok(info) => format!(" | {}", info),
                Err(_) => " | RPC unreachable".to_string(),
            }
        } else {
            String::new()
        };

        let pid_str = read_pid(base_dir, v.id)
            .map(|p| format!(" pid={}", p))
            .unwrap_or_default();

        println!(
            "  V{}: {} {} p2p={} rpc={}{}",
            v.id, status_str, pid_str, v.p2p_port, v.rpc_port, rpc_info,
        );
    }

    Ok(())
}

fn query_rpc_status(host: &str, port: u16) -> Result<String> {
    let addr = format!("{}:{}", host, port);
    let mut stream =
        TcpStream::connect_timeout(&addr.parse().c(d!())?, Duration::from_secs(2)).c(d!())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .c(d!())?;

    // Hotmint RPC uses raw JSON-RPC over TCP (not HTTP).
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"status","params":[]}"#;
    stream.write_all(req.as_bytes()).c(d!())?;
    stream.write_all(b"\n").c(d!())?;

    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(response.trim())
        && let Some(result) = val.get("result") {
            let height = result
                .get("last_committed_height")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let view = result
                .get("current_view")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let epoch = result.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0);
            return Ok(format!("height={} view={} epoch={}", height, view, epoch));
        }
    Err(eg!("could not parse RPC response"))
}
