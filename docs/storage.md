# Storage

Hotmint provides two `BlockStore` implementations and a `PersistentConsensusState` for block persistence and consensus state crash recovery, respectively.

| Component | Purpose | Backend |
|:----------|:--------|:--------|
| `MemoryBlockStore` | Development / testing | HashMap + BTreeMap |
| `VsdbBlockStore` | Production | vsdb MapxOrd |
| `PersistentConsensusState` | Consensus state crash recovery | vsdb MapxOrd |

## vsdb Overview

[vsdb](https://crates.io/crates/vsdb) is a high-performance embedded key-value database whose API mirrors Rust standard collections (HashMap / BTreeMap). Under the hood it uses MMDB (a pure-Rust memory-mapped database engine) with no C library dependencies.

### Core Types

Core vsdb v10.x types used by Hotmint:

| Type | Description | Rust Equivalent |
|:-----|:------------|:----------------|
| `MapxOrd<K, V>` | Ordered KV store | `BTreeMap<K, V>` |
| `Mapx<K, V>` | Unordered KV store | `HashMap<K, V>` |
| `Orphan<T>` | Single-value persistent container | `Box<T>` on disk |

Common `MapxOrd` methods:

```rust
// Create
let mut map: MapxOrd<u64, String> = MapxOrd::new();

// Write
map.insert(&1, &"hello".into());

// Read
let val: Option<String> = map.get(&1);
let exists: bool = map.contains_key(&1);

// Range queries
let first: Option<(u64, String)> = map.first();
let last: Option<(u64, String)> = map.last();
let le: Option<(u64, String)> = map.get_le(&5);  // last entry ≤ 5
let ge: Option<(u64, String)> = map.get_ge(&5);  // first entry ≥ 5

// Iteration
for (k, v) in map.iter() { /* ... */ }
for (k, v) in map.range(10..20) { /* ... */ }

// Delete
map.remove(&1);
map.clear();
```

### Serialization Requirements

- Keys must implement `KeyEnDeOrdered` (ordered encoding)
- Values must implement `ValueEnDe`
- Any type implementing serde `Serialize + Deserialize` automatically satisfies both requirements

### Key Functions

```rust
// Set the data directory (must be called before any vsdb operation; can only be called once)
vsdb::vsdb_set_base_dir("/var/lib/hotmint/data").unwrap();

// Get the current data directory
let dir = vsdb::vsdb_get_base_dir();

// Force flush to disk
vsdb::vsdb_flush();
```

## BlockStore Trait

```rust
pub trait BlockStore: Send + Sync {
    fn put_block(&mut self, block: Block);
    fn get_block(&self, hash: &BlockHash) -> Option<Block>;
    fn get_block_by_height(&self, h: Height) -> Option<Block>;

    /// Get blocks in [from, to] inclusive. Default iterates one-by-one.
    fn get_blocks_in_range(&self, from: Height, to: Height) -> Vec<Block> { /* default */ }

    /// Return the highest stored block height. Default returns genesis.
    fn tip_height(&self) -> Height { Height::GENESIS }
}
```

The trait returns owned `Block` values (not references) because vsdb stores data on disk and cannot hand out borrowed references into memory. This design lets in-memory and persistent implementations share the same interface.

## MemoryBlockStore

An in-memory implementation suited for testing, development, and short-lived processes.

```rust
use hotmint::consensus::store::MemoryBlockStore;

let store = MemoryBlockStore::new();
// Automatically includes the genesis block at height 0
```

For convenience, a thread-safe shared instance can be created in one step:

```rust
let shared_store = MemoryBlockStore::new_shared();
// Returns Arc<RwLock<Box<dyn BlockStore>>>
```

Internal structure:
- `by_hash: HashMap<BlockHash, Block>` — O(1) hash lookup
- `by_height: BTreeMap<u64, BlockHash>` — ordered height lookup

## VsdbBlockStore

A persistent block store backed by vsdb `MapxOrd`. Blocks survive process restarts.

```rust
use hotmint::storage::block_store::VsdbBlockStore;

let store = VsdbBlockStore::new();
// Automatically includes the genesis block

// Check whether a block exists
if store.contains(&block_hash) {
    // ...
}

// Explicitly flush to disk
store.flush();
```

### Internal Data Model

```rust
pub struct VsdbBlockStore {
    by_hash: MapxOrd<[u8; 32], Block>,     // BlockHash → Block
    by_height: MapxOrd<u64, [u8; 32]>,     // Height → BlockHash
}
```

The two indexes work together:
- `put_block()` writes to both maps
- `get_block()` looks up directly in `by_hash`
- `get_block_by_height()` resolves the hash via `by_height`, then fetches the block from `by_hash`

### Using with ConsensusEngine

```rust
use std::sync::{Arc, RwLock};
use hotmint::consensus::engine::SharedBlockStore;
use hotmint::crypto::Ed25519Verifier;

let store: SharedBlockStore =
    Arc::new(RwLock::new(Box::new(VsdbBlockStore::new())));

let engine = ConsensusEngine::builder()
    .state(state)
    .block_store(store)        // SharedBlockStore = Arc<RwLock<Box<dyn BlockStore>>>
    .network_sink(network_sink)
    .application(app)
    .signer(signer)
    .message_receiver(msg_rx)
    .verifier(Ed25519Verifier)
    .build();
```

## PersistentConsensusState

Critical consensus state (view number, locked QC, highest QC, committed height, current epoch) must be recovered after a crash to maintain safety.

### Internal Data Model

```rust
// Multiple state fields stored in a single MapxOrd
pub struct PersistentConsensusState {
    store: MapxOrd<u64, StateValue>,
}

// State value enum (serialized via serde)
enum StateValue {
    View(ViewNumber),
    Height(Height),
    Qc(QuorumCertificate),
    Epoch(Epoch),
}

// Fixed key constants
const KEY_CURRENT_VIEW: u64 = 1;
const KEY_LOCKED_QC: u64 = 2;
const KEY_HIGHEST_QC: u64 = 3;
const KEY_LAST_COMMITTED_HEIGHT: u64 = 4;
const KEY_CURRENT_EPOCH: u64 = 5;
```

### API

```rust
use hotmint::storage::consensus_state::PersistentConsensusState;

let mut pstate = PersistentConsensusState::new();

// Save state (typically called after view changes or commits)
pstate.save_current_view(ViewNumber(42));
pstate.save_locked_qc(&qc);
pstate.save_highest_qc(&highest_qc);
pstate.save_last_committed_height(Height(10));
pstate.save_current_epoch(&epoch);
pstate.flush();

// Load state (at startup / crash recovery)
let view = pstate.load_current_view();           // Option<ViewNumber>
let locked = pstate.load_locked_qc();            // Option<QuorumCertificate>
let highest = pstate.load_highest_qc();          // Option<QuorumCertificate>
let committed = pstate.load_last_committed_height(); // Option<Height>
let epoch = pstate.load_current_epoch();         // Option<Epoch>
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

    // Restore from persisted state
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
    if let Some(epoch) = pstate.load_current_epoch() {
        state.current_epoch = epoch;
    }

    (state, store)
}
```

## Data Directory Configuration

By default vsdb stores data in the process working directory. There are two ways to specify a custom path:

### Environment Variable

```bash
export VSDB_BASE_DIR=/var/lib/hotmint/data
```

### Programmatic Configuration

```rust
// Must be called before any vsdb operation; can only be called once
vsdb::vsdb_set_base_dir("/var/lib/hotmint/data").unwrap();
```

`vsdb_set_base_dir()` accepts `impl AsRef<Path>` and returns an error if the database has already been initialized.

## Flush Semantics

By default vsdb writes are flushed asynchronously (the OS decides when to persist). Calling `vsdb_flush()` forces all pending writes to be synchronously flushed to disk.

Recommended flush points:
- After critical consensus state changes (view switches, QC updates, commits)
- After the application's `on_commit()` completes
- Before a graceful node shutdown

Both `VsdbBlockStore` and `PersistentConsensusState` expose a `.flush()` method that internally calls `vsdb::vsdb_flush()`.

## Advanced vsdb Features

Beyond basic KV storage, vsdb v10.x offers several advanced features that may be useful for future Hotmint extensions:

### VerMap — Versioned Storage

`VerMap` provides Git-style versioned storage with support for branching, committing, three-way merging, and rollback.

```rust
use vsdb::versioned::map::VerMap;

let mut m: VerMap<u32, String> = VerMap::new();
let main = m.main_branch();

m.insert(main, &1, &"hello".into())?;
m.commit(main)?;

let feat = m.create_branch("feature", main)?;
m.insert(feat, &1, &"updated".into())?;
m.commit(feat)?;

// Branches isolate changes
assert_eq!(m.get(main, &1)?, Some("hello".into()));
assert_eq!(m.get(feat, &1)?, Some("updated".into()));

// Three-way merge
m.merge(feat, main)?;
```

Potential use case: optimistic execution and rollback of application state.

### MptCalc / SmtCalc — Merkle Proofs

`MptCalc` (Merkle Patricia Trie) and `SmtCalc` (Sparse Merkle Tree) provide stateless Merkle root computation and proof generation.

`VerMapWithProof` combines versioned storage with Merkle root computation, producing a 32-byte state root on each commit.

Potential use cases:
- Light client state verification
- Cross-chain state proofs
- Application-layer state commitments

## Implementing a Custom BlockStore

To use a different storage backend (e.g., SQLite, sled, or a remote database):

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
        let hash = compute_block_hash(&block);
        let data = serde_cbor_2::to_vec(&block).unwrap();
        self.conn.execute(
            "INSERT OR REPLACE INTO blocks (hash, height, data) VALUES (?1, ?2, ?3)",
            (&hash.0[..], block.height.as_u64() as i64, &data),
        ).unwrap();
    }

    fn get_block(&self, hash: &BlockHash) -> Option<Block> {
        self.conn
            .query_row(
                "SELECT data FROM blocks WHERE hash = ?1",
                [&hash.0[..]],
                |row| {
                    let data: Vec<u8> = row.get(0)?;
                    Ok(serde_cbor_2::from_slice(&data).unwrap())
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
                    Ok(serde_cbor_2::from_slice(&data).unwrap())
                },
            )
            .ok()
    }
}
```
