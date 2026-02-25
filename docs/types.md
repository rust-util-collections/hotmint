# Core Types

Reference for all data types defined in `hotmint-types`.

## Primitives

### ViewNumber

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ViewNumber(pub u64);
```

Monotonically increasing view number. Corresponds to `v` in the HotStuff-2 paper.

```rust
let view = ViewNumber(0);
let next = ViewNumber(view.as_u64() + 1);
```

### Height

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct Height(pub u64);
```

Block height in the committed chain. Corresponds to `k` in the paper.

### BlockHash

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct BlockHash(pub [u8; 32]);
```

32-byte Blake3 hash of a block.

## Block

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub height: Height,
    pub parent_hash: BlockHash,
    pub hash: BlockHash,
    pub view: ViewNumber,
    pub proposer: ValidatorId,
    pub payload: Vec<u8>,
}
```

Corresponds to `B_k := (b_k, h_{k-1})` in the paper. The `payload` field contains length-prefixed transactions (encoded by `Mempool::collect_payload`).

```rust
// genesis block
let genesis = Block::genesis();
assert_eq!(genesis.height, Height(0));
assert_eq!(genesis.parent_hash, BlockHash([0u8; 32]));
```

## Certificates

### QuorumCertificate (QC)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumCertificate {
    pub block_hash: BlockHash,
    pub view: ViewNumber,
    pub aggregate_signature: AggregateSignature,
}
```

Corresponds to `C_v(B_k)` — an aggregate signature from 2f+1 validators on a block hash. Formed when the leader collects sufficient phase-1 votes.

### DoubleCertificate (DC)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoubleCertificate {
    pub qc: QuorumCertificate,
    pub aggregate_signature: AggregateSignature,
}
```

Corresponds to `C_v(C_v(B_k))` — a QC of a QC. Triggers the commit of the referenced block and all uncommitted ancestors.

### TimeoutCertificate (TC)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutCertificate {
    pub target_view: ViewNumber,
    pub wishes: Vec<(ValidatorId, Option<QuorumCertificate>, Signature)>,
}
```

Corresponds to `TC_v` — proof that 2f+1 validators timed out and wish to advance to `target_view`. Each wish carries the validator's `highest_qc` to help the next leader pick the best chain to extend.

## Vote

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub block_hash: BlockHash,
    pub view: ViewNumber,
    pub validator: ValidatorId,
    pub signature: Signature,
    pub vote_type: VoteType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoteType {
    Vote,   // phase-1 vote (on a block proposal)
    Vote2,  // phase-2 vote (on a QC / Prepare message)
}
```

## Validators

### ValidatorId

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ValidatorId(pub u64);
```

### ValidatorInfo

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    pub id: ValidatorId,
    pub public_key: PublicKey,
    pub power: u64,
}
```

### ValidatorSet

```rust
let vs = ValidatorSet::new(vec![
    ValidatorInfo { id: ValidatorId(0), public_key: pk0, power: 1 },
    ValidatorInfo { id: ValidatorId(1), public_key: pk1, power: 1 },
    ValidatorInfo { id: ValidatorId(2), public_key: pk2, power: 1 },
    ValidatorInfo { id: ValidatorId(3), public_key: pk3, power: 1 },
]);

assert_eq!(vs.validator_count(), 4);
assert_eq!(vs.quorum_threshold(), 3);    // ceil(2*4/3)
assert_eq!(vs.max_faulty_power(), 1);    // floor((4-1)/3)

// round-robin leader election
let leader = vs.leader_for_view(ViewNumber(5)); // validators[5 % 4] = validators[1]

// O(1) lookups
let idx = vs.index_of(ValidatorId(2));    // Some(2)
let info = vs.get(ValidatorId(2));        // Some(&ValidatorInfo)
let power = vs.power_of(ValidatorId(2));  // 1
```

After deserialization (e.g., from persistent storage), call `rebuild_index()` to reconstruct the O(1) lookup map:

```rust
let mut vs: ValidatorSet = serde_json::from_str(&json)?;
vs.rebuild_index();
```

## Epochs

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct EpochNumber(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Epoch {
    pub number: EpochNumber,
    pub validator_set: ValidatorSet,
}
```

An epoch defines a validator set configuration. Epoch transitions happen at predefined block heights and allow adding/removing validators or changing voting power.

## Cryptographic Primitives

### Signature

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature(pub Vec<u8>);
```

### PublicKey

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PublicKey(pub Vec<u8>);
```

### AggregateSignature

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateSignature {
    pub signers: Vec<bool>,        // bitfield: signers[i] == true if validator i signed
    pub signatures: Vec<Signature>,
}
```

```rust
let mut agg = AggregateSignature::new(4); // 4 validators
agg.add(0, sig_from_validator_0)?;
agg.add(2, sig_from_validator_2)?;
agg.add(3, sig_from_validator_3)?;
assert_eq!(agg.count(), 3); // 3 of 4 signed
```

## ConsensusMessage (Wire Protocol)

The `ConsensusMessage` enum defines all message types exchanged between validators:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsensusMessage {
    // Leader -> All: block proposal with justify QC
    Propose {
        block: Block,
        justify: QuorumCertificate,
        double_cert: Option<DoubleCertificate>,
        signature: Signature,
    },

    // Replica -> Leader: phase-1 vote on a proposed block
    VoteMsg(Vote),

    // Leader -> All: QC formed, update your lock
    Prepare {
        certificate: QuorumCertificate,
        signature: Signature,
    },

    // Replica -> Next Leader: phase-2 vote on the QC
    Vote2Msg(Vote),

    // Any -> All: timeout, requesting view change
    Wish {
        target_view: ViewNumber,
        validator: ValidatorId,
        highest_qc: Option<QuorumCertificate>,
        signature: Signature,
    },

    // Any -> All: aggregated timeout proof
    TimeoutCert(TimeoutCertificate),

    // Replica -> Leader: status report at view entry
    StatusCert {
        locked_qc: Option<QuorumCertificate>,
        validator: ValidatorId,
        signature: Signature,
    },
}
```

All messages are serialized with CBOR (`serde_cbor_2`) for network transport.
