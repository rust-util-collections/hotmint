# bench-ipc

IPC protocol benchmark for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Measures the overhead of the ABCI IPC layer (Unix socket + CBOR framing) by running consensus with an out-of-process application handler.

## Run

```bash
cargo run --release -p bench-ipc
```

## License

GPL-3.0-only
