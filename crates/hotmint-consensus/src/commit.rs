use ruc::*;

use crate::application::Application;
use crate::store::BlockStore;
use hotmint_types::{Block, BlockHash, DoubleCertificate, Height};
use tracing::info;

/// Execute the two-chain commit rule:
/// When we get C_v(C_v(B_k)), commit the inner QC's block and all uncommitted ancestors
pub fn try_commit(
    double_cert: &DoubleCertificate,
    store: &dyn BlockStore,
    app: &dyn Application,
    last_committed_height: &mut Height,
) -> Result<Vec<Block>> {
    let commit_hash = double_cert.inner_qc.block_hash;
    let commit_block = store
        .get_block(&commit_hash)
        .c(d!("block to commit not found"))?;

    if commit_block.height <= *last_committed_height {
        return Ok(vec![]);
    }

    // Collect all uncommitted ancestors (from highest to lowest)
    let mut to_commit = Vec::new();
    let mut current = commit_block;
    loop {
        if current.height <= *last_committed_height {
            break;
        }
        let parent_hash = current.parent_hash;
        to_commit.push(current);
        if parent_hash == BlockHash::GENESIS {
            break;
        }
        match store.get_block(&parent_hash) {
            Some(parent) => current = parent,
            None => break,
        }
    }

    // Commit from lowest height to highest
    to_commit.reverse();

    for block in &to_commit {
        info!(height = block.height.as_u64(), hash = %block.hash, "committing block");
        app.on_commit(block).c(d!("application commit failed"))?;
        *last_committed_height = block.height;
    }

    Ok(to_commit)
}
