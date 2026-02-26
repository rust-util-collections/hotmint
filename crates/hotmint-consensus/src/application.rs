use ruc::*;

use hotmint_types::Block;
use hotmint_types::context::{BlockContext, TxContext};
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator_update::EndBlockResponse;

/// Application interface for the consensus engine.
///
/// The lifecycle for each committed block:
/// 1. `execute_block` — receives all decoded transactions at once; returns
///    validator updates and events
/// 2. `on_commit` — notification after the block is finalized
///
/// For block proposal:
/// - `create_payload` — build the payload bytes for a new block
///
/// For validation (before voting):
/// - `validate_block` — full block validation
/// - `validate_tx` — individual transaction validation for mempool
///
/// For evidence:
/// - `on_evidence` — called when equivocation is detected
///
/// All methods have default no-op implementations.
pub trait Application: Send + Sync {
    /// Create a payload for a new block proposal.
    /// Typically pulls transactions from the mempool.
    ///
    /// If your mempool is async, use `tokio::runtime::Handle::current().block_on(..)`
    /// to bridge into this synchronous callback.
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        vec![]
    }

    /// Validate a proposed block before voting.
    fn validate_block(&self, _block: &Block, _ctx: &BlockContext) -> bool {
        true
    }

    /// Validate a single transaction for mempool admission.
    ///
    /// An optional [`TxContext`] provides the current chain height and epoch,
    /// which can be useful for state-dependent validation (nonce checks, etc.).
    fn validate_tx(&self, _tx: &[u8], _ctx: Option<&TxContext>) -> bool {
        true
    }

    /// Execute an entire block in one call.
    ///
    /// Receives all decoded transactions from the block payload at once,
    /// allowing batch-optimised processing (bulk DB writes, parallel
    /// signature verification, etc.).
    ///
    /// Return [`EndBlockResponse`] with `validator_updates` to schedule an
    /// epoch transition, and/or `events` to emit application-defined events.
    fn execute_block(&self, _txs: &[&[u8]], _ctx: &BlockContext) -> Result<EndBlockResponse> {
        Ok(EndBlockResponse::default())
    }

    /// Called when a block is committed to the chain (notification).
    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        Ok(())
    }

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

/// No-op application stub for testing.
pub struct NoopApplication;

impl Application for NoopApplication {}
