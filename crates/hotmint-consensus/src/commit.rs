use ruc::*;

use crate::application::Application;
use crate::store::BlockStore;
use hotmint_types::context::BlockContext;
use hotmint_types::epoch::Epoch;
use hotmint_types::{Block, BlockHash, DoubleCertificate, Height, ViewNumber};
use tracing::info;

/// Result of a commit operation
pub struct CommitResult {
    pub committed_blocks: Vec<Block>,
    /// The QC that certified the committed block (for sync protocol).
    pub commit_qc: hotmint_types::QuorumCertificate,
    /// If an epoch transition was triggered by end_block, the new epoch (start_view is placeholder)
    pub pending_epoch: Option<Epoch>,
    /// Application state root after executing the last committed block.
    pub last_app_hash: BlockHash,
}

/// Decode length-prefixed transactions from a block payload.
pub fn decode_payload(payload: &[u8]) -> Vec<&[u8]> {
    let mut txs = Vec::new();
    let mut offset = 0;
    while offset + 4 <= payload.len() {
        let len = u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if offset + len > payload.len() {
            break;
        }
        txs.push(&payload[offset..offset + len]);
        offset += len;
    }
    txs
}

/// Execute the two-chain commit rule:
/// When we get C_v(C_v(B_k)), commit the inner QC's block and all uncommitted ancestors.
///
/// For each committed block, runs the full application lifecycle:
/// begin_block → deliver_tx (×N) → end_block → on_commit
///
/// # Safety
/// Caller MUST verify both inner_qc and outer_qc aggregate signatures
/// and quorum counts before calling this function. This function trusts
/// the DoubleCertificate completely and performs no cryptographic checks.
pub fn try_commit(
    double_cert: &DoubleCertificate,
    store: &dyn BlockStore,
    app: &dyn Application,
    last_committed_height: &mut Height,
    current_epoch: &Epoch,
) -> Result<CommitResult> {
    let commit_hash = double_cert.inner_qc.block_hash;
    let commit_block = store
        .get_block(&commit_hash)
        .c(d!("block to commit not found"))?;

    if commit_block.height <= *last_committed_height {
        return Ok(CommitResult {
            committed_blocks: vec![],
            commit_qc: double_cert.inner_qc.clone(),
            pending_epoch: None,
            last_app_hash: BlockHash::GENESIS,
        });
    }

    // Collect all uncommitted ancestors (from highest to lowest)
    let mut to_commit = Vec::new();
    let mut current = commit_block;
    loop {
        if current.height <= *last_committed_height {
            break;
        }
        let parent_hash = current.parent_hash;
        let current_height = current.height;
        to_commit.push(current);
        if parent_hash == BlockHash::GENESIS {
            break;
        }
        match store.get_block(&parent_hash) {
            Some(parent) => current = parent,
            None => {
                // If the missing ancestor is above last committed + 1, the store
                // is corrupt or incomplete — we must not silently skip blocks.
                if current_height > Height(last_committed_height.as_u64() + 1) {
                    return Err(eg!(
                        "missing ancestor block {} for height {} (last committed: {})",
                        parent_hash,
                        current_height,
                        last_committed_height
                    ));
                }
                break;
            }
        }
    }

    // Commit from lowest height to highest
    to_commit.reverse();

    let mut pending_epoch = None;
    let mut last_app_hash = BlockHash::GENESIS;

    for block in &to_commit {
        let ctx = BlockContext {
            height: block.height,
            view: block.view,
            proposer: block.proposer,
            epoch: current_epoch.number,
            epoch_start_view: current_epoch.start_view,
            validator_set: &current_epoch.validator_set,
        };

        info!(height = block.height.as_u64(), hash = %block.hash, "committing block");

        let txs = decode_payload(&block.payload);
        let response = app
            .execute_block(&txs, &ctx)
            .c(d!("execute_block failed"))?;

        app.on_commit(block, &ctx)
            .c(d!("application commit failed"))?;

        // When the application does not track state roots, carry the block's
        // authoritative app_hash forward so the engine state stays coherent
        // with the chain even when NoopApplication always returns GENESIS.
        last_app_hash = if app.tracks_app_hash() {
            response.app_hash
        } else {
            block.app_hash
        };

        if !response.validator_updates.is_empty() {
            let new_vs = current_epoch
                .validator_set
                .apply_updates(&response.validator_updates);
            let epoch_start = ViewNumber(block.view.as_u64() + 2);
            pending_epoch = Some(Epoch::new(current_epoch.number.next(), epoch_start, new_vs));
        }

        *last_committed_height = block.height;
    }

    Ok(CommitResult {
        committed_blocks: to_commit,
        commit_qc: double_cert.inner_qc.clone(),
        pending_epoch,
        last_app_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::NoopApplication;
    use crate::store::MemoryBlockStore;
    use hotmint_types::crypto::PublicKey;
    use hotmint_types::validator::{ValidatorInfo, ValidatorSet};
    use hotmint_types::{AggregateSignature, QuorumCertificate, ValidatorId, ViewNumber};

    fn make_block(height: u64, parent: BlockHash) -> Block {
        let hash = BlockHash([height as u8; 32]);
        Block {
            height: Height(height),
            parent_hash: parent,
            view: ViewNumber(height),
            proposer: ValidatorId(0),
            payload: vec![],
            app_hash: BlockHash::GENESIS,
            hash,
        }
    }

    fn make_qc(hash: BlockHash, view: u64) -> QuorumCertificate {
        QuorumCertificate {
            block_hash: hash,
            view: ViewNumber(view),
            aggregate_signature: AggregateSignature::new(4),
        }
    }

    fn make_epoch() -> Epoch {
        let vs = ValidatorSet::new(vec![ValidatorInfo {
            id: ValidatorId(0),
            public_key: PublicKey(vec![0]),
            power: 1,
        }]);
        Epoch::genesis(vs)
    }

    #[test]
    fn test_commit_single_block() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let epoch = make_epoch();
        let b1 = make_block(1, BlockHash::GENESIS);
        store.put_block(b1.clone());

        let dc = DoubleCertificate {
            inner_qc: make_qc(b1.hash, 1),
            outer_qc: make_qc(b1.hash, 1),
        };

        let mut last = Height::GENESIS;
        let result = try_commit(&dc, &store, &app, &mut last, &epoch).unwrap();
        assert_eq!(result.committed_blocks.len(), 1);
        assert_eq!(result.committed_blocks[0].height, Height(1));
        assert_eq!(last, Height(1));
        assert!(result.pending_epoch.is_none());
    }

    #[test]
    fn test_commit_chain_of_blocks() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let epoch = make_epoch();
        let b1 = make_block(1, BlockHash::GENESIS);
        let b2 = make_block(2, b1.hash);
        let b3 = make_block(3, b2.hash);
        store.put_block(b1);
        store.put_block(b2);
        store.put_block(b3.clone());

        let dc = DoubleCertificate {
            inner_qc: make_qc(b3.hash, 3),
            outer_qc: make_qc(b3.hash, 3),
        };

        let mut last = Height::GENESIS;
        let result = try_commit(&dc, &store, &app, &mut last, &epoch).unwrap();
        assert_eq!(result.committed_blocks.len(), 3);
        assert_eq!(result.committed_blocks[0].height, Height(1));
        assert_eq!(result.committed_blocks[1].height, Height(2));
        assert_eq!(result.committed_blocks[2].height, Height(3));
        assert_eq!(last, Height(3));
    }

    #[test]
    fn test_commit_already_committed() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let epoch = make_epoch();
        let b1 = make_block(1, BlockHash::GENESIS);
        store.put_block(b1.clone());

        let dc = DoubleCertificate {
            inner_qc: make_qc(b1.hash, 1),
            outer_qc: make_qc(b1.hash, 1),
        };

        let mut last = Height(1);
        let result = try_commit(&dc, &store, &app, &mut last, &epoch).unwrap();
        assert!(result.committed_blocks.is_empty());
    }

    #[test]
    fn test_commit_partial_chain() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let epoch = make_epoch();
        let b1 = make_block(1, BlockHash::GENESIS);
        let b2 = make_block(2, b1.hash);
        let b3 = make_block(3, b2.hash);
        store.put_block(b1);
        store.put_block(b2);
        store.put_block(b3.clone());

        let dc = DoubleCertificate {
            inner_qc: make_qc(b3.hash, 3),
            outer_qc: make_qc(b3.hash, 3),
        };

        let mut last = Height(1);
        let result = try_commit(&dc, &store, &app, &mut last, &epoch).unwrap();
        assert_eq!(result.committed_blocks.len(), 2);
        assert_eq!(result.committed_blocks[0].height, Height(2));
        assert_eq!(result.committed_blocks[1].height, Height(3));
    }

    #[test]
    fn test_commit_missing_block() {
        let store = MemoryBlockStore::new();
        let app = NoopApplication;
        let epoch = make_epoch();
        let dc = DoubleCertificate {
            inner_qc: make_qc(BlockHash([99u8; 32]), 1),
            outer_qc: make_qc(BlockHash([99u8; 32]), 1),
        };
        let mut last = Height::GENESIS;
        assert!(try_commit(&dc, &store, &app, &mut last, &epoch).is_err());
    }

    #[test]
    fn test_decode_payload_empty() {
        assert!(decode_payload(&[]).is_empty());
    }

    #[test]
    fn test_decode_payload_roundtrip() {
        // Encode: 4-byte LE length prefix + data
        let mut payload = Vec::new();
        let tx1 = b"hello";
        let tx2 = b"world";
        payload.extend_from_slice(&(tx1.len() as u32).to_le_bytes());
        payload.extend_from_slice(tx1);
        payload.extend_from_slice(&(tx2.len() as u32).to_le_bytes());
        payload.extend_from_slice(tx2);

        let txs = decode_payload(&payload);
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0], b"hello");
        assert_eq!(txs[1], b"world");
    }
}
