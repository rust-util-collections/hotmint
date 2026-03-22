use ruc::*;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::types::{
    BlockInfo, EpochInfo, RpcRequest, RpcResponse, StatusInfo, TxResult, ValidatorInfoResponse,
};
use hotmint_consensus::application::Application;
use hotmint_consensus::store::BlockStore;
use hotmint_mempool::Mempool;
use hotmint_network::service::PeerStatus;
use hotmint_types::{BlockHash, Height};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::time::{Duration, timeout};
use tracing::{info, warn};

const MAX_RPC_CONNECTIONS: usize = 256;
const RPC_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum bytes per RPC line. Prevents OOM from clients sending huge data without newlines.
const MAX_LINE_BYTES: usize = 1_048_576;

/// Named consensus status shared via watch channel.
#[derive(Debug, Clone, Copy)]
pub struct ConsensusStatus {
    pub current_view: u64,
    pub last_committed_height: u64,
    pub epoch_number: u64,
    pub validator_count: usize,
    pub epoch_start_view: u64,
}

impl ConsensusStatus {
    pub fn new(
        current_view: u64,
        last_committed_height: u64,
        epoch_number: u64,
        validator_count: usize,
        epoch_start_view: u64,
    ) -> Self {
        Self {
            current_view,
            last_committed_height,
            epoch_number,
            validator_count,
            epoch_start_view,
        }
    }
}

/// Shared state accessible by the RPC server
pub struct RpcState {
    pub validator_id: u64,
    pub mempool: Arc<Mempool>,
    pub status_rx: watch::Receiver<ConsensusStatus>,
    /// Shared block store for block queries
    pub store: Arc<RwLock<Box<dyn BlockStore>>>,
    /// Peer info channel
    pub peer_info_rx: watch::Receiver<Vec<PeerStatus>>,
    /// Live validator set for get_validators
    pub validator_set_rx: watch::Receiver<Vec<ValidatorInfoResponse>>,
    /// Application reference for tx validation (optional for backward compatibility).
    pub app: Option<Arc<dyn Application>>,
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

    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("listener has local addr")
    }

    pub async fn run(self) {
        let semaphore = Arc::new(Semaphore::new(MAX_RPC_CONNECTIONS));
        loop {
            match self.listener.accept().await {
                Ok((stream, _addr)) => {
                    let permit = match semaphore.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            warn!("RPC connection limit reached, rejecting");
                            drop(stream);
                            continue;
                        }
                    };
                    let state = self.state.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let (reader, mut writer) = stream.into_split();
                        let mut reader = BufReader::with_capacity(65_536, reader);
                        loop {
                            let line = match timeout(
                                RPC_READ_TIMEOUT,
                                read_line_limited(&mut reader, MAX_LINE_BYTES),
                            )
                            .await
                            {
                                Ok(Ok(Some(line))) => line,
                                Ok(Err(e)) => {
                                    warn!(error = %e, "RPC read error (line too long?)");
                                    break;
                                }
                                _ => break, // EOF or timeout
                            };
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
            let s = *state.status_rx.borrow();
            let info = StatusInfo {
                validator_id: state.validator_id,
                current_view: s.current_view,
                last_committed_height: s.last_committed_height,
                epoch: s.epoch_number,
                validator_count: s.validator_count,
                mempool_size: state.mempool.size().await,
            };
            json_ok(req.id, &info)
        }

        "submit_tx" => {
            let Some(tx_hex) = req.params.as_str() else {
                return RpcResponse::err(req.id, -32602, "params must be a hex string".to_string());
            };
            if tx_hex.is_empty() {
                return RpcResponse::err(req.id, -32602, "empty transaction".to_string());
            }
            let tx_bytes = match hex_decode(tx_hex) {
                Some(b) if !b.is_empty() => b,
                _ => {
                    return RpcResponse::err(req.id, -32602, "invalid hex".to_string());
                }
            };
            // Validate via Application if available
            if let Some(ref app) = state.app
                && !app.validate_tx(&tx_bytes, None)
            {
                return RpcResponse::err(
                    req.id,
                    -32602,
                    "transaction validation failed".to_string(),
                );
            }
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
            let store = state.store.read().await;
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
                    let store = state.store.read().await;
                    match store.get_block(&hash) {
                        Some(block) => json_ok(req.id, &block_to_info(&block)),
                        None => RpcResponse::err(req.id, -32602, "block not found".to_string()),
                    }
                }
                None => RpcResponse::err(req.id, -32602, "invalid hash hex".to_string()),
            }
        }

        "get_validators" => {
            let validators = state.validator_set_rx.borrow().clone();
            json_ok(req.id, &validators)
        }

        "get_epoch" => {
            let s = *state.status_rx.borrow();
            let info = EpochInfo {
                number: s.epoch_number,
                start_view: s.epoch_start_view,
                validator_count: s.validator_count,
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

/// Read a line from `reader`, failing fast if it exceeds `max_bytes`.
///
/// Uses `fill_buf` + incremental scanning so memory allocation is bounded.
/// Returns `Ok(None)` on EOF, `Ok(Some(line))` on success, or an error
/// if the line exceeds the limit.
async fn read_line_limited<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
            };
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        let to_consume = available.len();
        buf.extend_from_slice(available);
        reader.consume(to_consume);
        if buf.len() > max_bytes {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
    }
}
