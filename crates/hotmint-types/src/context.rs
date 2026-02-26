use crate::block::Height;
use crate::epoch::EpochNumber;
use crate::validator::{ValidatorId, ValidatorSet};
use crate::view::ViewNumber;

/// Context provided to Application trait methods during block processing.
pub struct BlockContext<'a> {
    pub height: Height,
    pub view: ViewNumber,
    pub proposer: ValidatorId,
    pub epoch: EpochNumber,
    pub validator_set: &'a ValidatorSet,
}

/// Lightweight context for transaction validation (mempool admission).
/// Unlike [`BlockContext`], this does not require a specific block.
pub struct TxContext {
    pub height: Height,
    pub epoch: EpochNumber,
}
