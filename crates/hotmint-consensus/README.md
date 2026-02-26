# hotmint-consensus

[![crates.io](https://img.shields.io/crates/v/hotmint-consensus.svg)](https://crates.io/crates/hotmint-consensus)
[![docs.rs](https://docs.rs/hotmint-consensus/badge.svg)](https://docs.rs/hotmint-consensus)

HotStuff-2 consensus state machine and engine for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT framework.

This is the core crate of Hotmint. It implements the full HotStuff-2 protocol — two-chain commit, five-step view protocol, pacemaker with exponential backoff — and is completely decoupled from I/O through pluggable trait interfaces.

## Architecture

```
ConsensusEngine
  ├── ConsensusState      mutable state (view, locks, role)
  ├── view_protocol       steady-state protocol (Paper Figure 1)
  ├── pacemaker           timeout & view change (Paper Figure 2)
  ├── vote_collector      vote aggregation & QC formation
  ├── commit              two-chain commit rule
  └── leader              round-robin leader election
```

## Pluggable Traits

| Trait | Purpose | Built-in Stub |
|:------|:--------|:--------------|
| `Application` | ABCI-like app lifecycle | `NoopApplication` |
| `BlockStore` | Block persistence | `MemoryBlockStore` |
| `NetworkSink` | Message transport | `ChannelNetwork` |

## Usage

```rust
use hotmint_consensus::engine::ConsensusEngine;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::application::NoopApplication;

let engine = ConsensusEngine::new(
    ConsensusState::new(vid, validator_set),
    std::sync::Arc::new(std::sync::RwLock::new(Box::new(MemoryBlockStore::new()))),
    Box::new(ChannelNetwork::new(vid, senders)),
    Box::new(NoopApplication),
    Box::new(signer),
    msg_rx,
);

tokio::spawn(async move { engine.run().await });
```

### Implement Application

All methods have default no-op implementations. Lifecycle: `execute_block(txs, ctx)` → `on_commit(block, ctx)`.

```rust
use ruc::*;
use hotmint_types::Block;
use hotmint_consensus::application::Application;

struct MyApp;

impl Application for MyApp {
    fn on_commit(&self, block: &Block, _ctx: &hotmint_types::context::BlockContext) -> Result<()> {
        println!("committed height {}", block.height.as_u64());
        Ok(())
    }
}
```

### Prometheus Metrics

```rust
use prometheus_client::registry::Registry;
use hotmint_consensus::metrics::ConsensusMetrics;

let mut registry = Registry::default();
let metrics = ConsensusMetrics::new(&mut registry);
// Exposes: hotmint_blocks_committed, hotmint_votes_sent,
//          hotmint_view_timeouts, hotmint_view_duration_seconds, ...
```

## License

GPL-3.0-only
