# Mempool & JSON-RPC API

## Mempool

The `Mempool` is a thread-safe transaction pool that collects, deduplicates, and batches transactions for block inclusion.

### Construction

```rust
use hotmint::mempool::Mempool;

// custom limits: max 10,000 transactions, max 1MB per transaction
let mempool = Mempool::new(10_000, 1_048_576);

// default limits: 10,000 txs, 1MB
let mempool = Mempool::default();
```

### Adding Transactions

```rust
use std::sync::Arc;

let mempool = Arc::new(Mempool::default());

// add_tx returns true if accepted, false if rejected (duplicate or full)
let accepted = mempool.add_tx(b"transfer alice bob 100".to_vec()).await;
```

Transactions are deduplicated by their Blake3 hash. Duplicate transactions are silently rejected. If the pool is full (at `max_size`), new transactions are also rejected.

### Collecting Payload for Block Proposal

When the leader needs to propose a block, it calls `collect_payload` from the `Application::create_payload` method:

```rust
use hotmint::consensus::application::Application;

struct MyApp {
    mempool: Arc<Mempool>,
}

impl Application for MyApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        let rt = tokio::runtime::Handle::current();
        // collect up to 1MB of transactions
        rt.block_on(self.mempool.collect_payload(1_048_576))
    }

    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> ruc::Result<()> {
        Ok(())
    }
}
```

`collect_payload` drains transactions from the front of the queue (FIFO) until the byte limit is reached. Transactions are encoded in a length-prefixed format:

```
[u32_le: tx1_len][tx1_bytes][u32_le: tx2_len][tx2_bytes]...
```

### Decoding Payload

```rust
let txs: Vec<Vec<u8>> = Mempool::decode_payload(&block.payload);
for tx in &txs {
    // process each transaction
}
```

### Pool Status

```rust
let size = mempool.size().await;
println!("pending transactions: {}", size);
```

## JSON-RPC API

The `RpcServer` provides a TCP-based JSON-RPC interface for external clients to query node status and submit transactions.

### Setup

```rust
use std::sync::Arc;
use tokio::sync::watch;
use hotmint::mempool::Mempool;
use hotmint::api::rpc::{RpcServer, RpcState};

let mempool = Arc::new(Mempool::default());

// status channel: (current_view, last_committed_height, epoch, validator_count)
// update this from your Application::on_commit handler
let (status_tx, status_rx) = watch::channel((0u64, 0u64, 0u64, 4usize));

use std::sync::RwLock;
use hotmint::consensus::engine::SharedBlockStore;
use hotmint::consensus::store::MemoryBlockStore;

let store: SharedBlockStore =
    Arc::new(RwLock::new(Box::new(MemoryBlockStore::new())));
let (_peer_tx, peer_info_rx) = watch::channel(vec![]);

let rpc_state = RpcState {
    validator_id: 0,
    mempool: mempool.clone(),
    status_rx,
    store,
    peer_info_rx,
};

let server = RpcServer::bind("127.0.0.1:26657", rpc_state).await.unwrap();
let addr = server.local_addr(); // actual bound address (useful if port was 0)
tokio::spawn(async move { server.run().await });
```

### Updating Status

Wire the status channel into your application's commit handler:

```rust
struct MyApp {
    mempool: Arc<Mempool>,
    status_tx: watch::Sender<(u64, u64, u64, usize)>,
}

impl Application for MyApp {
    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> ruc::Result<()> {
        let _ = self.status_tx.send((
            block.view.as_u64(),
            block.height.as_u64(),
            ctx.epoch.as_u64(),
            ctx.validator_set.validator_count(),
        ));
        Ok(())
    }

    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.mempool.collect_payload(1_048_576))
    }
}
```

### Protocol

The RPC server uses newline-delimited JSON over TCP. Each request is a single JSON object terminated by `\n`, and each response is a single JSON object terminated by `\n`.

### Request Format

```json
{
    "method": "method_name",
    "params": { ... },
    "id": 1
}
```

