use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxContext {
    pub height: Height,
    pub epoch: EpochNumber,
}

/// Owned version of [`BlockContext`] for cross-process IPC.
///
/// `BlockContext<'a>` borrows the `ValidatorSet`, which cannot be sent across
/// process boundaries. This type owns all its data and is serializable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedBlockContext {
    pub height: Height,
    pub view: ViewNumber,
    pub proposer: ValidatorId,
    pub epoch: EpochNumber,
    pub validator_set: ValidatorSet,
}

impl From<&BlockContext<'_>> for OwnedBlockContext {
    fn from(ctx: &BlockContext<'_>) -> Self {
        Self {
            height: ctx.height,
            view: ctx.view,
            proposer: ctx.proposer,
            epoch: ctx.epoch,
            validator_set: ctx.validator_set.clone(),
        }
    }
}
