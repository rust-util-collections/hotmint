mod app;
mod tx;

pub use app::{EvmApplication, EvmConfig, GenesisAccount};
pub use tx::{EvmTx, encode_payload};

/// 1 ETH in wei.
pub const ETH: u128 = 1_000_000_000_000_000_000;

// Re-export revm types for convenience
pub use revm::primitives::{Address, U256};
pub use revm::state::AccountInfo;
