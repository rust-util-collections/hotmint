use ruc::*;

use std::sync::Arc;

use crate::types::{RpcRequest, RpcResponse, StatusInfo, TxResult};
use hotmint_mempool::Mempool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{info, warn};

/// Shared state accessible by the RPC server
pub struct RpcState {
    pub validator_id: u64,
    pub mempool: Arc<Mempool>,
    pub status_rx: watch::Receiver<(u64, u64, u64)>, // (current_view, last_committed_height, epoch)
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
            let (view, height, epoch) = *state.status_rx.borrow();
            let info = StatusInfo {
                validator_id: state.validator_id,
                current_view: view,
                last_committed_height: height,
                epoch,
                mempool_size: state.mempool.size().await,
            };
            match serde_json::to_value(info) {
                Ok(v) => RpcResponse::ok(req.id, v),
                Err(e) => RpcResponse::err(req.id, -32603, format!("serialization error: {e}")),
            }
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
            match serde_json::to_value(TxResult { accepted }) {
                Ok(v) => RpcResponse::ok(req.id, v),
                Err(e) => RpcResponse::err(req.id, -32603, format!("serialization error: {e}")),
            }
        }
        _ => RpcResponse::err(req.id, -32601, format!("unknown method: {}", req.method)),
    }
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