### Response Format

Success:
```json
{
    "result": { ... },
    "error": null,
    "id": 1
}
```

Error:
```json
{
    "result": null,
    "error": { "code": -32601, "message": "method not found" },
    "id": 1
}
```

### Methods

#### `status`

Returns the current node status.

Request:
```bash
echo '{"method":"status","params":{},"id":1}' | nc 127.0.0.1 26657
```

Response:
```json
{
    "result": {
        "validator_id": 0,
        "current_view": 42,
        "last_committed_height": 15,
        "mempool_size": 3
    },
    "error": null,
    "id": 1
}
```

#### `submit_tx`

Submit a transaction (hex-encoded bytes).

Request:
```bash
echo '{"method":"submit_tx","params":{"tx":"48656c6c6f"},"id":2}' | nc 127.0.0.1 26657
```

Response:
```json
{
    "result": { "accepted": true },
    "error": null,
    "id": 2
}
```

The transaction is hex-decoded and added to the mempool. `accepted: false` means the transaction was rejected (duplicate, pool full, or failed `Application::validate_tx`).

### Types

```rust
pub struct RpcRequest {
    pub method: String,
    pub params: serde_json::Value,
    pub id: u64,
}

pub struct RpcResponse {
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcError>,
    pub id: u64,
}

pub struct RpcError {
    pub code: i32,
    pub message: String,
}

pub struct StatusInfo {
    pub validator_id: u64,
    pub current_view: u64,
    pub last_committed_height: u64,
    pub mempool_size: usize,
}

pub struct TxResult {
    pub accepted: bool,
}
```

## Full Example: Node with Mempool and RPC

```rust
use std::sync::Arc;
use ruc::*;
use tokio::sync::watch;
use hotmint::prelude::*;
use hotmint::consensus::application::Application;
use hotmint::consensus::engine::ConsensusEngine;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::MemoryBlockStore;
use hotmint::consensus::network::ChannelNetwork;
use hotmint::crypto::Ed25519Signer;
use hotmint::mempool::Mempool;
use hotmint::api::rpc::{RpcServer, RpcState};

struct TxCounterApp {
    mempool: Arc<Mempool>,
    status_tx: watch::Sender<(u64, u64, u64, usize)>,
}

impl Application for TxCounterApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.mempool.collect_payload(1_048_576))
    }

    fn validate_tx(&self, tx: &[u8]) -> bool {
        !tx.is_empty() && tx.len() <= 4096
    }

    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()> {
        let txs = Mempool::decode_payload(&block.payload);
        let _ = self.status_tx.send((
            block.view.as_u64(),
            block.height.as_u64(),
            ctx.epoch.as_u64(),
            ctx.validator_set.validator_count(),
        ));
        println!(
            "height={} txs={} view={}",
            block.height.as_u64(),
            txs.len(),
            block.view.as_u64(),
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let mempool = Arc::new(Mempool::default());
    let (status_tx, status_rx) = watch::channel((0u64, 0u64, 0u64, 4usize));

    use std::sync::RwLock;
    use hotmint::consensus::engine::SharedBlockStore;
    use hotmint::consensus::store::MemoryBlockStore;

    let store: SharedBlockStore =
        Arc::new(RwLock::new(Box::new(MemoryBlockStore::new())));
    let (_peer_tx, peer_info_rx) = watch::channel(vec![]);

    // start RPC server
    let rpc_state = RpcState {
        validator_id: 0,
        mempool: mempool.clone(),
        status_rx,
        store,
        peer_info_rx,
    };
    let server = RpcServer::bind("127.0.0.1:26657", rpc_state).await.unwrap();
    println!("RPC listening on {}", server.local_addr());
    tokio::spawn(async move { server.run().await });

    let app = TxCounterApp {
        mempool: mempool.clone(),
        status_tx,
    };

    // ... set up validators and consensus engine as shown in getting-started.md
    // ... pass `app` to ConsensusEngine::new()
}
```
