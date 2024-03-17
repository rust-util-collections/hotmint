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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::NoopApplication;
    use crate::store::MemoryBlockStore;
    use hotmint_types::{AggregateSignature, QuorumCertificate, ValidatorId, ViewNumber};

    fn make_block(height: u64, parent: BlockHash) -> Block {
        let hash = BlockHash([height as u8; 32]);
        Block {
            height: Height(height),
            parent_hash: parent,
            view: ViewNumber(height),
            proposer: ValidatorId(0),
            payload: vec![],
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

    #[test]
    fn test_commit_single_block() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let b1 = make_block(1, BlockHash::GENESIS);
        store.put_block(b1.clone());

        let dc = DoubleCertificate {
            inner_qc: make_qc(b1.hash, 1),
            outer_qc: make_qc(b1.hash, 1),
        };

        let mut last = Height::GENESIS;
        let committed = try_commit(&dc, &store, &app, &mut last).unwrap();
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].height, Height(1));
        assert_eq!(last, Height(1));
    }

    #[test]
    fn test_commit_chain_of_blocks() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
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
        let committed = try_commit(&dc, &store, &app, &mut last).unwrap();
        assert_eq!(committed.len(), 3);
        assert_eq!(committed[0].height, Height(1));
        assert_eq!(committed[1].height, Height(2));
        assert_eq!(committed[2].height, Height(3));
        assert_eq!(last, Height(3));
    }

    #[test]
    fn test_commit_already_committed() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let b1 = make_block(1, BlockHash::GENESIS);
        store.put_block(b1.clone());

        let dc = DoubleCertificate {
            inner_qc: make_qc(b1.hash, 1),
            outer_qc: make_qc(b1.hash, 1),
        };

        let mut last = Height(1); // already committed
        let committed = try_commit(&dc, &store, &app, &mut last).unwrap();
        assert!(committed.is_empty());
    }

    #[test]
    fn test_commit_partial_chain() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
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

        let mut last = Height(1); // b1 already committed
        let committed = try_commit(&dc, &store, &app, &mut last).unwrap();
        assert_eq!(committed.len(), 2); // only b2 and b3
        assert_eq!(committed[0].height, Height(2));
        assert_eq!(committed[1].height, Height(3));
    }

    #[test]
    fn test_commit_missing_block() {
        let store = MemoryBlockStore::new();
        let app = NoopApplication;
        let dc = DoubleCertificate {
            inner_qc: make_qc(BlockHash([99u8; 32]), 1),
            outer_qc: make_qc(BlockHash([99u8; 32]), 1),
        };
        let mut last = Height::GENESIS;
        assert!(try_commit(&dc, &store, &app, &mut last).is_err());
    }
}
