use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use ruc::*;
use serde::{Deserialize, Serialize};

use hotmint_types::crypto::PublicKey;
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};

// ── config.toml ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Unix socket path for the ABCI application connection.
    pub proxy_app: String,
    pub rpc: RpcConfig,
    pub p2p: P2pConfig,
    pub consensus: ConsensusConfig,
    pub mempool: MempoolConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    /// JSON-RPC listen address (e.g., "127.0.0.1:26657").
    pub laddr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConfig {
    /// P2P listen address as a multiaddr (e.g., "/ip4/0.0.0.0/tcp/26656").
    pub laddr: String,
    /// Persistent peers: `"<validator_id>@<multiaddr>"`.
    pub persistent_peers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusConfig {
    pub base_timeout_ms: u64,
    pub max_timeout_ms: u64,
    pub backoff_multiplier: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MempoolConfig {
    pub max_txs: usize,
    pub max_tx_bytes: usize,
    pub max_payload_bytes: usize,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            proxy_app: "unix:///tmp/hotmint/app.sock".into(),
            rpc: RpcConfig {
                laddr: "127.0.0.1:26657".into(),
            },
            p2p: P2pConfig {
                laddr: "/ip4/0.0.0.0/tcp/26656".into(),
                persistent_peers: vec![],
            },
            consensus: ConsensusConfig {
                base_timeout_ms: 2000,
                max_timeout_ms: 30000,
                backoff_multiplier: 1.5,
            },
            mempool: MempoolConfig {
                max_txs: 10_000,
                max_tx_bytes: 1_048_576,
                max_payload_bytes: 4_194_304,
            },
        }
    }
}

impl NodeConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path).c(d!("read config.toml"))?;
        toml::from_str(&contents).c(d!("parse config.toml"))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let contents = toml::to_string_pretty(self).c(d!("serialize config.toml"))?;
        std::fs::write(path, contents).c(d!("write config.toml"))
    }
}

// ── genesis.json ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisDoc {
    pub chain_id: String,
    pub validators: Vec<GenesisValidator>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisValidator {
    pub id: u64,
    /// Hex-encoded ed25519 public key (32 bytes → 64 hex chars).
    pub public_key: String,
    pub power: u64,
}

impl GenesisDoc {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path).c(d!("read genesis.json"))?;
        serde_json::from_str(&contents).c(d!("parse genesis.json"))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let contents = serde_json::to_string_pretty(self).c(d!("serialize genesis.json"))?;
        std::fs::write(path, contents).c(d!("write genesis.json"))
    }

    /// Build a `ValidatorSet` from the genesis validators.
    pub fn to_validator_set(&self) -> Result<ValidatorSet> {
        let infos: Result<Vec<ValidatorInfo>> = self
            .validators
            .iter()
            .map(|v| {
                let pk_bytes = hex::decode(&v.public_key).c(d!("decode public key hex"))?;
                Ok(ValidatorInfo {
                    id: ValidatorId(v.id),
                    public_key: PublicKey(pk_bytes),
                    power: v.power,
                })
            })
            .collect();
        let mut vs = ValidatorSet::new(infos?);
        vs.rebuild_index();
        Ok(vs)
    }
}

// ── priv_validator_key.json ────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct PrivValidatorKey {
    pub validator_id: u64,
    /// Hex-encoded ed25519 public key.
    pub public_key: String,
    /// Hex-encoded 32-byte ed25519 seed (private key).
    pub private_key: String,
}

impl PrivValidatorKey {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let public_key = signing_key.verifying_key();
        Self {
            validator_id: 0,
            public_key: hex::encode(public_key.to_bytes()),
            private_key: hex::encode(signing_key.to_bytes()),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path).c(d!("read priv_validator_key.json"))?;
        serde_json::from_str(&contents).c(d!("parse priv_validator_key.json"))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let contents =
            serde_json::to_string_pretty(self).c(d!("serialize priv_validator_key.json"))?;
        std::fs::write(path, contents).c(d!("write priv_validator_key.json"))
    }

    pub fn to_signing_key(&self) -> Result<SigningKey> {
        let bytes = hex::decode(&self.private_key).c(d!("decode private key hex"))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| eg!("private key must be 32 bytes"))?;
        Ok(SigningKey::from_bytes(&seed))
    }

    pub fn to_litep2p_keypair(&self) -> Result<litep2p::crypto::ed25519::Keypair> {
        let bytes = hex::decode(&self.private_key).c(d!("decode private key hex"))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| eg!("private key must be 32 bytes"))?;
        let secret = litep2p::crypto::ed25519::SecretKey::try_from_bytes(seed)
            .c(d!("create litep2p secret key"))?;
        Ok(litep2p::crypto::ed25519::Keypair::from(secret))
    }
}

