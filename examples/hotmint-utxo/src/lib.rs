//! # hotmint-utxo
//!
//! Production-grade UTXO runtime toolkit for building Bitcoin-style chains
//! on hotmint.
//!
//! Features:
//! - ed25519 signature verification (via hotmint-crypto)
//! - Persistent UTXO set with SMT proofs (via vsdb `VerMapWithProof`)
//! - Address-indexed UTXO queries with pagination (via vsdb `SlotDex`)
//! - Full transaction validation (double-spend, ownership, amounts)
//!
//! **Prerequisite:** Call `vsdb::vsdb_set_base_dir()` before constructing
//! [`UtxoApplication`].

mod app;
mod state;
mod types;

pub use app::{GenesisUtxo, UtxoApplication, UtxoConfig};
pub use types::{OutPoint, TxInput, TxOutput, UtxoTx, encode_payload, hash_pubkey};

/// 1 satoshi (smallest unit).
pub const SATOSHI: u64 = 1;

/// 1 coin = 10^8 satoshi.
pub const COIN: u64 = 100_000_000;
