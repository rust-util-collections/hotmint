use hotmint_types::{Block, BlockHash};

/// Compute the Blake3 hash of a block's content fields.
///
/// Hashes `height || parent_hash || view || proposer || app_hash || payload`,
/// deliberately excluding `block.hash` to avoid circularity.
pub fn compute_block_hash(block: &Block) -> BlockHash {
    block.compute_hash()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotmint_types::{Height, ValidatorId, ViewNumber};

    #[test]
    fn test_hash_deterministic() {
        let block = Block {
            height: Height(1),
            parent_hash: BlockHash::GENESIS,
            view: ViewNumber(1),
            proposer: ValidatorId(0),
            payload: b"hello".to_vec(),
            app_hash: BlockHash::GENESIS,
            hash: BlockHash::GENESIS,
        };
        let h1 = compute_block_hash(&block);
        let h2 = compute_block_hash(&block);
        assert_eq!(h1, h2);
        assert!(!h1.is_genesis());
    }

    #[test]
    fn test_different_blocks_different_hashes() {
        let b1 = Block {
            height: Height(1),
            parent_hash: BlockHash::GENESIS,
            view: ViewNumber(1),
            proposer: ValidatorId(0),
            payload: b"a".to_vec(),
            app_hash: BlockHash::GENESIS,
            hash: BlockHash::GENESIS,
        };
        let b2 = Block {
            height: Height(1),
            parent_hash: BlockHash::GENESIS,
            view: ViewNumber(1),
            proposer: ValidatorId(0),
            payload: b"b".to_vec(),
            app_hash: BlockHash::GENESIS,
            hash: BlockHash::GENESIS,
        };
        assert_ne!(compute_block_hash(&b1), compute_block_hash(&b2));
    }

    #[test]
    fn test_block_compute_hash_matches() {
        let block = Block {
            height: Height(1),
            parent_hash: BlockHash::GENESIS,
            view: ViewNumber(1),
            proposer: ValidatorId(0),
            payload: b"hello".to_vec(),
            app_hash: BlockHash::GENESIS,
            hash: BlockHash::GENESIS,
        };
        assert_eq!(compute_block_hash(&block), block.compute_hash());
    }
}
