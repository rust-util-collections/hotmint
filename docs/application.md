# Application

The `Application` trait is hotmint's equivalent of Tendermint's ABCI (Application BlockChain Interface). It defines the boundary between the consensus engine and your business logic.

## Trait Definition

```rust
pub trait Application: Send + Sync {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8>          { vec![] }
    fn validate_block(&self, _block: &Block, _ctx: &BlockContext) -> bool { true }
    fn validate_tx(&self, _tx: &[u8]) -> bool                        { true }
    fn begin_block(&self, _ctx: &BlockContext) -> Result<()>          { Ok(()) }
    fn deliver_tx(&self, _tx: &[u8]) -> Result<()>                   { Ok(()) }
    fn end_block(&self, _ctx: &BlockContext) -> Result<EndBlockResponse> { Ok(EndBlockResponse::default()) }
    fn on_evidence(&self, _proof: &EquivocationProof) -> Result<()>  { Ok(()) }
    fn query(&self, _path: &str, _data: &[u8]) -> Result<Vec<u8>>   { Ok(vec![]) }

    // The only required method
    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()>;
}
```

All methods except `on_commit` have default implementations, so you can incrementally adopt functionality.

### BlockContext

Most `Application` methods receive a `BlockContext` that provides full consensus context:

```rust
pub struct BlockContext<'a> {
    pub height: Height,
    pub view: ViewNumber,
    pub proposer: ValidatorId,
    pub epoch: EpochNumber,
    pub validator_set: &'a ValidatorSet,
}
```

This gives the application access to the current epoch number and the active validator set without maintaining separate state.

## Lifecycle

When a block is committed, the consensus engine invokes the application methods in this order:

```
begin_block(ctx)
    │
    ├── deliver_tx(tx_1)
    ├── deliver_tx(tx_2)
    ├── ...
    └── deliver_tx(tx_n)
    │
end_block(ctx)  →  EndBlockResponse { validator_updates }
    │
on_commit(block, ctx)
```

The payload in the committed block is decoded into individual transactions (using `Mempool::decode_payload`), and each is delivered via `deliver_tx`.

If `end_block` returns an `EndBlockResponse` with non-empty `validator_updates`, an epoch transition is scheduled. The new validator set takes effect at the next view boundary.

## Method Reference

### `on_commit(block: &Block, ctx: &BlockContext) -> Result<()>` (required)

Called after a block is finalized and all transactions have been delivered. This is where you apply the block to your application state (update databases, emit events, etc.). The `ctx` provides the current epoch and validator set.

```rust
fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()> {
    self.db.apply_block(block.height.as_u64(), &block.payload)?;
    self.event_bus.emit(BlockCommitted {
        height: block.height,
        epoch: ctx.epoch,
    });
    Ok(())
}
```

### `create_payload(ctx: &BlockContext) -> Vec<u8>`

Called when this validator is the **leader** and needs to propose a new block. Return the block payload — typically serialized transactions collected from the mempool.

```rust
fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
    // collect up to 1MB of pending transactions
    let rt = tokio::runtime::Handle::current();
    rt.block_on(self.mempool.collect_payload(1_048_576))
}
```

If you return an empty `Vec<u8>`, the leader proposes an empty block.

### `validate_block(block: &Block, ctx: &BlockContext) -> bool`

Called by **replicas** before voting on a proposed block. Return `false` to reject the proposal (the replica will not vote). The `ctx` provides the current epoch and validator set for context-aware validation.

```rust
fn validate_block(&self, block: &Block, _ctx: &BlockContext) -> bool {
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

### `begin_block(ctx: &BlockContext) -> Result<()>`

Called at the start of block execution, before any `deliver_tx` calls. Use this to prepare per-block state.

```rust
fn begin_block(&self, ctx: &BlockContext) -> Result<()> {
    self.state.lock().begin_tx_batch(ctx.height.as_u64());
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

### `end_block(ctx: &BlockContext) -> Result<EndBlockResponse>`

Called after all transactions have been delivered, before `on_commit`. Use this for per-block finalization (e.g., committing a database transaction batch). Return an `EndBlockResponse` to optionally trigger an epoch transition with validator set updates.

```rust
fn end_block(&self, ctx: &BlockContext) -> Result<EndBlockResponse> {
    self.state.lock().commit_tx_batch(ctx.height.as_u64());
    Ok(EndBlockResponse::default()) // no validator changes
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

### `on_evidence(proof: &EquivocationProof) -> Result<()>`

Called when equivocation (double-voting) is detected by the VoteCollector. A validator that votes for two different blocks in the same (view, vote_type) produces an `EquivocationProof`. The application can use this callback to implement slashing logic.

```rust
fn on_evidence(&self, proof: &EquivocationProof) -> Result<()> {
    tracing::warn!(
        validator = ?proof.validator,
        view = proof.view.as_u64(),
        "equivocation detected — slashing"
    );
    self.slashing_state.lock().slash(proof.validator, proof.view);
    Ok(())
}
```

The `EquivocationProof` contains:

```rust
pub struct EquivocationProof {
    pub validator: ValidatorId,
    pub view: ViewNumber,
    pub vote_type: VoteType,
    pub block_hash_a: BlockHash,
    pub signature_a: Signature,
    pub block_hash_b: BlockHash,
    pub signature_b: Signature,
}
```

Both signatures are retained so that any third party can independently verify the proof.

## EndBlockResponse and Epoch Transitions

`end_block` returns an `EndBlockResponse` that can trigger dynamic validator set changes:

```rust
pub struct EndBlockResponse {
    pub validator_updates: Vec<ValidatorUpdate>,
}

pub struct ValidatorUpdate {
    pub id: ValidatorId,
    pub public_key: PublicKey,
    pub power: u64, // power = 0 means remove the validator
}
```

When `validator_updates` is non-empty, the consensus engine schedules an **epoch transition**:

1. The engine applies the updates to the current `ValidatorSet` to produce a new set.
2. A new `Epoch` is constructed with an incremented `EpochNumber` and the updated validator set.
3. The transition takes effect at the next view boundary (in `advance_view_to`).
4. The new epoch is persisted via `PersistentConsensusState::save_current_epoch`.

Example: removing a slashed validator at a specific block height:

```rust
fn end_block(&self, ctx: &BlockContext) -> Result<EndBlockResponse> {
    let slashed = self.slashing_state.lock().drain_slashed();
    if slashed.is_empty() {
        return Ok(EndBlockResponse::default());
    }
    let updates = slashed
        .into_iter()
        .map(|id| {
            let info = ctx.validator_set.get(&id).unwrap();
            ValidatorUpdate {
                id,
                public_key: info.public_key.clone(),
                power: 0, // remove
            }
        })
        .collect();
    Ok(EndBlockResponse { validator_updates: updates })
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

    fn validate_block(&self, block: &Block, _ctx: &BlockContext) -> bool {
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

    fn on_commit(&self, block: &Block, _ctx: &BlockContext) -> Result<()> {
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

For testing or when you don't need application logic. `NoopApplication` implements the required `on_commit` as a no-op:

```rust
pub struct NoopApplication;

impl Application for NoopApplication {
    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        Ok(())
    }
}
```

Usage:

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
