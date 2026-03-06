# Cryptography

Hotmint separates cryptographic abstractions (traits in `hotmint-types`) from concrete implementations (in `hotmint-crypto`). This allows swapping signature schemes without modifying the consensus engine.

## Traits

### Signer

```rust
pub trait Signer: Send + Sync {
    fn sign(&self, message: &[u8]) -> Signature;
    fn public_key(&self) -> PublicKey;
    fn validator_id(&self) -> ValidatorId;
}
```

The consensus engine calls `Signer::sign()` to sign proposals, votes, wishes, and status messages. Each validator has a single `Signer` instance.

### Verifier

```rust
pub trait Verifier: Send + Sync {
    fn verify(&self, pk: &PublicKey, msg: &[u8], sig: &Signature) -> bool;
    fn verify_aggregate(&self, vs: &ValidatorSet, msg: &[u8], agg: &AggregateSignature) -> bool;
}
```

`verify_aggregate` checks an `AggregateSignature` against the validator set — it iterates over the bitfield, retrieves each signer's public key from the `ValidatorSet`, and verifies their individual signature.

## Ed25519 Implementation

### Ed25519Signer

```rust
use hotmint::crypto::Ed25519Signer;

// generate a random keypair
let signer = Ed25519Signer::generate(ValidatorId(0));

// or construct from an existing ed25519-dalek SigningKey
use ed25519_dalek::SigningKey;
let signing_key = SigningKey::from_bytes(&secret_key_bytes);
let signer = Ed25519Signer::new(signing_key, ValidatorId(0));

// sign a message
let sig = signer.sign(b"hello");

// get the public key (for building ValidatorInfo)
let pk = signer.public_key();

// get the ed25519-dalek VerifyingKey (for direct verification)
let vk = signer.verifying_key();
```

### Ed25519Verifier

```rust
use hotmint::crypto::Ed25519Verifier;

let verifier = Ed25519Verifier;

// verify a single signature
let valid = verifier.verify(&pk, b"hello", &sig);

// verify an aggregate signature against the validator set
let valid = verifier.verify_aggregate(&validator_set, b"block_hash", &aggregate_sig);
```

## Aggregate Signatures

Hotmint uses a simple aggregate signature scheme: a bitfield indicating which validators signed, plus a list of their individual signatures.

```rust
use hotmint::prelude::AggregateSignature;

// create for a 4-validator set
let mut agg = AggregateSignature::new(4);

// add signatures as votes arrive
agg.add(0, sig_0)?;  // validator 0 signed
agg.add(2, sig_2)?;  // validator 2 signed
agg.add(3, sig_3)?;  // validator 3 signed

assert_eq!(agg.count(), 3);
assert!(agg.signers[0]);   // true
assert!(!agg.signers[1]);  // false — validator 1 didn't sign
assert!(agg.signers[2]);   // true
assert!(agg.signers[3]);   // true
```

This scheme is straightforward and correct but has O(n) signature size. For production networks with many validators, consider implementing the `Signer`/`Verifier` traits with BLS threshold signatures.

## Block Hashing

```rust
use hotmint::crypto::compute_block_hash;

let hash = compute_block_hash(&block);
// returns BlockHash — a 32-byte Blake3 hash

// Or use the convenience method directly on Block:
let hash = block.compute_hash();
```

The hash covers all block fields except the hash itself: `height || parent_hash || view || proposer || payload`.

## Implementing a Custom Signer

To use a different signature scheme (e.g., BLS, ECDSA), implement the `Signer` and `Verifier` traits:

```rust
use hotmint::prelude::*;

struct MySigner {
    secret_key: MySecretKey,
    validator_id: ValidatorId,
}

impl Signer for MySigner {
    fn sign(&self, message: &[u8]) -> Signature {
        let sig_bytes = self.secret_key.sign(message);
        Signature(sig_bytes.to_vec())
    }

    fn public_key(&self) -> PublicKey {
        PublicKey(self.secret_key.public_key().to_bytes().to_vec())
    }

    fn validator_id(&self) -> ValidatorId {
        self.validator_id
    }
}

struct MyVerifier;

impl Verifier for MyVerifier {
    fn verify(&self, pk: &PublicKey, msg: &[u8], sig: &Signature) -> bool {
        let public_key = MyPublicKey::from_bytes(&pk.0);
        let signature = MySignature::from_bytes(&sig.0);
        public_key.verify(msg, &signature).is_ok()
    }

    fn verify_aggregate(
        &self,
        vs: &ValidatorSet,
        msg: &[u8],
        agg: &AggregateSignature,
    ) -> bool {
        let mut sig_idx = 0;
        for (i, signed) in agg.signers.iter().enumerate() {
            if *signed {
                let info = &vs.validators()[i];
                if !self.verify(&info.public_key, msg, &agg.signatures[sig_idx]) {
                    return false;
                }
                sig_idx += 1;
            }
        }
        true
    }
}
```

Then pass `Box::new(MySigner { ... })` to `ConsensusEngine::new()`.
