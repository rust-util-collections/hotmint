use std::collections::{BTreeMap, HashMap};

use hotmint_types::{Block, BlockHash, Height};

pub trait BlockStore: Send + Sync {
    fn put_block(&mut self, block: Block);
    fn get_block(&self, hash: &BlockHash) -> Option<Block>;
    fn get_block_by_height(&self, h: Height) -> Option<Block>;
}

/// In-memory block store stub
pub struct MemoryBlockStore {
    by_hash: HashMap<BlockHash, Block>,
    by_height: BTreeMap<u64, BlockHash>,
}

impl Default for MemoryBlockStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryBlockStore {
    pub fn new() -> Self {
        let mut store = Self {
            by_hash: HashMap::new(),
            by_height: BTreeMap::new(),
        };
        let genesis = Block::genesis();
        store.put_block(genesis);
        store
    }
}

impl BlockStore for MemoryBlockStore {
    fn put_block(&mut self, block: Block) {
        let hash = block.hash;
        self.by_height.insert(block.height.as_u64(), hash);
        self.by_hash.insert(hash, block);
    }

    fn get_block(&self, hash: &BlockHash) -> Option<Block> {
        self.by_hash.get(hash).cloned()
    }

    fn get_block_by_height(&self, h: Height) -> Option<Block> {
        self.by_height
            .get(&h.as_u64())
            .and_then(|hash| self.by_hash.get(hash))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotmint_types::{ValidatorId, ViewNumber};

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

    #[test]
    fn test_genesis_present() {
        let store = MemoryBlockStore::new();
        let genesis = store.get_block(&BlockHash::GENESIS);
        assert!(genesis.is_some());
        assert_eq!(genesis.unwrap().height, Height::GENESIS);
    }

    #[test]
    fn test_put_and_get_by_hash() {
        let mut store = MemoryBlockStore::new();
        let block = make_block(1, BlockHash::GENESIS);
        let hash = block.hash;
        store.put_block(block);
        let retrieved = store.get_block(&hash).unwrap();
        assert_eq!(retrieved.height, Height(1));
    }

    #[test]
    fn test_get_by_height() {
        let mut store = MemoryBlockStore::new();
        let b1 = make_block(1, BlockHash::GENESIS);
        let b2 = make_block(2, b1.hash);
        store.put_block(b1);
        store.put_block(b2);

        assert!(store.get_block_by_height(Height(1)).is_some());
        assert!(store.get_block_by_height(Height(2)).is_some());
        assert!(store.get_block_by_height(Height(99)).is_none());
    }

    #[test]
    fn test_get_nonexistent() {
        let store = MemoryBlockStore::new();
        assert!(store.get_block(&BlockHash([99u8; 32])).is_none());
        assert!(store.get_block_by_height(Height(999)).is_none());
    }

    #[test]
    fn test_overwrite_same_height() {
        let mut store = MemoryBlockStore::new();
        let b1 = make_block(1, BlockHash::GENESIS);
        store.put_block(b1);
        // Different block at same height (different hash)
        let mut b2 = make_block(1, BlockHash::GENESIS);
        b2.hash = BlockHash([42u8; 32]);
        b2.payload = vec![1, 2, 3];
        store.put_block(b2);
        // Height now points to new block
        let retrieved = store.get_block_by_height(Height(1)).unwrap();
        assert_eq!(retrieved.hash, BlockHash([42u8; 32]));
    }
}
