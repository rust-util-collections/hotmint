use serde::{Deserialize, Serialize};

use crate::crypto::PublicKey;
use crate::validator::ValidatorId;

/// A validator update returned by the application layer.
/// Power of 0 means remove the validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorUpdate {
    pub id: ValidatorId,
    pub public_key: PublicKey,
    pub power: u64,
}

/// An application-defined event emitted during block execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Event {
    pub r#type: String,
    pub attributes: Vec<EventAttribute>,
}

/// A key-value pair within an [`Event`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventAttribute {
    pub key: String,
    pub value: String,
}

/// Response from `Application::end_block()`.
/// If `validator_updates` is non-empty, an epoch transition is scheduled.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EndBlockResponse {
    pub validator_updates: Vec<ValidatorUpdate>,
    /// Application-defined events emitted during this block.
    pub events: Vec<Event>,
    /// Application state root after executing this block.
    ///
    /// This hash is included in the **next** block's header, forming a
    /// chain of state commitments that enables cross-node state divergence
    /// detection. Applications that do not track state roots can leave this
    /// as the default (all zeros).
    #[serde(default)]
    pub app_hash: crate::block::BlockHash,
}
