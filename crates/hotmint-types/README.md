# hotmint-types

[![crates.io](https://img.shields.io/crates/v/hotmint-types.svg)](https://crates.io/crates/hotmint-types)
[![docs.rs](https://docs.rs/hotmint-types/badge.svg)](https://docs.rs/hotmint-types)

Core data types for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

This crate defines all shared primitives used across the Hotmint ecosystem with minimal dependencies (only `serde` and `ruc`). It is the foundation that every other Hotmint crate depends on.

## Types

| Type | Description |
|:-----|:------------|
| `Block`, `BlockHash`, `Height` | Chain primitives — block structure, 32-byte Blake3 hash, block height |
| `ViewNumber` | Monotonically increasing consensus view number |
| `Vote`, `VoteType` | Phase-1 and phase-2 voting messages |
| `QuorumCertificate` | Aggregate proof from 2f+1 validators on a block |
| `DoubleCertificate` | QC-of-QC that triggers commit (two-chain rule) |
| `TimeoutCertificate` | Aggregated timeout proof for view change |
| `ConsensusMessage` | Wire protocol enum covering all message types |
| `ValidatorId`, `ValidatorInfo`, `ValidatorSet` | Validator identity, metadata, and set management |
| `Signature`, `PublicKey`, `AggregateSignature` | Cryptographic primitives |
| `Signer`, `Verifier` | Abstract traits for pluggable signature schemes |
| `Epoch`, `EpochNumber` | Epoch management for validator set transitions |

## Usage

```rust
use hotmint_types::*;

// Create a validator set
let vs = ValidatorSet::new(vec![
    ValidatorInfo { id: ValidatorId(0), public_key: pk0, power: 1 },
    ValidatorInfo { id: ValidatorId(1), public_key: pk1, power: 1 },
    ValidatorInfo { id: ValidatorId(2), public_key: pk2, power: 1 },
    ValidatorInfo { id: ValidatorId(3), public_key: pk3, power: 1 },
]);

assert_eq!(vs.quorum_threshold(), 3);    // ceil(2*4/3)
assert_eq!(vs.max_faulty_power(), 1);    // floor((4-1)/3)

// Round-robin leader election
let leader = vs.leader_for_view(ViewNumber(5)).unwrap();

// Aggregate signatures
let mut agg = AggregateSignature::new(4);
agg.add(0, sig_0).unwrap();
agg.add(2, sig_2).unwrap();
assert_eq!(agg.count(), 2);
```

## Implementing Custom Signers

```rust
use hotmint_types::{Signer, Verifier, Signature, PublicKey, ValidatorId};

struct MySigner { /* ... */ }

impl Signer for MySigner {
    fn sign(&self, message: &[u8]) -> Signature { /* ... */ }
    fn public_key(&self) -> PublicKey { /* ... */ }
    fn validator_id(&self) -> ValidatorId { /* ... */ }
}
```

## License

GPL-3.0-only
