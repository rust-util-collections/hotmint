# hotmint-crypto

[![crates.io](https://img.shields.io/crates/v/hotmint-crypto.svg)](https://crates.io/crates/hotmint-crypto)
[![docs.rs](https://docs.rs/hotmint-crypto/badge.svg)](https://docs.rs/hotmint-crypto)

Cryptographic implementations for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Provides concrete implementations of the `Signer` and `Verifier` traits defined in `hotmint-types`, using Ed25519 for digital signatures and Blake3 for block hashing.

## Components

| Component | Description |
|:----------|:------------|
| `Ed25519Signer` | Implements `Signer` using ed25519-dalek |
| `Ed25519Verifier` | Implements `Verifier` for single and aggregate signature verification |
| `compute_block_hash()` | Blake3 hashing of block fields |

## Usage

```rust
use hotmint_types::{Signer, ValidatorId};
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};

// Generate a random keypair
let signer = Ed25519Signer::generate(ValidatorId(0));

// Sign a message
let sig = signer.sign(b"hello");
let pk = signer.public_key();

// Verify
let verifier = Ed25519Verifier;
assert!(verifier.verify(&pk, b"hello", &sig));
```

### Construct from existing key

```rust
use ed25519_dalek::SigningKey;

let signing_key = SigningKey::from_bytes(&secret_key_bytes);
let signer = Ed25519Signer::new(signing_key, ValidatorId(0));
```

### Block hashing

```rust
use hotmint_crypto::compute_block_hash;
use hotmint_types::{Block, BlockHash};

let hash = compute_block_hash(&block);

// Or use the convenience method on Block:
let hash = block.compute_hash();
```

## License

GPL-3.0-only
