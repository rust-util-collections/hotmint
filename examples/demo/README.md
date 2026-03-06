# demo

Minimal consensus demo for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Spawns 4 in-process validators connected via channels and runs a simple counting application that logs each committed block.

## Run

```bash
cargo run -p hotmint-demo
```

## License

GPL-3.0-only