// ── Home directory resolution ──────────────────────────────────────

pub fn resolve_home(cli_home: Option<&str>) -> PathBuf {
    if let Some(h) = cli_home {
        return PathBuf::from(h);
    }
    if let Ok(h) = std::env::var("HOTMINT_HOME") {
        return PathBuf::from(h);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hotmint")
}

// ── Init node directory ────────────────────────────────────────────

pub fn init_node_dir(home: &Path) -> Result<()> {
    let config_dir = home.join("config");
    let data_dir = home.join("data");

    std::fs::create_dir_all(&config_dir).c(d!("create config dir"))?;
    std::fs::create_dir_all(&data_dir).c(d!("create data dir"))?;

    // Generate validator key
    let priv_key = PrivValidatorKey::generate();
    let priv_key_path = config_dir.join("priv_validator_key.json");
    priv_key.save(&priv_key_path)?;

    // Create genesis with this single validator
    let genesis = GenesisDoc {
        chain_id: "hotmint-localnet".into(),
        validators: vec![GenesisValidator {
            id: priv_key.validator_id,
            public_key: priv_key.public_key.clone(),
            power: 1,
        }],
    };
    genesis.save(&config_dir.join("genesis.json"))?;

    // Write default config
    let config = NodeConfig::default();
    config.save(&config_dir.join("config.toml"))?;

    println!("Initialized hotmint node directory: {}", home.display());
    println!("  Validator ID:  {}", priv_key.validator_id);
    println!("  Public key:    {}", priv_key.public_key);
    println!(
        "  Config:        {}",
        config_dir.join("config.toml").display()
    );
    println!(
        "  Genesis:       {}",
        config_dir.join("genesis.json").display()
    );
    println!("  Validator key: {}", priv_key_path.display());
    println!("  Data dir:      {}", data_dir.display());

    Ok(())
}

// ── Peer parsing ───────────────────────────────────────────────────

use hotmint_network::service::PeerMap;
use litep2p::PeerId;
use litep2p::types::multiaddr::Multiaddr;

/// Parsed peer network information: a PeerMap and the corresponding known addresses.
pub type PeerNetworkInfo = (PeerMap, Vec<(PeerId, Vec<Multiaddr>)>);

/// Parse persistent_peers from config into PeerMap + known_addresses.
///
/// Format: `"<validator_id>@<multiaddr>"`, e.g., `"0@/ip4/10.0.0.1/tcp/26656"`.
/// The PeerId is derived from the validator's public key in the genesis doc.
pub fn parse_persistent_peers(peers: &[String], genesis: &GenesisDoc) -> Result<PeerNetworkInfo> {
    let mut peer_map = PeerMap::new();
    let mut known_addresses = Vec::new();

    for entry in peers {
        let (id_str, addr_str) = entry.split_once('@').ok_or_else(|| {
            eg!(
                "invalid peer format, expected '<id>@<multiaddr>': {}",
                entry
            )
        })?;

        let vid: u64 = id_str.parse().c(d!("invalid validator id: {}", id_str))?;

        let addr: Multiaddr = addr_str.parse().c(d!("invalid multiaddr: {}", addr_str))?;

        // Find the validator's public key in genesis
        let gv = genesis
            .validators
            .iter()
            .find(|v| v.id == vid)
            .ok_or_else(|| eg!("validator {} not found in genesis", vid))?;

        let pk_bytes = hex::decode(&gv.public_key).c(d!("decode peer public key"))?;
        let pk = litep2p::crypto::ed25519::PublicKey::try_from_bytes(&pk_bytes)
            .c(d!("invalid ed25519 public key for validator {}", vid))?;
        let peer_id = pk.to_peer_id();

        peer_map.insert(ValidatorId(vid), peer_id);
        known_addresses.push((peer_id, vec![addr]));
    }

    Ok((peer_map, known_addresses))
}
