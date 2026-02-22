# hotmint

[![crates.io](https://img.shields.io/crates/v/hotmint.svg)](https://crates.io/crates/hotmint)
[![docs.rs](https://docs.rs/hotmint/badge.svg)](https://docs.rs/hotmint)

A Rust BFT consensus framework combining Tendermint's engineering ergonomics with HotStuff-2's protocol efficiency.

This is the top-level facade crate that re-exports the entire Hotmint ecosystem. Add this single dependency to access all sub-crates.

## Sub-Crates

| Re-export | Crate | Description |
|:----------|:------|:------------|
| `hotmint::types` | [hotmint-types](https://crates.io/crates/hotmint-types) | Core data types |
| `hotmint::crypto` | [hotmint-crypto](https://crates.io/crates/hotmint-crypto) | Ed25519 + Blake3 |
| `hotmint::consensus` | [hotmint-consensus](https://crates.io/crates/hotmint-consensus) | Consensus engine |
| `hotmint::storage` | [hotmint-storage](https://crates.io/crates/hotmint-storage) | Persistent storage (vsdb) |
| `hotmint::network` | [hotmint-network](https://crates.io/crates/hotmint-network) | P2P networking (litep2p) |
| `hotmint::mempool` | [hotmint-mempool](https://crates.io/crates/hotmint-mempool) | Transaction mempool |
| `hotmint::api` | [hotmint-api](https://crates.io/crates/hotmint-api) | JSON-RPC API |

## Prelude

```rust
use hotmint::prelude::*;
// Imports: Block, BlockHash, Height, ViewNumber, Vote, VoteType,
//          QuorumCertificate, DoubleCertificate, TimeoutCertificate,
//          ValidatorId, ValidatorInfo, ValidatorSet,
//          Signer, Verifier, Signature, Epoch, EpochNumber,
//          ConsensusMessage
```

## Quick Start

```toml
[dependencies]
hotmint = { git = "https://github.com/rust-util-collections/hotmint" }
tokio = { version = "1", features = ["full"] }
ruc = "9.3"
```

```rust
use ruc::*;
use hotmint::prelude::*;
use hotmint::consensus::application::Application;
use hotmint::consensus::engine::ConsensusEngine;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::MemoryBlockStore;
use hotmint::consensus::network::ChannelNetwork;
use hotmint::crypto::Ed25519Signer;

struct MyApp;

impl Application for MyApp {
    fn on_commit(&self, block: &Block) -> Result<()> {
        println!("committed height {}", block.height.as_u64());
        Ok(())
    }
}

// 1. Generate signers and build validator set
let signers: Vec<Ed25519Signer> = (0..4)
    .map(|i| Ed25519Signer::generate(ValidatorId(i)))
    .collect();

let vs = ValidatorSet::new(
    signers.iter().enumerate().map(|(i, s)| ValidatorInfo {
        id: ValidatorId(i as u64),
        public_key: Signer::public_key(s),
        power: 1,
    }).collect()
);

// 2. Create channels and spawn engines
// (see docs/getting-started.md for the full wiring example)
```

## Demo Binary

```bash
# run the built-in 4-node in-process demo
cargo run --bin hotmint-node
```

## Documentation

See the [docs/](https://github.com/rust-util-collections/hotmint/tree/main/docs) directory for comprehensive guides:

- [Getting Started](https://github.com/rust-util-collections/hotmint/blob/main/docs/getting-started.md)
- [Protocol](https://github.com/rust-util-collections/hotmint/blob/main/docs/protocol.md)
- [Architecture](https://github.com/rust-util-collections/hotmint/blob/main/docs/architecture.md)
- [Application](https://github.com/rust-util-collections/hotmint/blob/main/docs/application.md)

## License

GPL-3.0-only
