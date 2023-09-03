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
