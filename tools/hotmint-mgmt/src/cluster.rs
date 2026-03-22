//! Cluster initialization, configuration, and lifecycle management.

use ruc::*;
use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

// ── Cluster state ───────────────────────────────────────────────

/// Persisted cluster state (saved in base_dir/cluster.json).
#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterState {
    pub chain_id: String,
    pub validator_count: u32,
    pub bind_ip: String,
    pub p2p_base_port: u16,
    pub rpc_base_port: u16,
    pub validators: Vec<ValidatorEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ValidatorEntry {
    pub id: u64,
    pub public_key: String,
    pub peer_id: String,
    pub home_dir: String,
    pub p2p_port: u16,
    pub rpc_port: u16,
}

impl ClusterState {
    pub fn load(base_dir: &Path) -> Result<Self> {
        let path = base_dir.join("cluster.json");
        let contents = fs::read_to_string(&path).c(d!("read cluster.json"))?;
        serde_json::from_str(&contents).c(d!("parse cluster.json"))
    }

    pub fn save(&self, base_dir: &Path) -> Result<()> {
        let path = base_dir.join("cluster.json");
        let contents = serde_json::to_string_pretty(self).c(d!("serialize cluster.json"))?;
        fs::write(&path, contents).c(d!("write cluster.json"))
    }
}

// ── Genesis and config types (standalone, no hotmint dependency) ──

