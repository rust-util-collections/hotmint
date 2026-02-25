# hotmint-mempool

[![crates.io](https://img.shields.io/crates/v/hotmint-mempool.svg)](https://crates.io/crates/hotmint-mempool)
[![docs.rs](https://docs.rs/hotmint-mempool/badge.svg)](https://docs.rs/hotmint-mempool)

Transaction mempool for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

A thread-safe, async transaction pool with FIFO ordering, Blake3-based deduplication, and configurable size limits. Provides length-prefixed payload encoding for block inclusion.

## Features

- **FIFO ordering** — transactions are proposed in the order they were received
- **Deduplication** — duplicate transactions (by Blake3 hash) are silently rejected
- **Size limits** — configurable max transaction count and per-transaction byte limit
- **Payload encoding** — length-prefixed format for embedding transactions in blocks
- **Thread-safe** — all operations are `async` and safe for concurrent access

## Usage

```rust
use hotmint_mempool::Mempool;

// custom limits: max 10,000 txs, 1MB per tx
let mempool = Mempool::new(10_000, 1_048_576);

// or use defaults (10k txs, 1MB)
let mempool = Mempool::default();
```

### Add transactions

```rust
// returns true if accepted, false if rejected (duplicate or full)
let accepted = mempool.add_tx(b"transfer alice bob 100".to_vec()).await;
```

### Collect payload for block proposal

```rust
// drain up to 1MB of transactions for block inclusion
let payload = mempool.collect_payload(1_048_576).await;
```

### Decode payload from a committed block

```rust
let txs: Vec<Vec<u8>> = Mempool::decode_payload(&block.payload);
for tx in &txs {
    // process each transaction
}
```

### Integration with Application trait

```rust
use std::sync::Arc;
use hotmint_consensus::application::Application;
use hotmint_mempool::Mempool;

struct MyApp {
    mempool: Arc<Mempool>,
}

impl Application for MyApp {
    fn create_payload(&self, _ctx: &hotmint_types::context::BlockContext) -> Vec<u8> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.mempool.collect_payload(1_048_576))
    }

    fn on_commit(&self, block: &hotmint_types::Block, _ctx: &hotmint_types::context::BlockContext) -> ruc::Result<()> {
        let txs = Mempool::decode_payload(&block.payload);
        println!("committed {} txs", txs.len());
        Ok(())
    }
}
```

## License

GPL-3.0-only
