use serde::{Deserialize, Serialize};

use crate::block::{Block, Height};
use crate::epoch::EpochNumber;
use crate::view::ViewNumber;

/// Maximum number of blocks in a single sync response
pub const MAX_SYNC_BATCH: u64 = 100;

/// Sync request sent by a node that needs to catch up
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncRequest {
    /// Request blocks in [from_height, to_height] inclusive
    GetBlocks {
        from_height: Height,
        to_height: Height,
    },
    /// Request the peer's current tip status
    GetStatus,
}

/// Sync response from a node serving blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncResponse {
    /// Requested blocks (may be fewer than requested if range exceeds tip)
    Blocks(Vec<Block>),
    /// Current status of the responding node
    Status {
        last_committed_height: Height,
        current_view: ViewNumber,
        epoch: EpochNumber,
    },
    /// Error (e.g., invalid range)
    Error(String),
}
