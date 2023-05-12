use hotmint_types::{Block, BlockHash};

pub fn hash_block(block: &Block) -> BlockHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&block.height.as_u64().to_le_bytes());
    hasher.update(&block.parent_hash.0);
    hasher.update(&block.view.as_u64().to_le_bytes());
    hasher.update(&block.proposer.0.to_le_bytes());
    hasher.update(&block.payload);
    let hash = hasher.finalize();
    BlockHash(*hash.as_bytes())
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
            hash: BlockHash::GENESIS,
        };
        let h1 = hash_block(&block);
        let h2 = hash_block(&block);
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
            hash: BlockHash::GENESIS,
        };
        let b2 = Block {
            height: Height(1),
            parent_hash: BlockHash::GENESIS,
            view: ViewNumber(1),
            proposer: ValidatorId(0),
            payload: b"b".to_vec(),
            hash: BlockHash::GENESIS,
        };
        assert_ne!(hash_block(&b1), hash_block(&b2));
    }
}
