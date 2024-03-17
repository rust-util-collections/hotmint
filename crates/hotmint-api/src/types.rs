use serde::{Deserialize, Serialize};

/// JSON-RPC request
#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    pub method: String,
    pub params: serde_json::Value,
    pub id: u64,
}

/// JSON-RPC response
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcResponse {
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcError>,
    pub id: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcResponse {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self {
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn err(id: u64, code: i32, message: String) -> Self {
        Self {
            result: None,
            error: Some(RpcError { code, message }),
            id,
        }
    }
}

/// Status info returned by the status endpoint
#[derive(Debug, Serialize)]
pub struct StatusInfo {
    pub validator_id: u64,
    pub current_view: u64,
    pub last_committed_height: u64,
    pub mempool_size: usize,
}

/// Transaction submission result
#[derive(Debug, Serialize)]
pub struct TxResult {
    pub accepted: bool,
}
