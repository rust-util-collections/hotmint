# Hotmint

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_edition-orange.svg)](https://www.rust-lang.org/)
[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)]()
[![HotStuff-2](https://img.shields.io/badge/protocol-HotStuff--2-purple.svg)](https://arxiv.org/abs/2301.03253)

> A Rust BFT consensus framework combining Tendermint's engineering ergonomics with HotStuff-2's protocol efficiency.

## Design Goals

Hotmint is a BFT consensus framework built from scratch. It retains the clean, modular architecture of Tendermint while adopting HotStuff-2's two-chain commit protocol for lower confirmation latency and simpler view-change mechanics.

**Core design philosophy:**

- **Protocol**: HotStuff-2 two-chain commit + simplified view change, replacing Tendermint's three-phase voting
- **Architecture**: Tendermint-inspired modular design (consensus / network / application separation) with clean trait boundaries
- **Storage**: [vsdb](https://crates.io/crates/vsdb) as the persistence backend, with Merkle proof support (MPT/SMT)
- **Networking**: [litep2p](https://crates.io/crates/litep2p) as the P2P foundation, lighter weight than libp2p
- **Error handling**: [ruc](https://crates.io/crates/ruc) chained error tracing

## Protocol Overview

Hotmint implements the core protocol from the HotStuff-2 paper ([arXiv:2301.03253](https://arxiv.org/abs/2301.03253)).

### Two-Chain Commit Rule

HotStuff-2's key innovation is reducing confirmation from three chains to two:

```
B_{k-1}  <--  C_v(B_{k-1})  <--  C_v(C_v(B_{k-1}))
  Block         QC (Quorum Cert)    Double Cert -> triggers commit
```

When a block receives a QC (aggregate signature from 2f+1 validators), and then that QC itself receives 2f+1 votes forming a "Double Certificate", the block and all its uncommitted ancestors are safely committed.

### Steady-State View Protocol (Paper Figure 1)

Each view v consists of 5 steps:

| Step | Name | Role | Description |
|:-----|:-----|:-----|:------------|
| 1 | Enter | All | Enter view: Leader waits for status or proposes directly; Replica sends status |
| 2 | Propose | Leader | Broadcast proposal `<propose, B_k, v, C_{v'}(B_{k-1})>` with justify QC |
| 3 | Vote & Commit | Replica | Safety check (justify.rank >= locked_qc.rank), vote if passed, check for commits |
| 4 | Prepare | Leader | Collect 2f+1 votes to form QC, broadcast `<prepare, C_v(B_k)>` |
| 5 | Vote2 | Replica | Update lock to C_v(B_k), send vote2 to next view's Leader |

### Safety Rules

- **Locking rule**: Replica updates its lock upon receiving Prepare (QC); voting requires justify.rank >= locked_qc.rank
- **Commit rule**: Double certificate `C_v(C_v(B_k))` triggers commit, committing all uncommitted ancestors in height order

### View Change / Pacemaker (Paper Figure 2)

Simplified Pacemaker (Phase 1):

```
enter_view -> start view_timer(base_timeout=2s)
timeout    -> broadcast Wish{target_view: current+1, highest_qc}
2f+1 wish  -> form TC (Timeout Certificate), broadcast and advance view
receive TC or DoubleCert -> advance to corresponding view
```

Full epoch structure and Cogsworth coordination will be implemented in later phases.

## Architecture

### Workspace Layout

```
hotmint/
├── Cargo.toml                     # workspace root
├── crates/
│   ├── hotmint-types/             # core data types (minimal dependencies)
│   │   └── src/
│   │       ├── block.rs           # Block, BlockHash, Height
│   │       ├── vote.rs            # Vote, VoteType
│   │       ├── certificate.rs     # QuorumCertificate, DoubleCertificate, TimeoutCertificate
│   │       ├── view.rs            # ViewNumber
│   │       ├── message.rs         # ConsensusMessage — wire protocol
│   │       ├── validator.rs       # ValidatorId, ValidatorSet, ValidatorInfo
│   │       └── crypto.rs          # Signature, PublicKey, AggregateSignature, Signer/Verifier traits
│   │
│   ├── hotmint-crypto/            # concrete cryptography implementations
│   │   └── src/
│   │       ├── signer.rs          # Ed25519Signer (sign/verify)
│   │       ├── aggregate.rs       # simple aggregate signatures (bitfield + signature list)
│   │       └── hash.rs            # Blake3 block hashing
│   │
│   ├── hotmint-consensus/         # consensus state machine
│   │   └── src/
│   │       ├── engine.rs          # ConsensusEngine — tokio::select! event loop
│   │       ├── state.rs           # ConsensusState — mutable consensus state
│   │       ├── view_protocol.rs   # Paper Figure 1: steady-state view protocol
│   │       ├── pacemaker.rs       # Paper Figure 2: timeout / view change with backoff
│   │       ├── leader.rs          # round-robin leader election (v mod n)
│   │       ├── commit.rs          # two-chain commit rule
│   │       ├── vote_collector.rs  # vote collection and QC formation
│   │       ├── metrics.rs         # Prometheus metrics
│   │       ├── store.rs           # BlockStore trait + in-memory stub
│   │       ├── network.rs         # NetworkSink trait + channel stub
│   │       ├── application.rs     # ABCI-like Application trait
│   │       └── error.rs           # ConsensusError
│   │
│   ├── hotmint-storage/           # persistent storage (vsdb/rocksdb)
│   │   └── src/
│   │       ├── block_store.rs     # VsdbBlockStore
│   │       └── consensus_state.rs # PersistentConsensusState
│   │
│   ├── hotmint-network/           # P2P networking (litep2p)
│   │   └── src/
│   │       └── service.rs         # NetworkService, Litep2pNetworkSink, PeerMap
│   │
│   ├── hotmint-mempool/           # transaction mempool
│   │   └── src/lib.rs             # Mempool (FIFO, dedup, payload encoding)
│   │
│   ├── hotmint-api/               # JSON-RPC API
│   │   └── src/
│   │       ├── rpc.rs             # RpcServer over TCP
│   │       └── types.rs           # RpcRequest, RpcResponse, StatusInfo
│   │
│   └── hotmint/                   # top-level library crate (facade)
│       └── src/
│           ├── lib.rs             # re-exports all sub-crates + prelude
│           └── bin/
│               └── node.rs        # [[bin]] demo: 4 in-process validators
```

### Dependency Graph

```
hotmint (library facade)
  ├── hotmint-consensus -> hotmint-types
  ├── hotmint-crypto    -> hotmint-types
  ├── hotmint-storage   -> hotmint-consensus, vsdb
  ├── hotmint-network   -> hotmint-consensus, litep2p
  ├── hotmint-mempool
  └── hotmint-api       -> hotmint-mempool
```

The consensus engine communicates with the network layer via `tokio::mpsc` channels and has no direct dependency on any networking crate.

### Core Trait Abstractions

```rust
// Cryptographic signing
trait Signer: Send + Sync {
    fn sign(&self, message: &[u8]) -> Signature;
    fn public_key(&self) -> PublicKey;
    fn validator_id(&self) -> ValidatorId;
}

// Block persistence (returns owned values for vsdb compatibility)
trait BlockStore: Send + Sync {
    fn put_block(&mut self, block: Block);
    fn get_block(&self, hash: &BlockHash) -> Option<Block>;
    fn get_block_by_height(&self, h: Height) -> Option<Block>;
}

// Network transport
trait NetworkSink: Send + Sync {
    fn broadcast(&self, msg: ConsensusMessage);
    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage);
}

// Application layer (ABCI-like lifecycle)
trait Application: Send + Sync {
    fn create_payload(&self) -> Vec<u8>;
    fn validate_block(&self, block: &Block) -> bool;
    fn validate_tx(&self, tx: &[u8]) -> bool;
    fn begin_block(&self, height: Height, view: ViewNumber) -> Result<()>;
    fn deliver_tx(&self, tx: &[u8]) -> Result<()>;
    fn end_block(&self, height: Height) -> Result<()>;
    fn on_commit(&self, block: &Block) -> Result<()>;
    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>>;
}
```

Each trait provides default implementations and has a stub (in-memory store, channel network, no-op app) for development use.

## Core Types

| Type | Paper Notation | Description |
|:-----|:---------------|:------------|
| `ViewNumber(u64)` | v | Monotonically increasing view number |
| `Height(u64)` | k | Block height in the committed chain |
| `BlockHash([u8; 32])` | h_k | 32-byte Blake3 hash |
| `Block` | B_k := (b_k, h_{k-1}) | Block: height + parent hash + view + proposer + payload |
| `QuorumCertificate` | C_v(B_k) | Aggregate signature of 2f+1 validators on the block hash |
| `DoubleCertificate` | C_v(C_v(B_k)) | QC of a QC, triggers commit |
| `TimeoutCertificate` | TC_v | Timeout proof from 2f+1 validators, carrying their highest_qc |
| `Vote` | — | Vote: block hash + view + validator + signature + type (Vote/Vote2) |
| `ValidatorSet` | — | Validator set with quorum threshold calculation and leader selection |

### ConsensusMessage (Wire Protocol)

```rust
enum ConsensusMessage {
    Propose { block, justify: QC, double_cert: Option<DC>, signature },
    VoteMsg(Vote),           // phase-1 vote -> current Leader
    Prepare { certificate: QC, signature },
    Vote2Msg(Vote),          // phase-2 vote -> next Leader
    Wish { target_view, validator, highest_qc, signature },
    TimeoutCert(TC),
    StatusCert { locked_qc, validator, signature },
}
```

## Consensus Engine

### State Machine

```rust
struct ConsensusState {
    validator_id: ValidatorId,
    validator_set: ValidatorSet,
    current_view: ViewNumber,
    role: ViewRole,              // Leader / Replica
    step: ViewStep,              // Entered -> Proposed/Voted -> Prepared -> SentVote2 -> Done
    locked_qc: Option<QC>,      // highest locked QC
    highest_double_cert: Option<DoubleCert>,
    highest_qc: Option<QC>,
    last_committed_height: Height,
}
```

### Event Loop

```rust
loop {
    tokio::select! {
        Some((sender, msg)) = msg_rx.recv() => handle_message(sender, msg),
        _ = pacemaker.view_timer => handle_timeout(),
    }
}
```

Message dispatch:
- `Propose` -> `view_protocol::on_proposal()` -> safety check + vote
- `VoteMsg` -> `vote_collector::add_vote()` -> on quorum: `on_qc_formed()`
- `Prepare` -> `view_protocol::on_prepare()` -> update lock + send vote2
- `Vote2Msg` -> `vote_collector::add_vote()` -> on quorum: `on_double_cert_formed()` -> commit
- `Wish` -> `pacemaker::add_wish()` -> on quorum: form TC -> advance view
- `TimeoutCert` -> advance view
- `StatusCert` -> Leader collects status then proposes

### ValidatorSet and Quorum

- n = total validators, f = floor((n-1)/3) max Byzantine
- Quorum threshold: ceil(2n/3) (for n=4, quorum=3, f=1)
- Leader selection: `view.as_u64() % n` round-robin

## Technology Choices

| Component | Current (Phase 1) | Production Target |
|:----------|:-------------------|:------------------|
| Signatures | Ed25519 (ed25519-dalek) | BLS/FROST aggregate signatures |
| Hashing | Blake3 | Blake3 |
| Aggregate Signatures | Bitfield + signature list | BLS threshold signatures |
| Storage | HashMap + BTreeMap (in-memory) | vsdb MapxOrd + VerMapWithProof |
| Networking | tokio::mpsc channels (in-process) | litep2p notification + request-response |
| Async Runtime | Tokio | Tokio |
| Error Handling | ruc 9.3 | ruc |
| Serialization | serde + msgpack (rmp-serde) | serde + msgpack |
| Logging | tracing + tracing-subscriber | tracing + prometheus-client |

## Roadmap

### Phase 1: Project Skeleton + Core Types + Consensus State Machine

- [x] Cargo workspace structure
- [x] Core data types (hotmint-types)
- [x] Ed25519 signing + Blake3 hashing (hotmint-crypto)
- [x] Consensus state machine (hotmint-consensus)
- [x] In-process 4-node test run

### Phase 2: Persistent Storage

- [x] VsdbBlockStore backed by rocksdb (via vsdb MapxOrd)
- [x] PersistentConsensusState for crash recovery
- [x] BlockStore trait refactored to return owned values (vsdb compatible)

### Phase 3: Real Networking

- [x] litep2p notification protocol (broadcast messages)
- [x] litep2p request-response protocol (directed messages)
- [x] Message serialization via msgpack (rmp-serde)
- [x] PeerMap for ValidatorId <-> PeerId routing

### Phase 4: Full Pacemaker

- [x] Exponential backoff timeout (1.5x per timeout, cap 30s)
- [x] TC relay (rebroadcast timeout certificates with dedup)
- [x] Reset-on-progress for successful view transitions
- [x] Epoch and EpochNumber types for validator set change boundaries

### Phase 5: Production Hardening

- [x] Prometheus metrics (blocks, votes, QCs, timeouts, view duration histogram)
- [x] Byzantine tolerance test (1-fault with 3/4 validators)
- [x] 27 unit + integration tests across all crates

### Phase 6: Application Framework

- [x] ABCI-like Application trait (begin_block, deliver_tx, end_block, on_commit, query)
- [x] Mempool module (FIFO with dedup, size/byte limits, payload encoding)
- [x] JSON-RPC API (status, submit_tx endpoints over TCP)

## References

| Paper | Link | Key Contribution |
|:------|:-----|:-----------------|
| HotStuff-2: Optimal Two-Chain BFT (2023) | [arXiv:2301.03253](https://arxiv.org/abs/2301.03253) | Two-chain commit, simplified view change |
| HotStuff: BFT Consensus (PODC 2019) | [arXiv:1803.05069](https://arxiv.org/abs/1803.05069) | Linear communication, pipelining |
| Tendermint: Latest Gossip on BFT (2018) | [arXiv:1807.04938](https://arxiv.org/abs/1807.04938) | Production BFT, ABCI architecture |

## Usage

Add `hotmint` as a dependency in your `Cargo.toml`:

```toml
[dependencies]
hotmint = { git = "https://github.com/user/hotmint" }
```

Implement the `Application` trait and wire up your consensus node:

```rust
use hotmint::prelude::*;
use hotmint::consensus::application::Application;

struct MyApp;

impl Application for MyApp {
    fn on_commit(&self, block: &Block) -> ruc::Result<()> {
        // process committed block
        Ok(())
    }
}
```

## Quick Start

```bash
# build
cargo build --workspace

# run tests
cargo test --workspace

# run the 4-node in-process demo
cargo run --bin hotmint-node
```

## License

GPL-3.0
