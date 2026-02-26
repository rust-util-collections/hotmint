# Architecture

Hotmint follows a modular, layered architecture inspired by Tendermint. Each concern — consensus logic, cryptography, networking, storage, and application — lives in its own crate with clear trait boundaries.

## Workspace Layout

```
hotmint/
├── Cargo.toml                     # workspace root
├── crates/
│   ├── hotmint-types/             # core data types (minimal dependencies)
│   ├── hotmint-crypto/            # cryptography implementations
│   ├── hotmint-consensus/         # consensus state machine
│   ├── hotmint-storage/           # persistent storage (vsdb)
│   ├── hotmint-network/           # P2P networking (litep2p)
│   ├── hotmint-mempool/           # transaction mempool
│   ├── hotmint-api/               # JSON-RPC API
│   └── hotmint/                   # top-level library facade
└── docs/
```

## Dependency Graph

```
hotmint (library facade — re-exports everything)
  ├── hotmint-consensus ──> hotmint-types
  ├── hotmint-crypto    ──> hotmint-types
  ├── hotmint-storage   ──> hotmint-consensus, vsdb
  ├── hotmint-network   ──> hotmint-consensus, litep2p
  ├── hotmint-mempool   (standalone)
  └── hotmint-api       ──> hotmint-mempool
```

Key design rule: **the consensus engine has no dependency on any concrete networking or storage crate**. It communicates with the outside world exclusively through trait objects (`Box<dyn BlockStore>`, `Box<dyn NetworkSink>`, `Box<dyn Application>`, `Box<dyn Signer>`), connected via `tokio::mpsc` channels.

## Crate Responsibilities

### hotmint-types

The foundational crate with minimal dependencies. Defines all data types shared across the system:

- `Block`, `BlockHash`, `Height` — chain primitives
- `ViewNumber` — consensus view tracking
- `Vote`, `VoteType` — voting messages
- `QuorumCertificate`, `DoubleCertificate`, `TimeoutCertificate` — aggregate proofs
- `ConsensusMessage` — the wire protocol enum
- `ValidatorId`, `ValidatorInfo`, `ValidatorSet` — validator management
- `Signature`, `PublicKey`, `AggregateSignature` — cryptographic primitives
- `Signer`, `Verifier` — abstract cryptographic traits
- `Epoch`, `EpochNumber` — epoch management

### hotmint-crypto

Concrete cryptographic implementations:

- `Ed25519Signer` — implements the `Signer` trait using ed25519-dalek
- `Ed25519Verifier` — implements the `Verifier` trait
- `hash_block()` — Blake3 block hashing

### hotmint-consensus

The core consensus state machine, entirely independent of I/O:

- `ConsensusEngine` — the main event loop (`tokio::select!`)
- `ConsensusState` — mutable consensus state (current view, locks, role)
- `view_protocol` — steady-state view protocol (Paper Figure 1)
- `pacemaker` — timeout and view change (Paper Figure 2)
- `vote_collector` — vote aggregation and QC formation
- `commit` — two-chain commit rule
- `leader` — round-robin leader election
- `metrics` — Prometheus metrics collection

Also defines the pluggable trait interfaces:
- `BlockStore` — block persistence
- `NetworkSink` — message transport
- `Application` — ABCI-like application lifecycle

Each trait includes an in-memory/no-op stub implementation for development use.

### hotmint-storage

Production-grade persistent storage backed by vsdb:

- `VsdbBlockStore` — implements `BlockStore` with `MapxOrd` for by-hash and by-height indexing
- `PersistentConsensusState` — persists critical consensus state (view, locks, committed height) for crash recovery

### hotmint-network

Real P2P networking using litep2p:

- `NetworkService` — manages litep2p connections, protocol handlers, and event routing
- `Litep2pNetworkSink` — implements `NetworkSink` for production use
- `PeerMap` — bidirectional `ValidatorId ↔ PeerId` mapping

Uses two sub-protocols:
- `/hotmint/consensus/notif/1` — notification protocol for broadcast
- `/hotmint/consensus/reqresp/1` — request-response protocol for directed messages

### hotmint-mempool

Transaction pool with FIFO ordering:

- Deduplication via Blake3 transaction hashing
- Configurable size limits (transaction count and byte size)
- Length-prefixed payload encoding for block inclusion

### hotmint-api

JSON-RPC server for external interaction:

- `RpcServer` — TCP-based JSON-RPC server (newline-delimited)
- Methods: `status` (node info), `submit_tx` (transaction submission)

## Core Trait Abstractions

The four pluggable traits define the boundary between the consensus engine and the outside world:

```rust
// Cryptographic signing — swap implementations without touching consensus
trait Signer: Send + Sync {
    fn sign(&self, message: &[u8]) -> Signature;
    fn public_key(&self) -> PublicKey;
    fn validator_id(&self) -> ValidatorId;
}

// Block persistence — in-memory for tests, vsdb for production
trait BlockStore: Send + Sync {
    fn put_block(&mut self, block: Block);
    fn get_block(&self, hash: &BlockHash) -> Option<Block>;
    fn get_block_by_height(&self, h: Height) -> Option<Block>;
}

// Network transport — channels for testing, litep2p for production
trait NetworkSink: Send + Sync {
    fn broadcast(&self, msg: ConsensusMessage);
    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage);
}

// Application lifecycle — your business logic
// All methods have default no-op implementations.
trait Application: Send + Sync {
    fn create_payload(&self, ctx: &BlockContext) -> Vec<u8>;
    fn validate_block(&self, block: &Block, ctx: &BlockContext) -> bool;
    fn validate_tx(&self, tx: &[u8], ctx: Option<&TxContext>) -> bool;
    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse>;
    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()>;
    fn on_evidence(&self, proof: &EquivocationProof) -> Result<()>;
    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>>;
}
```

## Message Flow

```
Client ──tx──> Mempool ──payload──> Application.create_payload()
                                        │
Leader: Propose ───broadcast───> Replicas
                                     │
Replicas: VoteMsg ───send_to───> Leader
                                     │
Leader: Prepare (QC) ──broadcast──> Replicas
                                     │
Replicas: Vote2Msg ──send_to──> Next Leader
                                     │
Next Leader: DoubleCert formed ──> commit chain
                                     │
                              Application.on_commit()
```

## Design Decisions

### Why trait objects instead of generics?

Trait objects (`Box<dyn T>`) are used for `BlockStore`, `NetworkSink`, `Application`, and `Signer` rather than generic type parameters. This choice:

- Keeps the `ConsensusEngine` type signature simple
- Allows runtime composition (e.g., switching between in-memory and persistent storage based on config)
- Avoids monomorphization bloat for what are typically single-instance objects

### Why tokio::mpsc for engine ↔ network communication?

The consensus engine receives messages through a `tokio::mpsc::UnboundedReceiver` rather than directly calling network APIs. This decouples the consensus logic from network implementation details and makes the engine trivially testable with in-memory channels.

### Why owned values in BlockStore?

`BlockStore` returns `Option<Block>` (owned) rather than references. This is a deliberate choice for vsdb compatibility — vsdb stores data on disk and cannot return references to in-memory data. The owned-value pattern works uniformly across both in-memory and persistent implementations.
