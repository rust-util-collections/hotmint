use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::seq::SliceRandom;
use ruc::*;
use serde::{Deserialize, Serialize};

use hotmint_types::validator::ValidatorId;
use litep2p::PeerId;
use litep2p::types::multiaddr::Multiaddr;

/// Role of a peer in the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerRole {
    /// Consensus validator (participates in voting and block production).
    Validator,
    /// Full node (syncs blocks, optionally relays messages, serves RPC).
    Fullnode,
}

/// Information about a known peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub role: PeerRole,
    pub validator_id: Option<u64>,
    pub addresses: Vec<String>,
    pub last_seen: u64,
    pub score: i32,
}

const BAN_THRESHOLD: i32 = -100;

impl PeerInfo {
    pub fn new(peer_id: PeerId, role: PeerRole, addresses: Vec<Multiaddr>) -> Self {
        Self {
            peer_id: peer_id.to_string(),
            role,
            validator_id: None,
            addresses: addresses.iter().map(|a| a.to_string()).collect(),
            last_seen: now_secs(),
            score: 0,
        }
    }

    pub fn with_validator(mut self, vid: ValidatorId) -> Self {
        self.validator_id = Some(vid.0);
        self
    }

    pub fn is_banned(&self) -> bool {
        self.score <= BAN_THRESHOLD
    }

    pub fn touch(&mut self) {
        self.last_seen = now_secs();
    }
}

/// Persistent address book of known peers.
pub struct PeerBook {
    peers: HashMap<String, PeerInfo>,
    path: PathBuf,
}

impl PeerBook {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            peers: HashMap::new(),
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref();
        if !p.exists() {
            return Ok(Self::new(p));
        }
        let contents = std::fs::read_to_string(p).c(d!("read peer book"))?;
        let peers: HashMap<String, PeerInfo> =
            serde_json::from_str(&contents).c(d!("parse peer book"))?;
        Ok(Self {
            peers,
            path: p.to_path_buf(),
        })
    }

    pub fn save(&self) -> Result<()> {
        let contents = serde_json::to_string_pretty(&self.peers).c(d!("serialize peer book"))?;
        std::fs::write(&self.path, contents).c(d!("write peer book"))
    }

    pub fn add_peer(&mut self, info: PeerInfo) {
        self.peers.insert(info.peer_id.clone(), info);
    }

    pub fn remove_peer(&mut self, peer_id: &str) {
        self.peers.remove(peer_id);
    }

    pub fn get(&self, peer_id: &str) -> Option<&PeerInfo> {
        self.peers.get(peer_id)
    }

    pub fn get_mut(&mut self, peer_id: &str) -> Option<&mut PeerInfo> {
        self.peers.get_mut(peer_id)
    }

    pub fn get_peers_by_role(&self, role: PeerRole) -> Vec<&PeerInfo> {
        self.peers
            .values()
            .filter(|p| p.role == role && !p.is_banned())
            .collect()
    }

    pub fn get_random_peers(&self, n: usize) -> Vec<&PeerInfo> {
        let mut candidates: Vec<&PeerInfo> =
            self.peers.values().filter(|p| !p.is_banned()).collect();
        let mut rng = rand::thread_rng();
        candidates.shuffle(&mut rng);
        candidates.truncate(n);
        candidates
    }

    pub fn adjust_score(&mut self, peer_id: &str, delta: i32) {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.score = peer.score.saturating_add(delta);
        }
    }

    pub fn prune_stale(&mut self, max_age_secs: u64) {
        let cutoff = now_secs().saturating_sub(max_age_secs);
        self.peers
            .retain(|_, p| p.last_seen >= cutoff || p.role == PeerRole::Validator);
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    pub fn all_peers(&self) -> impl Iterator<Item = &PeerInfo> {
        self.peers.values()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
