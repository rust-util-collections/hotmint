use ruc::*;

use std::sync::{Arc, RwLock};

use crate::types::{
    BlockInfo, EpochInfo, RpcRequest, RpcResponse, StatusInfo, TxResult, ValidatorInfoResponse,
};
use hotmint_consensus::store::BlockStore;
use hotmint_mempool::Mempool;
use hotmint_network::service::PeerStatus;
use hotmint_types::{BlockHash, Height};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{info, warn};

/// Shared state accessible by the RPC server
pub struct RpcState {
    pub validator_id: u64,
    pub mempool: Arc<Mempool>,
    /// (current_view, last_committed_height, epoch, validator_count)
    pub status_rx: watch::Receiver<(u64, u64, u64, usize)>,
    /// Shared block store for block queries
    pub store: Arc<RwLock<Box<dyn BlockStore>>>,
    /// Peer info channel
    pub peer_info_rx: watch::Receiver<Vec<PeerStatus>>,
}

/// Simple JSON-RPC server over TCP (one JSON object per line)
pub struct RpcServer {
    state: Arc<RpcState>,
    listener: TcpListener,
}

impl RpcServer {
    pub async fn bind(addr: &str, state: RpcState) -> Result<Self> {
        let listener = TcpListener::bind(addr)
            .await
            .c(d!("failed to bind RPC server"))?;
        info!(addr = addr, "RPC server listening");
        Ok(Self {
            state: Arc::new(state),
            listener,
        })
    }

    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.listener.local_addr().expect("listener has local addr")
    }

    pub async fn run(self) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _addr)) => {
                    let state = self.state.clone();
                    tokio::spawn(async move {
                        let (reader, mut writer) = stream.into_split();
                        let mut lines = BufReader::new(reader).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let response = handle_request(&state, &line).await;
                            let mut json = serde_json::to_string(&response).unwrap_or_default();
                            json.push('\n');
                            if writer.write_all(json.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                Err(e) => {
                    warn!(error = %e, "failed to accept connection");
                }
            }
        }
    }
}

async fn handle_request(state: &RpcState, line: &str) -> RpcResponse {
    let req: RpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return RpcResponse::err(0, -32700, format!("parse error: {e}"));
        }
    };

    match req.method.as_str() {
        "status" => {
            let (view, height, epoch, validator_count) = *state.status_rx.borrow();
            let info = StatusInfo {
                validator_id: state.validator_id,
                current_view: view,
                last_committed_height: height,
                epoch,
                validator_count,
                mempool_size: state.mempool.size().await,
            };
            json_ok(req.id, &info)
        }

        "submit_tx" => {
            let tx_hex = req.params.as_str().unwrap_or_default();
            let tx_bytes = match hex_decode(tx_hex) {
                Some(b) => b,
                None => {
                    return RpcResponse::err(req.id, -32602, "invalid hex".to_string());
                }
            };
            let accepted = state.mempool.add_tx(tx_bytes).await;
            json_ok(req.id, &TxResult { accepted })
        }

        "get_block" => {
            let height = match req.params.get("height").and_then(|v| v.as_u64()) {
                Some(h) => h,
                None => {
                    return RpcResponse::err(
                        req.id,
                        -32602,
                        "missing or invalid 'height' parameter".to_string(),
                    );
                }
            };
            let store = state.store.read().unwrap();
            match store.get_block_by_height(Height(height)) {
                Some(block) => json_ok(req.id, &block_to_info(&block)),
                None => RpcResponse::err(
                    req.id,
                    -32602,
                    format!("block at height {height} not found"),
                ),
            }
        }

        "get_block_by_hash" => {
            let hash_hex = req.params.as_str().unwrap_or_default();
            match hex_to_block_hash(hash_hex) {
                Some(hash) => {
                    let store = state.store.read().unwrap();
                    match store.get_block(&hash) {
                        Some(block) => json_ok(req.id, &block_to_info(&block)),
                        None => {
                            RpcResponse::err(req.id, -32602, "block not found".to_string())
                        }
                    }
                }
                None => RpcResponse::err(req.id, -32602, "invalid hash hex".to_string()),
            }
        }

        "get_validators" => {
            let (_, _, _, _) = *state.status_rx.borrow();
            // Read validator set from the store's genesis or from status
            // For now, return from the peer info (validators are the ones we know about)
            let store = state.store.read().unwrap();
            // Get the latest committed block to find proposer info
            let tip = store.tip_height();
            drop(store);

            // We don't have direct access to ValidatorSet from RPC.
            // Return what we know: the peer list as validator info.
            // A more complete implementation would pass ValidatorSet via watch channel.
            let peers = state.peer_info_rx.borrow().clone();
            let validators: Vec<ValidatorInfoResponse> = peers
                .iter()
                .map(|p| ValidatorInfoResponse {
                    id: p.validator_id.0,
                    power: 0, // not available from peer info
                    public_key: String::new(),
                })
                .collect();

            // If no peers, at minimum return our own validator
            let result = if validators.is_empty() {
                vec![ValidatorInfoResponse {
                    id: state.validator_id,
                    power: 0,
                    public_key: String::new(),
                }]
            } else {
                validators
            };
            let _ = tip; // silence unused warning
            json_ok(req.id, &result)
        }

        "get_epoch" => {
            let (_, _, epoch, validator_count) = *state.status_rx.borrow();
            let info = EpochInfo {
                number: epoch,
                start_view: 0, // not available from status channel
                validator_count,
            };
            json_ok(req.id, &info)
        }

        "get_peers" => {
            let peers = state.peer_info_rx.borrow().clone();
            json_ok(req.id, &peers)
        }

        _ => RpcResponse::err(req.id, -32601, format!("unknown method: {}", req.method)),
    }
}

fn json_ok<T: serde::Serialize>(id: u64, val: &T) -> RpcResponse {
    match serde_json::to_value(val) {
        Ok(v) => RpcResponse::ok(id, v),
        Err(e) => RpcResponse::err(id, -32603, format!("serialization error: {e}")),
    }
}

fn block_to_info(block: &hotmint_types::Block) -> BlockInfo {
    BlockInfo {
        height: block.height.as_u64(),
        hash: hex_encode(&block.hash.0),
        parent_hash: hex_encode(&block.parent_hash.0),
        view: block.view.as_u64(),
        proposer: block.proposer.0,
        payload_size: block.payload.len(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn hex_to_block_hash(s: &str) -> Option<BlockHash> {
    let bytes = hex_decode(s)?;
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(BlockHash(arr))
}
