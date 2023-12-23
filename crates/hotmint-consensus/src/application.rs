use ruc::*;

use hotmint_types::{Block, Height, ViewNumber};

/// ABCI-like application interface for the consensus engine.
///
/// The lifecycle for each block:
/// 1. `begin_block` — called when a new block is being proposed
/// 2. `deliver_tx` — called for each transaction in the payload
/// 3. `end_block` — called after all transactions are delivered
/// 4. `on_commit` — called when the block is finalized in the committed chain
///
/// For block validation:
/// - `validate_block` — full block validation before voting
/// - `validate_tx` — individual transaction validation for mempool
pub trait Application: Send + Sync {
    /// Create a payload for a new block proposal.
    /// Typically pulls transactions from the mempool.
    fn create_payload(&self) -> Vec<u8> {
        vec![]
    }

    /// Validate a proposed block before voting.
    fn validate_block(&self, _block: &Block) -> bool {
        true
    }

    /// Validate a single transaction for mempool admission.
    fn validate_tx(&self, _tx: &[u8]) -> bool {
        true
    }

    /// Called at the beginning of block execution.
    fn begin_block(&self, _height: Height, _view: ViewNumber) -> Result<()> {
        Ok(())
    }

    /// Called for each transaction in the block payload.
    fn deliver_tx(&self, _tx: &[u8]) -> Result<()> {
        Ok(())
    }

    /// Called after all transactions in the block are delivered.
    fn end_block(&self, _height: Height) -> Result<()> {
        Ok(())
    }

    /// Called when a block is committed to the chain.
    fn on_commit(&self, block: &Block) -> Result<()>;

    /// Query application state (returns opaque bytes).
    fn query(&self, _path: &str, _data: &[u8]) -> Result<Vec<u8>> {
        Ok(vec![])
    }
}

/// No-op application stub
pub struct NoopApplication;

impl Application for NoopApplication {
    fn on_commit(&self, _block: &Block) -> Result<()> {
        Ok(())
    }
}
