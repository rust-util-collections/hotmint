# Protocol

Hotmint implements the core protocol from the HotStuff-2 paper ([arXiv:2301.03253](https://arxiv.org/abs/2301.03253)), combining a two-chain commit rule with a simplified view-change mechanism.

## Two-Chain Commit Rule

Classical BFT protocols (PBFT, Tendermint) require three communication phases to commit a block. HotStuff-2 reduces this to two chains:

```
B_{k-1}  <──  C_v(B_{k-1})  <──  C_v(C_v(B_{k-1}))
  Block         QC                  Double Cert ──> commit
```

1. A block `B_k` is proposed by the leader.
2. Replicas vote on `B_k`. When 2f+1 votes are collected, they form a **Quorum Certificate (QC)** `C_v(B_k)`.
3. Replicas vote again on the QC. When 2f+1 votes are collected, they form a **Double Certificate (DC)** `C_v(C_v(B_k))`.
4. The double certificate triggers **commit** — the block and all its uncommitted ancestors are committed in height order.

This two-chain structure yields lower confirmation latency compared to Tendermint's three-phase voting while maintaining the same safety guarantees (tolerance of up to f = ⌊(n-1)/3⌋ Byzantine faults).

## Steady-State View Protocol

Each view v follows a 5-step protocol (corresponding to Figure 1 of the HotStuff-2 paper):

### Step 1: Enter View

All validators enter the view. The **leader** for this view (determined by round-robin: `v mod n`) either:
- Waits to collect status messages from replicas, or
- Proposes directly if it already holds the highest QC.

**Replicas** send a `StatusCert` to the leader, carrying their `locked_qc`.

### Step 2: Propose

The leader broadcasts a proposal:

```
<Propose, B_k, v, C_{v'}(B_{k-1})>
```

Where:
- `B_k` is the new block (height k, containing the payload)
- `C_{v'}(B_{k-1})` is the **justify QC** — the highest QC the leader knows of, proving the parent block received sufficient votes

### Step 3: Vote & Commit Check

Each replica performs the **safety check** before voting:

```
justify.rank >= locked_qc.rank
```

If the justify QC's rank (view number) is at least as high as the replica's locked QC, the proposal is safe to vote for. The replica sends a `VoteMsg` to the current leader.

Additionally, the replica checks if the received justify QC completes a two-chain (forming a double certificate) and triggers commit if so.

### Step 4: Prepare

The leader collects 2f+1 `VoteMsg` messages and aggregates them into a QC `C_v(B_k)`. It then broadcasts:

```
<Prepare, C_v(B_k)>
```

### Step 5: Vote2

Each replica receives the Prepare message and:
1. **Updates its lock** to `C_v(B_k)` (the newly formed QC)
2. Sends a `Vote2Msg` to the **next view's leader** (validator for view v+1)

When the next leader collects 2f+1 `Vote2Msg` messages, it forms the double certificate, completing the two-chain and triggering commit.

## Safety Rules

### Locking Rule

A replica updates its `locked_qc` upon receiving a valid Prepare message. The lock represents the highest QC the replica has seen and accepted. When voting on a new proposal, the replica requires:

```
proposal.justify.rank >= self.locked_qc.rank
```

This ensures that a replica never votes for a proposal that conflicts with a block it has already locked on, unless a higher-ranked QC proves that the network has moved past that lock.

### Commit Rule

A double certificate `C_v(C_v(B_k))` triggers commit. When committed:

1. Block `B_k` and all uncommitted ancestors are committed in ascending height order.
2. For each committed block, the application lifecycle is invoked: `begin_block` → `deliver_tx` (×N) → `end_block` → `on_commit`.
3. The `last_committed_height` is advanced accordingly.

## View Change / Pacemaker

The pacemaker ensures liveness by detecting stalled views and advancing to the next view.

### Timeout and Wish

```
enter_view(v) ──> start timer (base_timeout = 2s)
timer expires ──> broadcast Wish{target_view: v+1, highest_qc}
```

When a validator's view timer expires without completing the view protocol, it broadcasts a `Wish` message indicating it wants to move to the next view. The wish carries the validator's `highest_qc` to help the next leader build on the best known chain.

### Timeout Certificate (TC)

When 2f+1 validators broadcast wishes for the same target view, their wishes are aggregated into a **Timeout Certificate (TC)**:

```
TC_v = {target_view, wishes: [(validator, highest_qc, signature), ...]}
```

The TC proves that a supermajority agreed to abandon the current view. Upon receiving a TC, all validators advance to the target view.

### Exponential Backoff

To prevent timeout storms, the pacemaker applies exponential backoff:

```
timeout(k) = min(base_timeout × 1.5^k, 30s)
```

Where `k` is the number of consecutive timeouts. On successful progress (view completion), the backoff resets to the base timeout.

### TC Relay

When a validator receives a TC it hasn't seen before, it rebroadcasts the TC to ensure all validators can advance even if the original broadcast didn't reach everyone. Deduplication prevents infinite relay loops.

### View Advancement

A validator advances to a new view when any of the following occurs:
- A **Double Certificate** is formed (successful commit)
- A **Timeout Certificate** is received for a higher view
- The validator completes the Vote2 step for the current view

## Epochs

Validators are organized into **epochs**. An epoch defines a fixed validator set and configuration. Epoch boundaries are the mechanism for validator set changes (adding/removing validators, changing voting power).

```rust
struct Epoch {
    pub number: EpochNumber,
    pub validator_set: ValidatorSet,
}
```

Epoch transitions are coordinated through the consensus protocol itself, ensuring all honest validators agree on when and how the validator set changes.

## Quorum Arithmetic

Given n total validators with equal voting power:

| Parameter | Formula | Example (n=4) |
|:----------|:--------|:--------------|
| Max Byzantine faults f | ⌊(n-1)/3⌋ | 1 |
| Quorum threshold | ⌈2n/3⌉ | 3 |
| Safety guarantee | Any two quorums overlap in ≥1 honest validator | — |

Leader election is round-robin: `leader(v) = validators[v % n]`.
