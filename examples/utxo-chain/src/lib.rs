pub mod app;
pub mod utxo_app;
pub mod utxo_state;
pub mod utxo_types;

pub use utxo_app::{GenesisUtxo, UtxoApplication, UtxoConfig};
pub use utxo_types::{OutPoint, TxInput, TxOutput, UtxoTx, encode_payload, hash_pubkey};

/// 1 satoshi (smallest unit).
pub const SATOSHI: u64 = 1;

/// 1 coin = 10^8 satoshi.
pub const COIN: u64 = 100_000_000;
