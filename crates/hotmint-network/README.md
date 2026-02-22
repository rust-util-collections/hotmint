# hotmint-network

[![crates.io](https://img.shields.io/crates/v/hotmint-network.svg)](https://crates.io/crates/hotmint-network)
[![docs.rs](https://docs.rs/hotmint-network/badge.svg)](https://docs.rs/hotmint-network)

P2P networking layer for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Implements the `NetworkSink` trait from `hotmint-consensus` using [litep2p](https://crates.io/crates/litep2p) for real multi-process / multi-machine deployments. Messages are serialized with MessagePack.

## Sub-Protocols

| Protocol | Path | Use |
|:---------|:-----|:----|
| Notification | `/hotmint/consensus/notif/1` | `broadcast()` — fire-and-forget to all peers |
| Request-Response | `/hotmint/consensus/reqresp/1` | `send_to()` — directed message to a specific peer |

## Components

| Component | Description |
|:----------|:------------|
| `NetworkService` | Manages litep2p connections and event routing |
| `Litep2pNetworkSink` | Implements `NetworkSink` for production use |
| `PeerMap` | Bidirectional `ValidatorId ↔ PeerId` mapping |

## Usage

```rust
use hotmint_network::service::{NetworkService, PeerMap};

// map validator IDs to litep2p peer IDs
let mut peer_map = PeerMap::new();
peer_map.insert(ValidatorId(0), peer_id_0);
peer_map.insert(ValidatorId(1), peer_id_1);
// ...

let known_addresses = vec![
    (peer_id_0, vec!["/ip4/10.0.0.1/tcp/30000".parse().unwrap()]),
    (peer_id_1, vec!["/ip4/10.0.0.2/tcp/30000".parse().unwrap()]),
];

// create returns (service, network_sink, msg_rx)
let (net_service, network_sink, msg_rx) = NetworkService::create(
    "/ip4/0.0.0.0/tcp/30000".parse().unwrap(),
    peer_map,
    known_addresses,
).unwrap();

// run the network event loop
tokio::spawn(async move { net_service.run().await });

// pass network_sink and msg_rx to ConsensusEngine::new()
let engine = ConsensusEngine::new(
    state,
    Box::new(store),
    Box::new(network_sink),
    Box::new(app),
    Box::new(signer),
    msg_rx,
);
```

## License

GPL-3.0-only
