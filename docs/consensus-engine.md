# Consensus Engine

The `ConsensusEngine` is the heart of hotmint — an async event loop that drives the HotStuff-2 protocol.

## Overview

```rust
pub struct ConsensusEngine {
    state: ConsensusState,
    store: Box<dyn BlockStore>,
    network: Box<dyn NetworkSink>,
    app: Box<dyn Application>,
    signer: Box<dyn Signer>,
    vote_collector: VoteCollector,
    pacemaker: Pacemaker,
    msg_rx: UnboundedReceiver<(ValidatorId, ConsensusMessage)>,
    status_count: usize,
    current_view_qc: Option<QuorumCertificate>,
}
```

The engine takes ownership of all its dependencies and runs as an infinite async loop. It is `Send` and designed to be spawned onto a tokio runtime.

## Construction

```rust
use std::sync::{Arc, RwLock};
use hotmint::consensus::engine::{ConsensusEngine, SharedBlockStore};
use hotmint::consensus::state::ConsensusState;

let store: SharedBlockStore = Arc::new(RwLock::new(Box::new(block_store)));

let engine = ConsensusEngine::new(
    state,                    // ConsensusState
    store,                    // SharedBlockStore = Arc<RwLock<Box<dyn BlockStore>>>
    Box::new(network_sink),   // impl NetworkSink
    Box::new(application),    // impl Application
    Box::new(signer),         // impl Signer
    msg_rx,                   // UnboundedReceiver<(ValidatorId, ConsensusMessage)>
);
```

The `msg_rx` channel is the engine's sole input. All consensus messages — whether from the network or from loopback — arrive through this channel as `(sender_id, message)` tuples.

## Running

```rust
// engine.run() consumes self and never returns
tokio::spawn(async move { engine.run().await });
```

The event loop:

```rust
loop {
    tokio::select! {
        Some((sender, msg)) = self.msg_rx.recv() => {
            self.handle_message(sender, msg);
        }
        _ = self.pacemaker.sleep_until_deadline() => {
            self.handle_timeout();
        }
    }
}
```

## ConsensusState

The mutable state tracked by the engine:

```rust
pub struct ConsensusState {
    pub validator_id: ValidatorId,
    pub validator_set: ValidatorSet,
    pub current_view: ViewNumber,
    pub role: ViewRole,               // Leader or Replica
    pub step: ViewStep,               // progress within the current view
    pub locked_qc: Option<QuorumCertificate>,
    pub highest_double_cert: Option<DoubleCertificate>,
    pub highest_qc: Option<QuorumCertificate>,
    pub last_committed_height: Height,
}
```

### ViewRole

```rust
pub enum ViewRole {
    Leader,   // proposes blocks, collects votes
    Replica,  // votes on proposals
}
```

The role is determined at view entry: if `validator_set.leader_for_view(v).id == self.validator_id`, the node is the leader.

### ViewStep

Tracks progress through the view protocol:

```rust
pub enum ViewStep {
    Entered,             // just entered the view
    WaitingForStatus,    // leader: waiting for replica status messages
    Proposed,            // leader: proposal sent
    WaitingForProposal,  // replica: waiting for leader's proposal
    Voted,               // replica: sent phase-1 vote
    CollectingVotes,     // leader: collecting phase-1 votes
    Prepared,            // leader: QC formed, Prepare sent
    SentVote2,           // replica: sent phase-2 vote
    Done,                // view protocol complete
}
```

## Message Handling

Each `ConsensusMessage` variant is dispatched to a specific handler:

### Propose

```
Propose ──> view_protocol::on_proposal()
         ──> safety check: justify.rank >= locked_qc.rank
         ──> if safe: send VoteMsg to leader
```

The replica validates the block via `Application::validate_block()` before voting.

### VoteMsg (Phase 1)

```
VoteMsg ──> vote_collector::add_vote()
         ──> if quorum reached: on_qc_formed()
         ──> broadcast Prepare{QC}
```

The leader aggregates votes. When 2f+1 are collected, a QC is formed and broadcast in a Prepare message.

### Prepare

```
Prepare ──> view_protocol::on_prepare()
         ──> update locked_qc to the received QC
         ──> send Vote2Msg to next view's leader
```

### Vote2Msg (Phase 2)

```
Vote2Msg ──> vote_collector::add_vote()
          ──> if quorum reached: on_double_cert_formed()
          ──> commit block and ancestors
          ──> advance to next view
```

### Wish

```
Wish ──> pacemaker::add_wish()
      ──> if quorum reached: form TimeoutCertificate
      ──> broadcast TC
      ──> advance view
```

### TimeoutCert

```
TimeoutCert ──> advance to TC's target view
             ──> relay TC to other validators (if not seen before)
```

### StatusCert

```
StatusCert ──> leader collects status from replicas
            ──> when enough received: try_propose()
```

## Vote Collection

The `VoteCollector` manages vote aggregation for both phases:

```rust
pub struct VoteCollector {
    // phase-1 votes: view -> block_hash -> votes
    // phase-2 votes: view -> qc_block_hash -> votes
}
```

When a quorum (2f+1 weighted votes) is reached:
- Phase 1: forms a `QuorumCertificate` with an `AggregateSignature`
- Phase 2: forms a `DoubleCertificate`

The collector prunes stale votes for old views to prevent memory growth.

## Commit Process

When a double certificate is formed:

1. Identify the committed block from the double certificate
2. Walk the chain from the committed block backward to `last_committed_height + 1`
3. For each block in ascending height order:
   - Decode payload into transactions
   - `app.execute_block(txs, ctx)` (where `txs` is `&[&[u8]]` and `ctx` is a `BlockContext` with height, view, proposer, epoch, validator_set; returns `EndBlockResponse` which may contain validator updates and events)
   - `app.on_commit(block, ctx)`
4. Update `last_committed_height`

## Pacemaker Integration

The pacemaker manages view timeouts independently of message processing:

- **Base timeout**: 2 seconds
- **Backoff**: 1.5× per consecutive timeout, capped at 30 seconds
- **Reset**: on any successful view transition (QC formed, commit, etc.)

On timeout, the engine:
1. Builds and broadcasts a `Wish` message
2. Applies exponential backoff to the timer
3. Continues listening for messages (the view is not abandoned until a TC forms)

See [Protocol](protocol.md) for the full pacemaker specification.
