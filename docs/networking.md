# Networking

Hotmint uses [litep2p](https://crates.io/crates/litep2p) for P2P networking. The `NetworkService` manages peer connections, consensus message delivery, block synchronization, and peer exchange.

## NetworkSink Trait

```rust
pub trait NetworkSink: Send + Sync {
    fn broadcast(&self, msg: ConsensusMessage);
    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage);
    fn on_epoch_change(&self, new_validator_set: &ValidatorSet) {}
}
```

- `broadcast` — send a message to all validators (used for proposals, prepare, wishes, TCs)
- `send_to` — send a message to a specific validator (used for votes)

## litep2p NetworkService

For multi-process and multi-machine deployments, `NetworkService` provides real P2P networking using [litep2p](https://crates.io/crates/litep2p).

### Architecture

```
NetworkService
  ├── litep2p instance (manages TCP connections)
  ├── Notification protocol: /hotmint/consensus/notif/1 (broadcast)
  ├── Request-Response protocol: /hotmint/consensus/reqresp/1 (directed)
  ├── Sync protocol: /hotmint/sync/1 (block synchronization)
  ├── PeerMap (ValidatorId <-> PeerId mapping, supports runtime add/remove)
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

`PeerMap` also supports runtime removal of peers:

```rust
// remove a peer by ValidatorId (returns the removed PeerId if present)
let removed_peer: Option<PeerId> = peer_map.remove(ValidatorId(2));
```

### Creating the Service

```rust
use hotmint::network::service::{NetworkConfig, NetworkService, NetworkServiceHandles};

let listen_addr = "/ip4/0.0.0.0/tcp/20000".parse().unwrap();

let known_addresses = vec![
    (peer_id_0, vec!["/ip4/10.0.0.1/tcp/20000".parse().unwrap()]),
    (peer_id_1, vec!["/ip4/10.0.0.2/tcp/20000".parse().unwrap()]),
    (peer_id_2, vec!["/ip4/10.0.0.3/tcp/20000".parse().unwrap()]),
    (peer_id_3, vec!["/ip4/10.0.0.4/tcp/20000".parse().unwrap()]),
];

// validator_keys: list of (ValidatorId, PublicKey) for relay signature verification
let validator_keys = genesis.validators.iter()
    .map(|v| (v.id, v.public_key.clone()))
    .collect();

let NetworkServiceHandles {
    service: net_service,
    sink: network_sink,
    msg_rx,
    sync_req_rx,
    sync_resp_rx,
    peer_info_rx,
    connected_count_rx,
    notif_connected_count_rx,
} = NetworkService::create(NetworkConfig {
    listen_addr,
    peer_map,
    known_addresses,
    keypair: None,             // Option<litep2p::crypto::ed25519::Keypair> — None generates a random keypair
    peer_book,                 // Arc<tokio::sync::RwLock<PeerBook>> (persistent peer address store)
    pex_config,                // PexConfig (peer exchange settings)
    relay_consensus: false,    // whether to relay consensus messages to other peers
    initial_validators: validator_keys, // initial validator public keys for relay sender verification
    chain_id_hash,             // [u8; 32] — Blake3 hash of the chain ID (for relay signature verification)
}).unwrap();
```

`NetworkService::create` takes a single `NetworkConfig` struct with nine fields:
1. `listen_addr` — P2P listen address (multiaddr)
2. `peer_map` — mapping of `ValidatorId` ↔ `PeerId`
3. `known_addresses` — bootstrap peer addresses
4. `keypair` — `Option<litep2p::crypto::ed25519::Keypair>` (`None` generates a random keypair)
5. `peer_book` — persistent peer address store (`Arc<tokio::sync::RwLock<PeerBook>>`)
6. `pex_config` — peer exchange settings
7. `relay_consensus: bool` — whether to relay consensus messages to other validators
8. `initial_validators` — initial set of `(ValidatorId, PublicKey)` for relay sender signature verification
9. `chain_id_hash: [u8; 32]` — Blake3 hash of the chain identifier, used for relay signature verification (must match the `chain_id_hash` in `ConsensusState`)

It returns a `NetworkServiceHandles` struct with named fields:
1. `service: NetworkService` — the service itself, must be `.run()` on a tokio task
2. `sink: Litep2pNetworkSink` — implements `NetworkSink`, pass to `ConsensusEngine`
3. `msg_rx: Receiver<(Option<ValidatorId>, ConsensusMessage)>` — incoming consensus messages; sender is `None` for unknown peers
4. `sync_req_rx: Receiver<IncomingSyncRequest>` — incoming sync requests from peers
5. `sync_resp_rx: Receiver<SyncResponse>` — incoming sync responses from peers
6. `peer_info_rx: watch::Receiver<Vec<PeerStatus>>` — live peer connection status updates
7. `connected_count_rx: watch::Receiver<usize>` — number of TCP-connected peers
8. `notif_connected_count_rx: watch::Receiver<usize>` — number of peers with an open notification substream (ready for consensus)

### PeerBook

The `PeerBook` is a persistent peer address store wrapped in `Arc<tokio::sync::RwLock<PeerBook>>` (note: `tokio::sync::RwLock`, not `std::sync::RwLock`). It is shared between the `NetworkService` and the PEX subsystem.

### PEX Rate Limiting

Peer Exchange (PEX) requests are rate-limited per peer with a 10-second cooldown. If a peer sends a PEX request within 10 seconds of its last request, the request is rejected. This prevents peers from flooding the network with PEX requests.

### Relay Consensus

When `relay_consensus` is `true`, the `NetworkService` relays received consensus messages to other connected peers. This is useful for non-validator nodes that act as relays. Relay deduplication uses a two-set rotation strategy to avoid re-broadcasting messages that have already been seen.

### Running

```rust
// run the network event loop (infinite loop)
tokio::spawn(async move { net_service.run().await });

// build the consensus engine with the P2P network sink
use std::sync::Arc;
use tokio::sync::RwLock;
use hotmint::consensus::engine::{ConsensusEngineBuilder, SharedBlockStore};
use hotmint::crypto::Ed25519Verifier;

let shared_store: SharedBlockStore = Arc::new(RwLock::new(Box::new(store)));
let engine = ConsensusEngineBuilder::new()
    .state(state)
    .store(shared_store)
    .network(Box::new(network_sink))
    .app(Box::new(app))
    .signer(Box::new(signer))
    .messages(msg_rx)
    .verifier(Box::new(Ed25519Verifier))
    .build()
    .expect("all required fields must be set");
tokio::spawn(async move { engine.run().await });
```

### Message Serialization

All `ConsensusMessage` values are serialized with CBOR (`serde_cbor_2`) before transmission and deserialized on receipt. This is handled automatically by the `NetworkService`.

### Sub-Protocols

| Protocol | Path | Use |
|:---------|:-----|:----|
| Notification | `/hotmint/consensus/notif/1` | `broadcast()` — sends to all connected peers |
| Request-Response | `/hotmint/consensus/reqresp/1` | `send_to()` — sends to a specific peer |
| Sync | `/hotmint/sync/1` | Block synchronization — request-response for `SyncRequest`/`SyncResponse` |

The notification protocol is fire-and-forget. The request-response protocol sends a message and expects an acknowledgment (empty response). The sync protocol is a dedicated request-response channel used by the block sync subsystem to request missing blocks from peers.

## Full P2P Node Example

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use hotmint::prelude::*;
use hotmint::consensus::engine::{ConsensusEngineBuilder, SharedBlockStore};
use hotmint::consensus::state::ConsensusState;
use hotmint::crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint::storage::block_store::VsdbBlockStore;
use hotmint::storage::consensus_state::PersistentConsensusState;
use hotmint::network::service::{NetworkConfig, NetworkService, NetworkServiceHandles, PeerMap};

async fn run_validator(
    vid: ValidatorId,
    signer: Ed25519Signer,
    validator_set: ValidatorSet,
    peer_map: PeerMap,
    listen_addr: litep2p::types::multiaddr::Multiaddr,
    known_addresses: Vec<(litep2p::PeerId, Vec<litep2p::types::multiaddr::Multiaddr>)>,
    app: impl hotmint::consensus::application::Application + 'static,
    chain_id: &str,
) {
    // persistent storage
    let store = VsdbBlockStore::new();
    let pstate = PersistentConsensusState::new();

    // recover state (with chain ID for cross-chain replay prevention)
    let mut state = ConsensusState::with_chain_id(vid, validator_set, chain_id);
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
    let NetworkServiceHandles {
        service: net_service,
        sink: network_sink,
        msg_rx,
        sync_req_rx,
        sync_resp_rx,
        peer_info_rx,
        connected_count_rx,
        notif_connected_count_rx,
    } = NetworkService::create(NetworkConfig {
        listen_addr,
        peer_map,
        known_addresses,
        keypair: None,
        peer_book,
        pex_config,
        relay_consensus: false,
        initial_validators: validator_keys,
        chain_id_hash: state.chain_id_hash,
    }).unwrap();
    tokio::spawn(async move { net_service.run().await });

    // consensus engine
    let shared_store: SharedBlockStore =
        Arc::new(RwLock::new(Box::new(store)));
    let engine = ConsensusEngineBuilder::new()
        .state(state)
        .store(shared_store)
        .network(Box::new(network_sink))
        .app(Box::new(app))
        .signer(Box::new(signer))
        .messages(msg_rx)
        .verifier(Box::new(Ed25519Verifier))
        .build()
        .expect("all required fields must be set");
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
        let bytes = serde_cbor_2::to_vec(&msg).unwrap();
        // send `bytes` to all known peers
    }

    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage) {
        let bytes = serde_cbor_2::to_vec(&msg).unwrap();
        // send `bytes` to the peer corresponding to `target`
    }
}
```

You also need to provide the `mpsc::Receiver<(Option<ValidatorId>, ConsensusMessage)>` to the engine. When your network layer receives a message, deserialize it and send it through the channel. Use `Some(sender_id)` for known validators and `None` for unknown/unauthenticated peers:

```rust
let (msg_tx, msg_rx) = tokio::sync::mpsc::channel(8192);

// in your network receive loop:
let sender_id = identify_sender(&peer); // Option<ValidatorId>
let msg: ConsensusMessage = serde_cbor_2::from_slice(&bytes).unwrap();
msg_tx.send((sender_id, msg)).unwrap();

// pass msg_rx to ConsensusEngine::new()
```

## Dynamic Peer Management

The `Litep2pNetworkSink` supports adding and removing peers at runtime via `NetCommand::AddPeer` and `NetCommand::RemovePeer`. This enables dynamic validator set changes without restarting the network service.

```rust
// add a new peer at runtime
network_sink.add_peer(
    ValidatorId(4),
    new_peer_id,
    vec!["/ip4/10.0.0.5/tcp/20000".parse().unwrap()],
);

// remove a peer at runtime
network_sink.remove_peer(ValidatorId(4));
```

These methods send commands through an internal channel to the `NetworkService`, which updates the `PeerMap` and peer info accordingly. This is typically used in conjunction with validator set changes triggered by `Application::execute_block()` returning new `ValidatorUpdate` entries.

## Block Synchronization

The `/hotmint/sync/1` request-response protocol enables new or lagging nodes to catch up with the network by requesting missing blocks from peers.

The protocol uses `SyncRequest` and `SyncResponse` messages (defined in `hotmint_types::sync`) serialized with CBOR. The `Litep2pNetworkSink` provides methods for initiating sync:

```rust
// send a sync request to a specific peer
network_sink.send_sync_request(peer_id, &sync_request);

// respond to an incoming sync request
network_sink.send_sync_response(request_id, &sync_response);
```

Incoming sync requests are forwarded to the sync handler via `IncomingSyncRequest`, which contains the `request_id`, originating `peer`, and the deserialized `SyncRequest`.
