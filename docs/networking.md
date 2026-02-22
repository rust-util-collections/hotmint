# Networking

Hotmint provides two `NetworkSink` implementations: in-memory channels for single-process setups and litep2p for real P2P networking.

## NetworkSink Trait

```rust
pub trait NetworkSink: Send + Sync {
    fn broadcast(&self, msg: ConsensusMessage);
    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage);
}
```

- `broadcast` — send a message to all validators (used for proposals, prepare, wishes, TCs)
- `send_to` — send a message to a specific validator (used for votes)

## ChannelNetwork (In-Memory)

Connects validators within a single process via `tokio::mpsc` unbounded channels. Ideal for testing, development, and benchmarks.

```rust
use hotmint::consensus::network::ChannelNetwork;
use tokio::sync::mpsc;

// create a channel for each validator
let mut receivers = HashMap::new();
let mut all_senders = HashMap::new();
for i in 0..num_validators {
    let (tx, rx) = mpsc::unbounded_channel();
    receivers.insert(ValidatorId(i), rx);
    all_senders.insert(ValidatorId(i), tx);
}

// build each validator's network sink
for i in 0..num_validators {
    let vid = ValidatorId(i);
    let senders: Vec<_> = all_senders
        .iter()
        .map(|(&id, tx)| (id, tx.clone()))
        .collect();

    let network = ChannelNetwork::new(vid, senders);
    let rx = receivers.remove(&vid).unwrap();

    // pass `network` as Box<dyn NetworkSink> and `rx` to ConsensusEngine::new()
}
```

Behavior:
- `broadcast` sends to all channels except the sender's own
- `send_to` sends to the matching target channel
- Messages are delivered immediately (no serialization, no network latency)

## litep2p NetworkService (P2P)

