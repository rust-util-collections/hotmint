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

/// Consensus status info
#[derive(Debug, Clone, Serialize)]
pub struct StatusInfo {
    pub validator_id: u64,
    pub current_view: u64,
    pub last_committed_height: u64,
    pub epoch: u64,
    pub validator_count: usize,
    pub mempool_size: usize,
}

/// Block info returned by get_block / get_block_by_hash
#[derive(Debug, Serialize)]
pub struct BlockInfo {
    pub height: u64,
    pub hash: String,
    pub parent_hash: String,
    pub view: u64,
    pub proposer: u64,
    pub payload_size: usize,
}

/// Validator info returned by get_validators
#[derive(Debug, Serialize)]
pub struct ValidatorInfoResponse {
    pub id: u64,
    pub power: u64,
    pub public_key: String,
}

/// Epoch info returned by get_epoch
#[derive(Debug, Serialize)]
pub struct EpochInfo {
    pub number: u64,
    pub start_view: u64,
    pub validator_count: usize,
}

/// Transaction submission result
#[derive(Debug, Serialize)]
pub struct TxResult {
    pub accepted: bool,
}
