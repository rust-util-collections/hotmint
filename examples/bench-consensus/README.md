# bench-consensus

Consensus throughput benchmark for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Measures raw consensus throughput (blocks/sec) using 4 in-process validators with 1 KB fixed payloads, isolating consensus overhead from application execution.

## Run

```bash
cargo run --release -p bench-consensus
```

## License

GPL-3.0-only
