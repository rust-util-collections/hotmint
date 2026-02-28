use serde::{Deserialize, Serialize};

use crate::peer::{PeerInfo, PeerRole};

/// PEX (Peer Exchange) request messages.
#[derive(Debug, Serialize, Deserialize)]
pub enum PexRequest {
    /// Request a list of known peers.
    GetPeers,
    /// Advertise this node's presence and role.
    Advertise {
        role: PeerRole,
        validator_id: Option<u64>,
        addresses: Vec<String>,
    },
}

/// PEX response messages.
#[derive(Debug, Serialize, Deserialize)]
pub enum PexResponse {
    /// List of known peers (up to max_peers_per_response).
    Peers(Vec<PeerInfo>),
    /// Acknowledgment of an Advertise message.
    Ack,
}

/// PEX configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PexConfig {
    pub enabled: bool,
    pub max_peers: usize,
    pub request_interval_secs: u64,
    pub max_peers_per_response: usize,
}

impl Default for PexConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_peers: 50,
            request_interval_secs: 30,
            max_peers_per_response: 32,
        }
    }
}
