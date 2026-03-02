# Getting Started

## Prerequisites

- **Rust** 2024 edition (nightly or stable with edition support)

## Installation

Add `hotmint` as a dependency:

```toml
[dependencies]
hotmint = { git = "https://github.com/rust-util-collections/hotmint" }
tokio = { version = "1", features = ["full"] }
ruc = "9.3"
```

## Quick Start

```bash
# clone the repository
git clone https://github.com/rust-util-collections/hotmint.git
cd hotmint

# build all crates
cargo build --workspace

# run all tests
cargo test --workspace

# run the 4-node in-process demo
cargo run --bin hotmint-demo

# or initialize and run a production node (connects to ABCI app via Unix socket)
cargo run --bin hotmint-node -- init
cargo run --bin hotmint-node -- node
```

## Minimal Integration

The simplest way to use hotmint is to implement the `Application` trait and wire it into an in-memory cluster. All methods have default no-op implementations, so you only need to implement the ones your application cares about.

### Step 1: Define Your Application

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

### Step 2: Set Up Validators

```rust
use hotmint::crypto::Ed25519Signer;

const N: u64 = 4;

let signers: Vec<Ed25519Signer> = (0..N)
    .map(|i| Ed25519Signer::generate(ValidatorId(i)))
    .collect();

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
```

### Step 3: Create Channels and Spawn Engines

```rust
use std::collections::HashMap;
use tokio::sync::mpsc;
use hotmint::consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::MemoryBlockStore;
use hotmint::consensus::network::ChannelNetwork;
use hotmint::crypto::Ed25519Verifier;

let mut receivers = HashMap::new();
let mut all_senders = HashMap::new();
for i in 0..N {
    let (tx, rx) = mpsc::channel(8192);
    receivers.insert(ValidatorId(i), rx);
    all_senders.insert(ValidatorId(i), tx);
}

for i in 0..N {
    let vid = ValidatorId(i);
    let rx = receivers.remove(&vid).unwrap();
    let senders: Vec<_> = all_senders
        .iter()
        .map(|(&id, tx)| (id, tx.clone()))
        .collect();

    let store: hotmint::consensus::engine::SharedBlockStore =
        std::sync::Arc::new(std::sync::RwLock::new(Box::new(MemoryBlockStore::new())));

    let engine = ConsensusEngine::new(
        ConsensusState::new(vid, validator_set.clone()),
        store,
        Box::new(ChannelNetwork::new(vid, senders)),
        Box::new(MyApp),
        Box::new(signers[i as usize].clone()),
        rx,
        EngineConfig {
            verifier: Box::new(Ed25519Verifier),
            pacemaker: None,
            persistence: None,
        },
    );

    tokio::spawn(async move { engine.run().await });
}
```

That's it — the cluster is now running consensus. Blocks will be proposed, voted on, and committed via your `on_commit` handler.

## CLI Flags

The `hotmint-node` binary accepts the following flags:

| Flag | Description | Default |
|:-----|:------------|:--------|
| `--home <PATH>` | Set the home directory for config and data | `~/.hotmint` |
| `node --proxy-app <ADDR>` | Unix socket address of the ABCI application | `unix:///tmp/hotmint.sock` |
| `node --p2p-laddr <ADDR>` | P2P listen address (multiaddr format) | `/ip4/0.0.0.0/tcp/26656` |
| `node --rpc-laddr <ADDR>` | JSON-RPC listen address | `127.0.0.1:26657` |

Examples:

```bash
# initialize with a custom home directory
cargo run --bin hotmint-node -- --home /data/mynode init

# run a node with custom addresses
cargo run --bin hotmint-node -- --home /data/mynode node \
    --proxy-app unix:///tmp/myapp.sock \
    --p2p-laddr /ip4/0.0.0.0/tcp/26656 \
    --rpc-laddr 0.0.0.0:26657
```

## Configuration File

The `init` command creates a `config.toml` in the home directory. The full structure:

```toml
[node]
# Validator private key (hex-encoded Ed25519 seed)
validator_key = "..."
# Logging level: "debug", "info", "warn", "error"
log_level = "info"

[rpc]
# JSON-RPC listen address
laddr = "127.0.0.1:26657"

[p2p]
# P2P listen address (multiaddr format)
laddr = "/ip4/0.0.0.0/tcp/26656"
# List of persistent peer addresses
persistent_peers = []
# Optional Ed25519 keypair seed for deterministic PeerId (hex)
# node_key = "..."

[pex]
# Enable peer exchange protocol
enabled = true
# Interval between PEX requests (seconds)
interval_secs = 30

[consensus]
# Base pacemaker timeout (milliseconds)
base_timeout_ms = 2000
# Timeout backoff multiplier
backoff_multiplier = 1.5
# Maximum timeout (milliseconds)
max_timeout_ms = 30000

[mempool]
# Maximum number of pending transactions
max_size = 10000
# Maximum transaction size in bytes
max_tx_bytes = 1048576
```

## Next Steps

- [Application](application.md) — full lifecycle: `execute_block`, `on_commit`, `query`
- [Storage](storage.md) — swap in persistent vsdb storage for production
- [Networking](networking.md) — replace channels with litep2p for multi-process deployments
- [Mempool & API](mempool-api.md) — accept external transactions via JSON-RPC
