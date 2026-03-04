//! # hotmint-evm
//!
//! EVM execution toolkit for building EVM-compatible chains on hotmint.
//!
//! **WARNING: This crate uses simplified unsigned transactions (no ECDSA
//! signature verification). It is intended for development, testing, and
//! demos. Production EVM chains MUST add transaction signature verification
//! (e.g., secp256k1 ECDSA recovery) before deployment.**

mod app;
mod tx;

pub use app::{EvmApplication, EvmConfig, GenesisAccount};
pub use tx::{EvmTx, encode_payload};

/// 1 ETH in wei.
pub const ETH: u128 = 1_000_000_000_000_000_000;

// Re-export revm types for convenience
pub use revm::primitives::{Address, U256};
pub use revm::state::AccountInfo;
