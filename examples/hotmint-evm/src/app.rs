use std::sync::Mutex;

use ruc::*;
use tracing::{info, warn};

use hotmint_consensus::application::Application;
use hotmint_types::Block;
use hotmint_types::context::BlockContext;
use hotmint_types::validator_update::EndBlockResponse;

use revm::context::TxEnv;
use revm::database::CacheDB;
use revm::database_interface::EmptyDB;
use revm::handler::ExecuteCommitEvm;
use revm::primitives::{Address, TxKind, U256};
use revm::state::AccountInfo;
use revm::{Context, MainBuilder, MainContext};

use crate::tx::EvmTx;

/// Genesis account allocation.
#[derive(Debug, Clone)]
pub struct GenesisAccount {
    pub address: [u8; 20],
    pub balance: U256,
    pub nonce: u64,
    pub code: Vec<u8>,
}

impl GenesisAccount {
    /// Create a genesis account with balance only (no code).
    pub fn funded(address: [u8; 20], balance: U256) -> Self {
        Self {
            address,
            balance,
            nonce: 0,
            code: vec![],
        }
    }
}

/// EVM chain configuration.
#[derive(Debug, Clone)]
pub struct EvmConfig {
    /// Chain ID (for EIP-155 replay protection).
    pub chain_id: u64,
    /// Block gas limit.
    pub block_gas_limit: u64,
    /// Genesis account allocations.
    pub genesis_allocs: Vec<GenesisAccount>,
    /// Whether to log balance info on commit (useful for demos).
    pub log_on_commit: bool,
}

impl Default for EvmConfig {
    fn default() -> Self {
        Self {
            chain_id: 1337,
            block_gas_limit: 30_000_000,
            genesis_allocs: vec![],
            log_on_commit: false,
        }
    }
}

/// Ready-to-use EVM application that implements [`Application`].
///
/// Drop this into a hotmint consensus engine to get a fully functional
/// EVM-compatible chain. Transactions are decoded from CBOR-encoded
/// [`EvmTx`] structs in the block payload.
///
/// # Example
///
/// ```ignore
/// use hotmint_evm::*;
///
/// let config = EvmConfig {
///     genesis_allocs: vec![
///         GenesisAccount::funded([0xAA; 20], U256::from(100) * U256::from(ETH)),
///     ],
///     log_on_commit: true,
///     ..Default::default()
/// };
/// let app = EvmApplication::new(config);
/// // Pass Box::new(app) to ConsensusEngine::new()
/// ```
pub struct EvmApplication {
    db: Mutex<CacheDB<EmptyDB>>,
    config: EvmConfig,
}

impl EvmApplication {
    /// Create a new EVM application with the given configuration.
    /// Genesis accounts are initialized in the state database.
    pub fn new(config: EvmConfig) -> Self {
        let mut db = CacheDB::new(EmptyDB::default());

        for alloc in &config.genesis_allocs {
            let addr = Address::new(alloc.address);
            let mut info = AccountInfo {
                balance: alloc.balance,
                nonce: alloc.nonce,
                ..Default::default()
            };
            if !alloc.code.is_empty() {
                info.code = Some(revm::bytecode::Bytecode::new_raw(
                    revm::primitives::Bytes::copy_from_slice(&alloc.code),
                ));
            }
            db.insert_account_info(addr, info);
        }

        Self {
            db: Mutex::new(db),
            config,
        }
    }

    /// Query an account's balance.
    pub fn get_balance(&self, addr: &[u8; 20]) -> U256 {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        db.cache
            .accounts
            .get(&Address::new(*addr))
            .map(|a| a.info.balance)
            .unwrap_or_default()
    }

    /// Query an account's nonce.
    pub fn get_nonce(&self, addr: &[u8; 20]) -> u64 {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        db.cache
            .accounts
            .get(&Address::new(*addr))
            .map(|a| a.info.nonce)
            .unwrap_or(0)
    }
}

impl Application for EvmApplication {
    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        let mut db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let mut gas_used: u64 = 0;

        for tx_bytes in txs {
            let Some(tx) = EvmTx::decode(tx_bytes) else {
                continue;
            };

            // Enforce block gas limit
            if gas_used.saturating_add(tx.gas_limit) > self.config.block_gas_limit {
                warn!(
                    height = ctx.height.as_u64(),
                    gas_used,
                    tx_gas = tx.gas_limit,
                    limit = self.config.block_gas_limit,
                    "block gas limit exceeded, skipping remaining txs"
                );
                break;
            }

            let tx_env = TxEnv {
                caller: Address::new(tx.from),
                kind: TxKind::Call(Address::new(tx.to)),
                value: U256::from(tx.value),
                gas_limit: tx.gas_limit,
                nonce: tx.nonce,
                data: tx.data.into(),
                ..Default::default()
            };

            let mut evm = Context::mainnet().with_db(&mut *db).build_mainnet();

            match evm.transact_commit(tx_env) {
                Ok(_) => {
                    gas_used = gas_used.saturating_add(tx.gas_limit);
                }
                Err(e) => {
                    warn!(
                        height = ctx.height.as_u64(),
                        error = ?e,
                        "EVM tx execution failed"
                    );
                }
            }
        }

        Ok(EndBlockResponse::default())
    }

    fn on_commit(&self, _block: &Block, ctx: &BlockContext) -> Result<()> {
        if self.config.log_on_commit {
            for alloc in &self.config.genesis_allocs {
                let bal = self.get_balance(&alloc.address);
                let addr = Address::new(alloc.address);
                info!(
                    height = ctx.height.as_u64(),
                    address = %addr,
                    balance = %bal,
                    "committed"
                );
            }
        }
        Ok(())
    }

    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        match path {
            "balance" if data.len() == 20 => {
                let addr: [u8; 20] = data.try_into().unwrap();
                let bal = self.get_balance(&addr);
                Ok(bal.to_be_bytes::<32>().to_vec())
            }
            "nonce" if data.len() == 20 => {
                let addr: [u8; 20] = data.try_into().unwrap();
                let nonce = self.get_nonce(&addr);
                Ok(nonce.to_le_bytes().to_vec())
            }
            _ => Ok(vec![]),
        }
    }
}