#[derive(Debug, Serialize, Deserialize)]
struct GenesisDoc {
    chain_id: String,
    validators: Vec<GenesisValidator>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GenesisValidator {
    id: u64,
    public_key: String,
    power: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct PrivValidatorKey {
    validator_id: u64,
    public_key: String,
    private_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct NodeKey {
    public_key: String,
    private_key: String,
}

// ── Config template ─────────────────────────────────────────────

fn generate_config(
    p2p_port: u16,
    rpc_port: u16,
    persistent_peers: &[String],
    bind_ip: &str,
) -> String {
    let peers_toml: Vec<String> = persistent_peers
        .iter()
        .map(|p| format!("\"{}\"", p))
        .collect();
    let peers_str = peers_toml.join(", ");

    format!(
        r#"proxy_app = ""

[node]
mode = "validator"
relay_consensus = true
relay_transactions = true
serve_rpc = true
serve_sync = true

[rpc]
laddr = "{bind_ip}:{rpc_port}"

[p2p]
laddr = "/ip4/0.0.0.0/tcp/{p2p_port}"
persistent_peers = [{peers_str}]
private_peer_ids = []

[pex]
enabled = true
request_interval_secs = 30
max_peers = 50
max_peers_per_response = 10
private_peer_ids = []

[consensus]
base_timeout_ms = 2000
max_timeout_ms = 30000
backoff_multiplier = 1.5

[mempool]
max_txs = 10000
max_tx_bytes = 1048576
"#
    )
}

// ── Cluster init ────────────────────────────────────────────────

pub fn init_cluster(
    base_dir: &Path,
    validator_count: u32,
    chain_id: &str,
    p2p_base_port: u16,
    rpc_base_port: u16,
    bind_ip: &str,
) -> Result<()> {
    if base_dir.join("cluster.json").exists() {
        return Err(eg!(
            "cluster already initialized at {}. Use 'destroy' first.",
            base_dir.display()
        ));
    }
    fs::create_dir_all(base_dir).c(d!("create base dir"))?;

    println!(
        "Initializing cluster: {} validators, chain_id={}, base_dir={}",
        validator_count,
        chain_id,
        base_dir.display()
    );

    // Generate keys for all validators
    let mut keys: Vec<(u64, SigningKey, String, String)> = Vec::new();
    for i in 0..validator_count {
        let mut rng = rand::thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let public_key = signing_key.verifying_key();
        let pk_hex = hex::encode(public_key.to_bytes());
        let sk_hex = hex::encode(signing_key.to_bytes());
        keys.push((i as u64, signing_key, pk_hex, sk_hex));
    }

    // Build genesis
    let genesis = GenesisDoc {
        chain_id: chain_id.to_string(),
        validators: keys
            .iter()
            .map(|(id, _, pk_hex, _)| GenesisValidator {
                id: *id,
                public_key: pk_hex.clone(),
                power: 1,
            })
            .collect(),
    };
    let genesis_json = serde_json::to_string_pretty(&genesis).c(d!("serialize genesis"))?;

    // Build persistent_peers list: "id@/ip4/<ip>/tcp/<port>"
    let mut persistent_peers: Vec<String> = Vec::with_capacity(keys.len());
    for (id, _, _, _) in &keys {
        let port = p2p_base_port
            .checked_add(*id as u16)
            .ok_or_else(|| eg!("port calculation overflow"))?;
        persistent_peers.push(format!("{}@/ip4/{}/tcp/{}", id, bind_ip, port));
    }

    // Create per-validator home directories
    let mut entries = Vec::new();
    for (id, _sk, pk_hex, sk_hex) in &keys {
        let home = base_dir.join(format!("v{}", id));
        let config_dir = home.join("config");
        fs::create_dir_all(&config_dir).c(d!("create v{} config dir", id))?;

        // Write validator key
        let priv_key = PrivValidatorKey {
            validator_id: *id,
            public_key: pk_hex.clone(),
            private_key: sk_hex.clone(),
        };
        let key_json = serde_json::to_string_pretty(&priv_key).c(d!("serialize key"))?;
        fs::write(config_dir.join("priv_validator_key.json"), &key_json).c(d!("write key"))?;

        // Node key is the same as validator key for hotmint (PeerId = validator pubkey)
        let node_key = NodeKey {
            public_key: pk_hex.clone(),
            private_key: sk_hex.clone(),
        };
        let node_key_json = serde_json::to_string_pretty(&node_key).c(d!("serialize node key"))?;
        fs::write(config_dir.join("node_key.json"), &node_key_json).c(d!("write node key"))?;

        // Write genesis
        fs::write(config_dir.join("genesis.json"), &genesis_json).c(d!("write genesis"))?;

        // Build config with peers (excluding self)
        let peers_without_self: Vec<String> = persistent_peers
            .iter()
            .filter(|p| !p.starts_with(&format!("{}@", id)))
            .cloned()
            .collect();
        let p2p_port = p2p_base_port
            .checked_add(*id as u16)
            .ok_or_else(|| eg!("p2p port calculation overflow"))?;
        let rpc_port = rpc_base_port
            .checked_add(*id as u16)
            .ok_or_else(|| eg!("rpc port calculation overflow"))?;
        let config = generate_config(p2p_port, rpc_port, &peers_without_self, bind_ip);
        fs::write(config_dir.join("config.toml"), &config).c(d!("write config"))?;

        // Compute PeerId for display
        let peer_id = format!("(V{}: {}...)", id, &pk_hex[..16]);

        entries.push(ValidatorEntry {
            id: *id,
            public_key: pk_hex.clone(),
            peer_id,
            home_dir: home.display().to_string(),
            p2p_port,
            rpc_port,
        });

        println!(
            "  V{}: home={}, p2p={}, rpc={}, pubkey={}...",
            id,
            home.display(),
            p2p_port,
            rpc_port,
            &pk_hex[..16],
        );
    }

    // Save cluster state
    let state = ClusterState {
        chain_id: chain_id.to_string(),
        validator_count,
        bind_ip: bind_ip.to_string(),
        p2p_base_port,
        rpc_base_port,
        validators: entries,
    };
    state.save(base_dir)?;

    println!("\nCluster initialized successfully.");
    println!(
        "  Start:  hotmint-mgmt --base-dir {} start",
        base_dir.display()
    );
    println!(
        "  Status: hotmint-mgmt --base-dir {} status",
        base_dir.display()
    );

    Ok(())
}

// ── Clean (data only) ───────────────────────────────────────────

pub fn clean(base_dir: &Path) -> Result<()> {
    let state = ClusterState::load(base_dir)?;
    for v in &state.validators {
        let data_dir = PathBuf::from(&v.home_dir).join("data");
        if data_dir.exists() {
            fs::remove_dir_all(&data_dir).c(d!("remove data dir for V{}", v.id))?;
            println!("V{}: cleaned {}", v.id, data_dir.display());
        }
    }
    println!("Data directories cleaned. Configs preserved.");
    Ok(())
}

// ── Destroy (everything) ────────────────────────────────────────

pub fn destroy(base_dir: &Path) -> Result<()> {
    if !base_dir.exists() {
        println!("Nothing to destroy at {}", base_dir.display());
        return Ok(());
    }
    eprint!("Destroy cluster at {}? [y/N] ", base_dir.display());
    let _ = std::io::Write::flush(&mut std::io::stderr());
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .c(d!("read confirmation"))?;
    if !input.trim().eq_ignore_ascii_case("y") {
        println!("Aborted.");
        return Ok(());
    }
    fs::remove_dir_all(base_dir).c(d!("remove base dir"))?;
    println!("Cluster destroyed: {}", base_dir.display());
    Ok(())
}

// ── Info ────────────────────────────────────────────────────────

pub fn info(base_dir: &Path) -> Result<()> {
    let state = ClusterState::load(base_dir)?;
    println!(
        "Cluster: {} ({} validators)",
        state.chain_id, state.validator_count
    );
    println!("Bind IP: {}", state.bind_ip);
    println!();
    for v in &state.validators {
        println!(
            "  V{}: p2p={}, rpc={}, pubkey={}..., home={}",
            v.id,
            v.p2p_port,
            v.rpc_port,
            &v.public_key[..16],
            v.home_dir,
        );
    }
    Ok(())
}
