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

/// Response from `Application::end_block()`.
/// If `validator_updates` is non-empty, an epoch transition is scheduled.
#[derive(Debug, Clone, Default)]
pub struct EndBlockResponse {
    pub validator_updates: Vec<ValidatorUpdate>,
}
