# evm-chain

EVM-compatible chain example for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

A complete working example of an EVM chain running on hotmint consensus, using [revm](https://github.com/bluealloy/revm) for execution and vsdb `MptCalc` for state trie maintenance.

> **Note:** This example uses simplified unsigned transactions (no ECDSA signature verification). Production EVM chains must add secp256k1 ECDSA recovery.

## Binaries

| Binary | Description |
|:-------|:------------|
| `evm-chain-example` | 4-validator demo: Alice sends 1 ETH to Bob each block (30s) |
| `bench-evm` | Throughput benchmark: 10 transfers/block across multiple timeout configs |

## Run

```bash
# Demo (30 seconds)
cargo run -p evm-chain-example

# Benchmark
cargo run --release -p evm-chain-example --bin bench-evm
```

## Architecture

- `evm_app.rs` — `EvmApplication`: full `Application` trait implementation with revm execution
- `evm_tx.rs` — `EvmTx`: CBOR-encoded transaction format with dummy signatures
- `app.rs` — `DemoEvmApp`: wrapper that auto-generates Alice→Bob transfers
- `bench.rs` — `EvmBenchApp`: standalone benchmark application

## License

GPL-3.0-only
