use hotmint_consensus::store::BlockStore;
use hotmint_types::{Block, BlockHash, Height};
use tracing::debug;
use vsdb::MapxOrd;

/// Persistent block store backed by vsdb (rocksdb)
pub struct VsdbBlockStore {
    by_hash: MapxOrd<[u8; 32], Block>,
    by_height: MapxOrd<u64, [u8; 32]>,
}

impl VsdbBlockStore {
    pub fn new() -> Self {
        let mut store = Self {
            by_hash: MapxOrd::new(),
            by_height: MapxOrd::new(),
        };
        let genesis = Block::genesis();
        store.put_block(genesis);
        store
    }

    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.by_hash.contains_key(&hash.0)
    }

    pub fn flush(&self) {
        vsdb::vsdb_flush();
    }
}

impl Default for VsdbBlockStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockStore for VsdbBlockStore {
    fn put_block(&mut self, block: Block) {
        debug!(height = block.height.as_u64(), hash = %block.hash, "storing block to vsdb");
        self.by_height.insert(&block.height.as_u64(), &block.hash.0);
        self.by_hash.insert(&block.hash.0, &block);
    }

    fn get_block(&self, hash: &BlockHash) -> Option<Block> {
        self.by_hash.get(&hash.0)
    }

    fn get_block_by_height(&self, h: Height) -> Option<Block> {
        self.by_height
            .get(&h.as_u64())
            .and_then(|hash_bytes| self.by_hash.get(&hash_bytes))
    }
}
