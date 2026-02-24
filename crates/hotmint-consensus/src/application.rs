use ruc::*;

use hotmint_types::context::BlockContext;
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator_update::EndBlockResponse;
use hotmint_types::Block;

/// ABCI-like application interface for the consensus engine.
///
/// The lifecycle for each committed block:
/// 1. `begin_block` — called at the start of block execution
/// 2. `deliver_tx` — called for each transaction in the payload
/// 3. `end_block` — called after all transactions; may return validator updates
/// 4. `on_commit` — called when the block is finalized
///
/// For block validation (before voting, not yet committed):
/// - `validate_block` — full block validation
/// - `validate_tx` — individual transaction validation for mempool
///
/// For evidence:
/// - `on_evidence` — called when equivocation (double-voting) is detected
pub trait Application: Send + Sync {
    /// Create a payload for a new block proposal.
    /// Typically pulls transactions from the mempool.
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        vec![]
    }

    /// Validate a proposed block before voting.
    fn validate_block(&self, _block: &Block, _ctx: &BlockContext) -> bool {
        true
    }

    /// Validate a single transaction for mempool admission.
    fn validate_tx(&self, _tx: &[u8]) -> bool {
        true
    }

    /// Called at the beginning of block execution (during commit).
    fn begin_block(&self, _ctx: &BlockContext) -> Result<()> {
        Ok(())
    }

    /// Called for each transaction in the block payload (during commit).
    fn deliver_tx(&self, _tx: &[u8]) -> Result<()> {
        Ok(())
    }

    /// Called after all transactions in the block are delivered (during commit).
    /// Return `EndBlockResponse` with `validator_updates` to trigger an epoch transition.
    fn end_block(&self, _ctx: &BlockContext) -> Result<EndBlockResponse> {
        Ok(EndBlockResponse::default())
    }

    /// Called when a block is committed to the chain.
    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()>;

    /// Called when equivocation (double-voting) is detected.
    /// The application can use this to implement slashing.
    fn on_evidence(&self, _proof: &EquivocationProof) -> Result<()> {
        Ok(())
    }

    /// Query application state (returns opaque bytes).
    fn query(&self, _path: &str, _data: &[u8]) -> Result<Vec<u8>> {
        Ok(vec![])
    }
}

/// No-op application stub for testing
pub struct NoopApplication;

impl Application for NoopApplication {
    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        Ok(())
    }
}
