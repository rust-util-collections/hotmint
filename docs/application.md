# Application

The `Application` trait is hotmint's equivalent of Tendermint's ABCI (Application BlockChain Interface). It defines the boundary between the consensus engine and your business logic.

## Trait Definition

```rust
pub trait Application: Send + Sync {
    fn create_payload(&self) -> Vec<u8>           { vec![] }
    fn validate_block(&self, _block: &Block) -> bool { true }
    fn validate_tx(&self, _tx: &[u8]) -> bool     { true }
    fn begin_block(&self, _height: Height, _view: ViewNumber) -> Result<()> { Ok(()) }
    fn deliver_tx(&self, _tx: &[u8]) -> Result<()> { Ok(()) }
    fn end_block(&self, _height: Height) -> Result<()> { Ok(()) }
    fn query(&self, _path: &str, _data: &[u8]) -> Result<Vec<u8>> { Ok(vec![]) }

    // The only required method
    fn on_commit(&self, block: &Block) -> Result<()>;
}
```

All methods except `on_commit` have default implementations, so you can incrementally adopt functionality.

## Lifecycle

When a block is committed, the consensus engine invokes the application methods in this order:

```
begin_block(height, view)
    │
    ├── deliver_tx(tx_1)
    ├── deliver_tx(tx_2)
    ├── ...
    └── deliver_tx(tx_n)
    │
end_block(height)
    │
on_commit(block)
```

The payload in the committed block is decoded into individual transactions (using `Mempool::decode_payload`), and each is delivered via `deliver_tx`.

## Method Reference

### `on_commit(block: &Block) -> Result<()>` (required)

Called after a block is finalized and all transactions have been delivered. This is where you apply the block to your application state (update databases, emit events, etc.).

```rust
fn on_commit(&self, block: &Block) -> Result<()> {
    self.db.apply_block(block.height.as_u64(), &block.payload)?;
    self.event_bus.emit(BlockCommitted { height: block.height });
    Ok(())
}
```

### `create_payload() -> Vec<u8>`

Called when this validator is the **leader** and needs to propose a new block. Return the block payload — typically serialized transactions collected from the mempool.

```rust
fn create_payload(&self) -> Vec<u8> {
    // collect up to 1MB of pending transactions
    let rt = tokio::runtime::Handle::current();
    rt.block_on(self.mempool.collect_payload(1_048_576))
}
```

If you return an empty `Vec<u8>`, the leader proposes an empty block.

### `validate_block(block: &Block) -> bool`

Called by **replicas** before voting on a proposed block. Return `false` to reject the proposal (the replica will not vote).

```rust
fn validate_block(&self, block: &Block) -> bool {
    // reject blocks with oversized payloads
    if block.payload.len() > 2_097_152 {
        return false;
    }
    // verify all transactions in the payload
    let txs = hotmint::mempool::Mempool::decode_payload(&block.payload);
    txs.iter().all(|tx| self.validate_tx(tx))
}
```

### `validate_tx(tx: &[u8]) -> bool`

Validate an individual transaction. Used by the mempool to filter incoming transactions before they enter the pool.

```rust
fn validate_tx(&self, tx: &[u8]) -> bool {
    // must be valid protobuf
    MyTransaction::decode(tx).is_ok()
}
```

### `begin_block(height: Height, view: ViewNumber) -> Result<()>`

Called at the start of block execution, before any `deliver_tx` calls. Use this to prepare per-block state.

```rust
fn begin_block(&self, height: Height, view: ViewNumber) -> Result<()> {
    self.state.lock().begin_tx_batch(height.as_u64());
    Ok(())
}
```

### `deliver_tx(tx: &[u8]) -> Result<()>`

Called once per transaction in the committed block's payload. Apply the transaction to your application state.

```rust
fn deliver_tx(&self, tx: &[u8]) -> Result<()> {
    let tx = MyTransaction::decode(tx).map_err(|e| eg!("decode: {e}"))?;
    self.state.lock().execute(tx)?;
    Ok(())
}
```

### `end_block(height: Height) -> Result<()>`

Called after all transactions have been delivered, before `on_commit`. Use this for per-block finalization (e.g., committing a database transaction batch).

```rust
fn end_block(&self, height: Height) -> Result<()> {
    self.state.lock().commit_tx_batch(height.as_u64());
    Ok(())
}
```

### `query(path: &str, data: &[u8]) -> Result<Vec<u8>>`

Handle read-only queries from the JSON-RPC API. The `path` is a method name, and `data` is request-specific.

```rust
fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
    match path {
        "balance" => {
            let addr = String::from_utf8_lossy(data);
            let balance = self.state.lock().get_balance(&addr);
            Ok(balance.to_le_bytes().to_vec())
        }
        "tx_count" => {
            let count = self.state.lock().tx_count();
            Ok(count.to_le_bytes().to_vec())
        }
        _ => Err(eg!("unknown query: {path}")),
    }
}
```

## Complete Example

A key-value store application with transaction support:

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use ruc::*;
use hotmint::prelude::*;
use hotmint::consensus::application::Application;

struct KvStoreApp {
    store: Mutex<HashMap<String, String>>,
}

impl KvStoreApp {
    fn new() -> Self {
        Self { store: Mutex::new(HashMap::new()) }
    }
}

impl Application for KvStoreApp {
    fn validate_tx(&self, tx: &[u8]) -> bool {
        // expect "key=value" format
        let s = String::from_utf8_lossy(tx);
        s.contains('=') && s.split('=').count() == 2
    }

    fn validate_block(&self, block: &Block) -> bool {
        let txs = hotmint::mempool::Mempool::decode_payload(&block.payload);
        txs.iter().all(|tx| self.validate_tx(tx))
    }

    fn deliver_tx(&self, tx: &[u8]) -> Result<()> {
        let s = String::from_utf8_lossy(tx);
        let mut parts = s.splitn(2, '=');
        let key = parts.next().ok_or_else(|| eg!("missing key"))?.to_string();
        let val = parts.next().ok_or_else(|| eg!("missing value"))?.to_string();
        self.store.lock().unwrap().insert(key, val);
        Ok(())
    }

    fn on_commit(&self, block: &Block) -> Result<()> {
        let store = self.store.lock().unwrap();
        println!(
            "height={} entries={} hash={}",
            block.height.as_u64(),
            store.len(),
            block.hash
        );
        Ok(())
    }

    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        match path {
            "get" => {
                let key = String::from_utf8_lossy(data);
                let store = self.store.lock().unwrap();
                match store.get(key.as_ref()) {
                    Some(v) => Ok(v.as_bytes().to_vec()),
                    None => Ok(vec![]),
                }
            }
            _ => Err(eg!("unknown query: {path}")),
        }
    }
}
```

## NoopApplication

For testing or when you don't need application logic:

```rust
use hotmint::consensus::application::NoopApplication;

let engine = ConsensusEngine::new(
    state,
    Box::new(store),
    Box::new(network),
    Box::new(NoopApplication),
    Box::new(signer),
    msg_rx,
);
```
