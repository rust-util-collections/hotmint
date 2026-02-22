# hotmint-api

[![crates.io](https://img.shields.io/crates/v/hotmint-api.svg)](https://crates.io/crates/hotmint-api)
[![docs.rs](https://docs.rs/hotmint-api/badge.svg)](https://docs.rs/hotmint-api)

JSON-RPC API server for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Provides a TCP-based, newline-delimited JSON-RPC server for querying node status and submitting transactions to the mempool.

## RPC Methods

| Method | Description | Response |
|:-------|:------------|:---------|
| `status` | Query node status (view, height, mempool size) | `StatusInfo` |
| `submit_tx` | Submit a hex-encoded transaction | `TxResult { accepted: bool }` |

## Usage

### Start the Server

```rust
use std::sync::Arc;
use tokio::sync::watch;
use hotmint_mempool::Mempool;
use hotmint_api::rpc::{RpcServer, RpcState};

let mempool = Arc::new(Mempool::default());
let (status_tx, status_rx) = watch::channel((0u64, 0u64));

let rpc_state = RpcState {
    validator_id: 0,
    mempool: mempool.clone(),
    status_rx,
};

let server = RpcServer::bind("127.0.0.1:26657", rpc_state).await.unwrap();
println!("RPC listening on {}", server.local_addr());
tokio::spawn(async move { server.run().await });
```

### Update Status from Application

```rust
use hotmint_consensus::application::Application;

struct MyApp {
    status_tx: watch::Sender<(u64, u64)>,
}

impl Application for MyApp {
    fn on_commit(&self, block: &hotmint_types::Block) -> ruc::Result<()> {
        let _ = self.status_tx.send((
            block.view.as_u64(),
            block.height.as_u64(),
        ));
        Ok(())
    }
}
```

### Client Examples

```bash
# query status
echo '{"method":"status","params":{},"id":1}' | nc 127.0.0.1 26657
# => {"result":{"validator_id":0,"current_view":42,"last_committed_height":15,"mempool_size":3},...}

# submit transaction (hex-encoded)
echo '{"method":"submit_tx","params":{"tx":"deadbeef"},"id":2}' | nc 127.0.0.1 26657
# => {"result":{"accepted":true},...}
```

## Types

```rust
pub struct RpcRequest  { pub method: String, pub params: Value, pub id: u64 }
pub struct RpcResponse { pub result: Option<Value>, pub error: Option<RpcError>, pub id: u64 }
pub struct RpcError    { pub code: i32, pub message: String }
pub struct StatusInfo  { pub validator_id: u64, pub current_view: u64, pub last_committed_height: u64, pub mempool_size: usize }
pub struct TxResult    { pub accepted: bool }
```

## License

GPL-3.0-only
