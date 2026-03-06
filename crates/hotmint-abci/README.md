# hotmint-abci

[![crates.io](https://img.shields.io/crates/v/hotmint-abci.svg)](https://crates.io/crates/hotmint-abci)
[![docs.rs](https://docs.rs/hotmint-abci/badge.svg)](https://docs.rs/hotmint-abci)

IPC proxy layer (Application Binary Consensus Interface) for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Enables running the application logic in a separate process from the consensus engine, communicating over Unix domain sockets with length-prefixed CBOR frames.

## Architecture

```
┌──────────────┐  Unix socket  ┌──────────────────┐
│  Consensus   │◄─────────────►│   Application    │
│   Engine     │  CBOR frames  │    Process       │
│              │               │                  │
│ IpcApp       │               │ IpcApp           │
│  Client      │               │  Server          │
└──────────────┘               └──────────────────┘
```

## Components

| Type | Description |
|:-----|:------------|
| `IpcApplicationClient` | Implements `Application` trait, forwards calls over IPC |
| `IpcApplicationServer` | Unix socket listener, dispatches requests to handler |
| `ApplicationHandler` | Callback trait for the application process |
| `Request` / `Response` | Protocol message types (CBOR-serialized) |

## Protocol

Requests and responses are exchanged as length-prefixed CBOR frames over a Unix domain socket:

```
[4 bytes: payload length (LE)] [payload: CBOR-encoded Request/Response]
```

Supported operations: `CreatePayload`, `ValidateBlock`, `ValidateTx`, `ExecuteBlock`, `OnCommit`, `OnEvidence`, `Query`.

## Usage

### Application Process (Server)

```rust
use hotmint_abci::{ApplicationHandler, IpcApplicationServer};

struct MyApp;

impl ApplicationHandler for MyApp {
    fn create_payload(&self, height: u64, view: u64) -> Vec<u8> {
        vec![] // your payload logic
    }
    // ... implement other callbacks
}

let server = IpcApplicationServer::new(MyApp);
server.listen("/tmp/myapp.sock").await.unwrap();
```

### Consensus Process (Client)

```rust
use hotmint_abci::IpcApplicationClient;

let app = IpcApplicationClient::new("/tmp/myapp.sock");
// Pass to ConsensusEngine as Box<dyn Application>
```

## License

GPL-3.0-only
