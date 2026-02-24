# hotmint-storage

[![crates.io](https://img.shields.io/crates/v/hotmint-storage.svg)](https://crates.io/crates/hotmint-storage)
[![docs.rs](https://docs.rs/hotmint-storage/badge.svg)](https://docs.rs/hotmint-storage)

Persistent storage backends for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Implements the `BlockStore` trait from `hotmint-consensus` using [vsdb](https://crates.io/crates/vsdb) (a high-performance embedded KV database backed by MMDB), and provides `PersistentConsensusState` for crash recovery of critical consensus state.

## Components

| Component | Description |
|:----------|:------------|
| `VsdbBlockStore` | Persistent block storage backed by vsdb `MapxOrd` |
| `PersistentConsensusState` | Persists view number, locked QC, highest QC, committed height |

## Usage

### Block Store

```rust
use hotmint_consensus::store::BlockStore;
use hotmint_storage::block_store::VsdbBlockStore;

let store = VsdbBlockStore::new();
// genesis block is inserted automatically

// use as a drop-in replacement for MemoryBlockStore
let engine = ConsensusEngine::new(
    state,
    Box::new(store),
    // ...
);
```

### Crash Recovery

```rust
use hotmint_consensus::state::ConsensusState;
use hotmint_storage::consensus_state::PersistentConsensusState;

let pstate = PersistentConsensusState::new();

// restore after restart
let mut state = ConsensusState::new(vid, validator_set);
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
```

### Data Directory

vsdb stores data in the process working directory by default. Configure a custom location via environment variable or programmatically:

```bash
export VSDB_BASE_DIR=/var/lib/hotmint/data
```

```rust
// must be called before any vsdb operation, can only be called once
vsdb::vsdb_set_base_dir("/var/lib/hotmint/data").unwrap();
```

## License

GPL-3.0-only
