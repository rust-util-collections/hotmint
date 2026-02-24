# Hotmint

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_edition-orange.svg)](https://www.rust-lang.org/)
[![CI](https://github.com/rust-util-collections/hotmint/actions/workflows/ci.yml/badge.svg)](https://github.com/rust-util-collections/hotmint/actions/workflows/ci.yml)
[![HotStuff-2](https://img.shields.io/badge/protocol-HotStuff--2-purple.svg)](https://arxiv.org/abs/2301.03253)

A Rust BFT consensus framework combining Tendermint's engineering ergonomics with HotStuff-2's protocol efficiency.

| Feature | Progress | Status |
|:--------|:---------|:-------|
| Core Types & Crypto | 5/5 | ✅ |
| Persistent Storage | 3/3 | ✅ |
| P2P Networking | 4/4 | ✅ |
| Full Pacemaker | 4/4 | ✅ |
| Production Hardening | 3/3 | ✅ |
| Application Framework | 3/3 | ✅ |
| **Total** | **22/22** | **100%** |

## Design Goals

Hotmint is a BFT consensus framework built from scratch. It retains the clean, modular architecture of Tendermint while adopting HotStuff-2's two-chain commit protocol for lower confirmation latency and simpler view-change mechanics.

**Core design philosophy:**

- **Protocol**: HotStuff-2 two-chain commit + simplified view change, replacing Tendermint's three-phase voting
- **Architecture**: Tendermint-inspired modular design (consensus / network / application separation) with clean trait boundaries
- **Storage**: [vsdb](https://crates.io/crates/vsdb) as the persistence backend, with Merkle proof support (MPT/SMT)
- **Networking**: [litep2p](https://crates.io/crates/litep2p) as the P2P foundation, lighter weight than libp2p
- **Error handling**: [ruc](https://crates.io/crates/ruc) chained error tracing

## Protocol

Hotmint implements the HotStuff-2 two-chain commit protocol ([arXiv:2301.03253](https://arxiv.org/abs/2301.03253)):

```
Block  <──  QC (2f+1 votes)  <──  Double Cert (2f+1 votes on QC)  ──>  Commit
```

Each view follows a 5-step protocol: Enter → Propose → Vote → Prepare (QC) → Vote2. A double certificate triggers commit of the block and all uncommitted ancestors. View changes use a timeout + wish + TC mechanism with exponential backoff.

📖 **[Full protocol specification →](docs/protocol.md)**

## Architecture

```
hotmint (library facade — re-exports everything)
  ├── hotmint-types      core data types (Block, QC, Vote, ValidatorSet, ...)
  ├── hotmint-crypto     Ed25519 signing + Blake3 hashing
  ├── hotmint-consensus  consensus state machine (engine, pacemaker, vote collector)
  ├── hotmint-storage    persistent storage (vsdb)
  ├── hotmint-network    P2P networking (litep2p)
  ├── hotmint-mempool    transaction mempool (FIFO, dedup)
  └── hotmint-api        JSON-RPC server
```

The consensus engine is decoupled from all I/O through four pluggable traits:

| Trait | Purpose | Built-in Implementations |
|:------|:--------|:-------------------------|
| `Application` | ABCI-like app lifecycle | `NoopApplication` |
| `BlockStore` | Block persistence | `MemoryBlockStore`, `VsdbBlockStore` |
| `NetworkSink` | Message transport | `ChannelNetwork`, `Litep2pNetworkSink` |
| `Signer` | Cryptographic signing | `Ed25519Signer` |

📖 **[Architecture details →](docs/architecture.md)** · **[Core types reference →](docs/types.md)**

## Technology Stack

| Component | Implementation |
|:----------|:---------------|
| Signatures | Ed25519 ([ed25519-dalek](https://crates.io/crates/ed25519-dalek)) |
| Hashing | [Blake3](https://crates.io/crates/blake3) |
| Storage | [vsdb](https://crates.io/crates/vsdb) MapxOrd |
| Networking | [litep2p](https://crates.io/crates/litep2p) |
| Async Runtime | [Tokio](https://crates.io/crates/tokio) |
| Error Handling | [ruc](https://crates.io/crates/ruc) |
| Serialization | [serde](https://crates.io/crates/serde) + [msgpack](https://crates.io/crates/rmp-serde) |
| Metrics | [prometheus-client](https://crates.io/crates/prometheus-client) |

## References

| Paper | Link |
|:------|:-----|
| HotStuff-2: Optimal Two-Chain BFT (2023) | [arXiv:2301.03253](https://arxiv.org/abs/2301.03253) |
| HotStuff: BFT Consensus (PODC 2019) | [arXiv:1803.05069](https://arxiv.org/abs/1803.05069) |
| Tendermint: Latest Gossip on BFT (2018) | [arXiv:1807.04938](https://arxiv.org/abs/1807.04938) |

## Quick Start

```bash
cargo build --workspace && cargo test --workspace

# run the 4-node in-process demo
cargo run --bin hotmint-node
```

📖 **[Getting started guide →](docs/getting-started.md)**

## Documentation

| Guide | Description |
|:------|:------------|
| [Getting Started](docs/getting-started.md) | Installation, quick start, first integration |
| [Protocol](docs/protocol.md) | HotStuff-2 two-chain commit, view protocol, pacemaker |
| [Architecture](docs/architecture.md) | Module structure, dependency graph, design decisions |
| [Application](docs/application.md) | `Application` trait guide with ABCI-like lifecycle |
| [Consensus Engine](docs/consensus-engine.md) | Engine internals: state machine, event loop, vote collection |
| [Core Types](docs/types.md) | Block, QC, DC, TC, Vote, ValidatorSet, wire protocol |
| [Cryptography](docs/crypto.md) | Signer/Verifier traits, Ed25519, aggregate signatures |
| [Storage](docs/storage.md) | BlockStore trait, vsdb persistence, crash recovery |
| [Networking](docs/networking.md) | NetworkSink trait, in-memory channels, litep2p P2P |
| [Mempool & API](docs/mempool-api.md) | Transaction mempool and JSON-RPC server |
| [Metrics](docs/metrics.md) | Prometheus metrics and observability |
| [Production Readiness](docs/production-readiness.md) | Validator lifecycle, staking infrastructure, gap analysis |

## Usage

Add `hotmint` as a dependency in your `Cargo.toml`:

```toml
[dependencies]
hotmint = { git = "https://github.com/rust-util-collections/hotmint" }
tokio = { version = "1", features = ["full"] }
ruc = "9.3"
```

### Implement the Application Trait

Only `on_commit` is required. The lifecycle is: `begin_block(ctx)` → `deliver_tx` (×N) → `end_block(ctx)` → `on_commit(block, ctx)`. All lifecycle methods receive a `BlockContext` with height, view, proposer, epoch number, and the current validator set.

```rust
use ruc::*;
use hotmint::prelude::*;
use hotmint::consensus::application::Application;

struct MyApp;

impl Application for MyApp {
    fn on_commit(&self, block: &Block, _ctx: &BlockContext) -> Result<()> {
        println!("committed block at height {}", block.height.as_u64());
        Ok(())
    }
}
```

Override other methods as needed:

```rust
impl Application for MyApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        // called when this validator is the leader;
        // return the block payload (e.g. serialized transactions)
        vec![]
    }

    fn validate_block(&self, block: &Block, _ctx: &BlockContext) -> bool {
        // validate a proposed block before voting
        !block.payload.is_empty()
    }

    fn validate_tx(&self, tx: &[u8]) -> bool {
        // validate an individual transaction (used by mempool)
        tx.len() <= 1024
    }

    fn begin_block(&self, ctx: &BlockContext) -> Result<()> {
        println!("begin block at height {}", ctx.height.as_u64());
        Ok(())
    }

    fn deliver_tx(&self, tx: &[u8]) -> Result<()> {
        println!("deliver tx: {} bytes", tx.len());
        Ok(())
    }

    fn end_block(&self, ctx: &BlockContext) -> Result<EndBlockResponse> {
        println!("end block at height {}", ctx.height.as_u64());
        Ok(EndBlockResponse::default())
    }

    fn on_commit(&self, block: &Block, _ctx: &BlockContext) -> Result<()> {
        println!("committed block at height {}", block.height.as_u64());
        Ok(())
    }

    fn query(&self, path: &str, _data: &[u8]) -> Result<Vec<u8>> {
        match path {
            "info" => Ok(b"my-app v0.1".to_vec()),
            _ => Err(eg!("unknown query path")),
        }
    }
}
```

### Set Up Validators

```rust
use hotmint::prelude::*;
use hotmint::crypto::Ed25519Signer;

const NUM_VALIDATORS: u64 = 4;

// generate keypairs
let signers: Vec<Ed25519Signer> = (0..NUM_VALIDATORS)
    .map(|i| Ed25519Signer::generate(ValidatorId(i)))
    .collect();

// build the validator set from public keys
let validator_infos: Vec<ValidatorInfo> = signers
    .iter()
    .enumerate()
    .map(|(i, s)| ValidatorInfo {
        id: ValidatorId(i as u64),
        public_key: Signer::public_key(s),
        power: 1,
    })
    .collect();

let validator_set = ValidatorSet::new(validator_infos);
// quorum_threshold = ceil(2n/3), e.g. 3 out of 4
```

### Run an In-Process Multi-Node Cluster

Wire up all validators connected via in-memory channels — useful for testing and development:

```rust
use std::collections::HashMap;
use tokio::sync::mpsc;
use hotmint::consensus::engine::ConsensusEngine;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::MemoryBlockStore;
use hotmint::consensus::network::ChannelNetwork;

// create a message channel for each validator
let mut receivers = HashMap::new();
let mut all_senders = HashMap::new();
for i in 0..NUM_VALIDATORS {
    let (tx, rx) = mpsc::unbounded_channel();
    receivers.insert(ValidatorId(i), rx);
    all_senders.insert(ValidatorId(i), tx);
}

// spawn each validator
for i in 0..NUM_VALIDATORS {
    let vid = ValidatorId(i);
    let rx = receivers.remove(&vid).unwrap();
    let senders: Vec<_> = all_senders
        .iter()
        .map(|(&id, tx)| (id, tx.clone()))
        .collect();

    let engine = ConsensusEngine::new(
        ConsensusState::new(vid, validator_set.clone()),
        Box::new(MemoryBlockStore::new()),
        Box::new(ChannelNetwork::new(vid, senders)),
        Box::new(MyApp),
        Box::new(signers[i as usize].clone()),
        rx,
    );

    tokio::spawn(async move { engine.run().await });
}
```

### Use Persistent Storage

Replace the in-memory store with vsdb-backed storage for crash recovery:

```rust
use hotmint::storage::block_store::VsdbBlockStore;
use hotmint::storage::consensus_state::PersistentConsensusState;

// persistent block store (backed by vsdb)
let store = VsdbBlockStore::new();

// persistent consensus state (survives restarts)
let mut persistent_state = PersistentConsensusState::new();

// restore state after crash
let mut state = ConsensusState::new(vid, validator_set.clone());
if let Some(view) = persistent_state.load_current_view() {
    state.current_view = view;
}
if let Some(qc) = persistent_state.load_locked_qc() {
    state.locked_qc = Some(qc);
}
if let Some(qc) = persistent_state.load_highest_qc() {
    state.highest_qc = Some(qc);
}
if let Some(h) = persistent_state.load_last_committed_height() {
    state.last_committed_height = h;
}
if let Some(epoch) = persistent_state.load_current_epoch() {
    state.current_epoch = epoch;
}

let engine = ConsensusEngine::new(
    state,
    Box::new(store),
    Box::new(network_sink),  // ChannelNetwork or Litep2pNetworkSink
    Box::new(MyApp),
    Box::new(signer),
    msg_rx,
);
```

📖 **[Storage guide →](docs/storage.md)**

### Use Real P2P Networking

Replace in-memory channels with litep2p for multi-process / multi-machine deployments:

```rust
use hotmint::network::service::{NetworkService, PeerMap};

// build the peer map (ValidatorId <-> libp2p PeerId)
let mut peer_map = PeerMap::new();
peer_map.insert(ValidatorId(0), peer_id_0);
peer_map.insert(ValidatorId(1), peer_id_1);
// ...

let known_addresses = vec![
    (peer_id_0, vec!["/ip4/10.0.0.1/tcp/30000".parse().unwrap()]),
    (peer_id_1, vec!["/ip4/10.0.0.2/tcp/30000".parse().unwrap()]),
    // ...
];

let (net_service, network_sink, msg_rx) = NetworkService::create(
    "/ip4/0.0.0.0/tcp/30000".parse().unwrap(),
    peer_map,
    known_addresses,
).unwrap();

// run the network event loop in background
tokio::spawn(async move { net_service.run().await });

// pass network_sink and msg_rx to ConsensusEngine::new(...)
```

📖 **[Networking guide →](docs/networking.md)**

### Add Mempool and JSON-RPC API

Accept external transactions via JSON-RPC:

```rust
use std::sync::Arc;
use tokio::sync::watch;
use hotmint::mempool::Mempool;
use hotmint::api::rpc::{RpcServer, RpcState};

// shared mempool (10k txs max, 1MB per tx)
let mempool = Arc::new(Mempool::new(10_000, 1_048_576));

// status channel (updated by your commit handler)
// tuple: (current_view, last_committed_height, epoch)
let (status_tx, status_rx) = watch::channel((0u64, 0u64, 0u64));

let rpc_state = RpcState {
    validator_id: 0,
    mempool: mempool.clone(),
    status_rx,
};

let server = RpcServer::bind("127.0.0.1:26657", rpc_state).await.unwrap();
tokio::spawn(async move { server.run().await });
```

Submit transactions via JSON-RPC (newline-delimited JSON over TCP):

```bash
# query node status
echo '{"method":"status","params":{},"id":1}' | nc 127.0.0.1 26657

# submit a transaction (hex-encoded)
echo '{"method":"submit_tx","params":{"tx":"deadbeef"},"id":2}' | nc 127.0.0.1 26657
```

📖 **[Mempool & API guide →](docs/mempool-api.md)**

### Collect Prometheus Metrics

```rust
use prometheus_client::registry::Registry;
use hotmint::consensus::metrics::ConsensusMetrics;

let mut registry = Registry::default();
let metrics = ConsensusMetrics::new(&mut registry);

// metrics are automatically incremented by the consensus engine:
//   hotmint_blocks_committed, hotmint_blocks_proposed,
//   hotmint_votes_sent, hotmint_qcs_formed,
//   hotmint_double_certs_formed, hotmint_view_timeouts,
//   hotmint_tcs_formed, hotmint_current_view,
//   hotmint_current_height, hotmint_consecutive_timeouts,
//   hotmint_view_duration_seconds
```

📖 **[Metrics guide →](docs/metrics.md)**

## License

GPL-3.0