For multi-process and multi-machine deployments, `NetworkService` provides real P2P networking using [litep2p](https://crates.io/crates/litep2p).

### Architecture

```
NetworkService
  ├── litep2p instance (manages TCP connections)
  ├── Notification protocol: /hotmint/consensus/notif/1 (broadcast)
  ├── Request-Response protocol: /hotmint/consensus/reqresp/1 (directed)
  ├── PeerMap (ValidatorId <-> PeerId mapping)
  └── mpsc channels (bridge to ConsensusEngine)
```

### PeerMap

Maps between consensus-level `ValidatorId` and network-level `PeerId`:

```rust
use hotmint::network::service::PeerMap;

let mut peer_map = PeerMap::new();
peer_map.insert(ValidatorId(0), peer_id_0);
peer_map.insert(ValidatorId(1), peer_id_1);
peer_map.insert(ValidatorId(2), peer_id_2);
peer_map.insert(ValidatorId(3), peer_id_3);
```

Each validator's `PeerId` is derived from its litep2p keypair. You need to distribute these mappings out-of-band (e.g., via a configuration file or genesis file).

### Creating the Service

```rust
use hotmint::network::service::NetworkService;

let listen_addr = "/ip4/0.0.0.0/tcp/30000".parse().unwrap();

let known_addresses = vec![
    (peer_id_0, vec!["/ip4/10.0.0.1/tcp/30000".parse().unwrap()]),
    (peer_id_1, vec!["/ip4/10.0.0.2/tcp/30000".parse().unwrap()]),
    (peer_id_2, vec!["/ip4/10.0.0.3/tcp/30000".parse().unwrap()]),
    (peer_id_3, vec!["/ip4/10.0.0.4/tcp/30000".parse().unwrap()]),
];

let (net_service, network_sink, msg_rx) = NetworkService::create(
    listen_addr,
    peer_map,
    known_addresses,
).unwrap();
```

`NetworkService::create` returns three items:
1. `net_service: NetworkService` — the service itself, must be `.run()` on a tokio task
2. `network_sink: Litep2pNetworkSink` — implements `NetworkSink`, pass to `ConsensusEngine`
3. `msg_rx: UnboundedReceiver<(ValidatorId, ConsensusMessage)>` — incoming messages, pass to `ConsensusEngine`

### Running

```rust
// run the network event loop (infinite loop)
tokio::spawn(async move { net_service.run().await });

// build the consensus engine with the P2P network sink
let engine = ConsensusEngine::new(
    state,
    Box::new(store),
    Box::new(network_sink),
    Box::new(app),
    Box::new(signer),
    msg_rx,
);
tokio::spawn(async move { engine.run().await });
```

### Message Serialization

All `ConsensusMessage` values are serialized with MessagePack (`rmp-serde`) before transmission and deserialized on receipt. This is handled automatically by the `NetworkService`.

### Sub-Protocols

| Protocol | Path | Use |
|:---------|:-----|:----|
| Notification | `/hotmint/consensus/notif/1` | `broadcast()` — sends to all connected peers |
| Request-Response | `/hotmint/consensus/reqresp/1` | `send_to()` — sends to a specific peer |

The notification protocol is fire-and-forget. The request-response protocol sends a message and expects an acknowledgment (empty response).

## Full P2P Node Example

```rust
use std::sync::Arc;
use hotmint::prelude::*;
use hotmint::consensus::engine::ConsensusEngine;
use hotmint::consensus::state::ConsensusState;
use hotmint::crypto::Ed25519Signer;
use hotmint::storage::block_store::VsdbBlockStore;
use hotmint::storage::consensus_state::PersistentConsensusState;
use hotmint::network::service::{NetworkService, PeerMap};

async fn run_validator(
    vid: ValidatorId,
    signer: Ed25519Signer,
    validator_set: ValidatorSet,
    peer_map: PeerMap,
    listen_addr: litep2p::types::multiaddr::Multiaddr,
    known_addresses: Vec<(litep2p::PeerId, Vec<litep2p::types::multiaddr::Multiaddr>)>,
    app: impl hotmint::consensus::application::Application + 'static,
) {
    // persistent storage
    let store = VsdbBlockStore::new();
    let pstate = PersistentConsensusState::new();

    // recover state
    let mut state = ConsensusState::new(vid, validator_set);
    if let Some(v) = pstate.load_current_view() {
        state.current_view = v;
    }
    if let Some(qc) = pstate.load_locked_qc() {
        state.locked_qc = Some(qc);
    }
    if let Some(qc) = pstate.load_highest_qc() {
        state.highest_qc = Some(qc);
    }
    if let Some(h) = pstate.load_last_committed_height() {
        state.last_committed_height = h;
    }

    // P2P networking
    let (net_service, network_sink, msg_rx) =
        NetworkService::create(listen_addr, peer_map, known_addresses).unwrap();
    tokio::spawn(async move { net_service.run().await });

    // consensus engine
    let engine = ConsensusEngine::new(
        state,
        Box::new(store),
        Box::new(network_sink),
        Box::new(app),
        Box::new(signer),
        msg_rx,
    );
    engine.run().await;
}
```

## Implementing a Custom NetworkSink

To integrate with a different networking stack:

```rust
use hotmint::prelude::*;
use hotmint::consensus::network::NetworkSink;

struct MyNetworkSink {
    // your networking state
}

impl NetworkSink for MyNetworkSink {
    fn broadcast(&self, msg: ConsensusMessage) {
        let bytes = rmp_serde::to_vec(&msg).unwrap();
        // send `bytes` to all known peers
    }

    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage) {
        let bytes = rmp_serde::to_vec(&msg).unwrap();
        // send `bytes` to the peer corresponding to `target`
    }
}
```

You also need to provide the `mpsc::UnboundedReceiver<(ValidatorId, ConsensusMessage)>` to the engine. When your network layer receives a message, deserialize it and send it through the channel:

```rust
let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();

// in your network receive loop:
let sender_id = identify_sender(&peer);
let msg: ConsensusMessage = rmp_serde::from_slice(&bytes).unwrap();
msg_tx.send((sender_id, msg)).unwrap();

// pass msg_rx to ConsensusEngine::new()
```
