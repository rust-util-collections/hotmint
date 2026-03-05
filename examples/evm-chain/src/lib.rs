pub mod app;
pub mod evm_app;
pub mod evm_tx;

/// 1 ETH in wei.
pub const ETH: u128 = 1_000_000_000_000_000_000;

pub use evm_app::{EvmApplication, EvmConfig, GenesisAccount};
pub use evm_tx::{EvmTx, encode_payload};
pub use revm::primitives::{Address, U256};
pub use revm::state::AccountInfo;
