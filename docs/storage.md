# Storage

Hotmint provides two `BlockStore` implementations: an in-memory store for development/testing and a vsdb-backed persistent store for production.

## BlockStore Trait

```rust
pub trait BlockStore: Send + Sync {
    fn put_block(&mut self, block: Block);
    fn get_block(&self, hash: &BlockHash) -> Option<Block>;
    fn get_block_by_height(&self, h: Height) -> Option<Block>;
}
```

The trait returns owned `Block` values (not references) for compatibility with vsdb, which stores data in RocksDB and cannot return borrowed references.

## MemoryBlockStore

An in-memory implementation backed by `HashMap` and `BTreeMap`. Suitable for testing, development, and short-lived processes.

```rust
use hotmint::consensus::store::MemoryBlockStore;

let store = MemoryBlockStore::new();
// automatically contains the genesis block at height 0
```

Internally:
- `by_hash: HashMap<BlockHash, Block>` — O(1) lookup by hash
- `by_height: BTreeMap<u64, BlockHash>` — ordered lookup by height

## VsdbBlockStore

Persistent block storage backed by vsdb (`MapxOrd` over RocksDB). Blocks survive process restarts.

```rust
use hotmint::storage::block_store::VsdbBlockStore;

let store = VsdbBlockStore::new();
// automatically contains the genesis block

// check if a block exists
if store.contains(&block_hash) {
    // ...
}

// explicitly flush to disk
store.flush();
```

vsdb manages its own data directory. By default it stores data under the process working directory. Set the `VSDB_BASE_DIR` environment variable to control the location:

```bash
export VSDB_BASE_DIR=/var/lib/hotmint/data
```

### Using in ConsensusEngine

```rust
let engine = ConsensusEngine::new(
    state,
    Box::new(VsdbBlockStore::new()),  // swap in persistent storage
    Box::new(network_sink),
    Box::new(app),
    Box::new(signer),
    msg_rx,
);
```

## PersistentConsensusState

Critical consensus state (view number, locked QC, highest QC, committed height) must survive crashes to maintain safety. `PersistentConsensusState` persists these values to vsdb.

```rust
use hotmint::storage::consensus_state::PersistentConsensusState;

let mut pstate = PersistentConsensusState::new();

// save state (typically called after each view transition or commit)
pstate.save_current_view(ViewNumber(42));
pstate.save_locked_qc(&qc);
pstate.save_highest_qc(&highest_qc);
pstate.save_last_committed_height(Height(10));
pstate.flush();

// load state (on startup / crash recovery)
let view = pstate.load_current_view();           // Option<ViewNumber>
let locked = pstate.load_locked_qc();            // Option<QuorumCertificate>
let highest = pstate.load_highest_qc();          // Option<QuorumCertificate>
let committed = pstate.load_last_committed_height(); // Option<Height>
```

### Crash Recovery Example

```rust
use hotmint::consensus::state::ConsensusState;
use hotmint::storage::block_store::VsdbBlockStore;
use hotmint::storage::consensus_state::PersistentConsensusState;

fn recover_or_init(vid: ValidatorId, vs: ValidatorSet) -> (ConsensusState, VsdbBlockStore) {
    let store = VsdbBlockStore::new();
    let pstate = PersistentConsensusState::new();

    let mut state = ConsensusState::new(vid, vs);

    // restore persisted state if available
    if let Some(view) = pstate.load_current_view() {
        state.current_view = view;
    }
    if let Some(qc) = pstate.load_locked_qc() {
        state.locked_qc = Some(qc);
    }
    if let Some(qc) = pstate.load_highest_qc() {
        state.highest_qc = Some(qc);
    }
    if let Some(h) = pstate.load_last_committed_height() {
        state.last_committed_height = h;
    }

    (state, store)
}
```

## Implementing a Custom BlockStore

To use a different storage backend (e.g., SQLite, sled, a remote database):

```rust
use hotmint::prelude::*;
use hotmint::consensus::store::BlockStore;

struct SqliteBlockStore {
    conn: rusqlite::Connection,
}

impl SqliteBlockStore {
    fn new(path: &str) -> Self {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blocks (
                hash BLOB PRIMARY KEY,
                height INTEGER NOT NULL,
                data BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_height ON blocks(height);"
        ).unwrap();
        Self { conn }
    }
}

impl BlockStore for SqliteBlockStore {
    fn put_block(&mut self, block: Block) {
        let data = rmp_serde::to_vec(&block).unwrap();
        self.conn.execute(
            "INSERT OR REPLACE INTO blocks (hash, height, data) VALUES (?1, ?2, ?3)",
            (&block.hash.0[..], block.height.as_u64() as i64, &data),
        ).unwrap();
    }

    fn get_block(&self, hash: &BlockHash) -> Option<Block> {
        self.conn
            .query_row(
                "SELECT data FROM blocks WHERE hash = ?1",
                [&hash.0[..]],
                |row| {
                    let data: Vec<u8> = row.get(0)?;
                    Ok(rmp_serde::from_slice(&data).unwrap())
                },
            )
            .ok()
    }

    fn get_block_by_height(&self, h: Height) -> Option<Block> {
        self.conn
            .query_row(
                "SELECT data FROM blocks WHERE height = ?1",
                [h.as_u64() as i64],
                |row| {
                    let data: Vec<u8> = row.get(0)?;
                    Ok(rmp_serde::from_slice(&data).unwrap())
                },
            )
            .ok()
    }
}
```
