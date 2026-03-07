use std::sync::Mutex;

use ruc::*;
use tracing::{info, warn};
use vsdb::MptCalc;

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

use crate::evm_tx::EvmTx;

/// Encode account state as `nonce(8) || balance(32) || code_hash(32)` for trie insertion.
fn encode_account_state(info: &AccountInfo) -> Vec<u8> {
    let mut buf = Vec::with_capacity(72);
    buf.extend_from_slice(&info.nonce.to_be_bytes());
    buf.extend_from_slice(&info.balance.to_be_bytes::<32>());
    buf.extend_from_slice(info.code_hash.as_slice());
    buf
}

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
/// **WARNING: This uses simplified unsigned transactions (no ECDSA
/// signature verification). Production EVM chains MUST add transaction
/// signature verification (e.g., secp256k1 ECDSA recovery).**
pub struct EvmApplication {
    db: Mutex<CacheDB<EmptyDB>>,
    state_trie: Mutex<MptCalc>,
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

        // Initialize state trie with genesis accounts
        let mut state_trie = MptCalc::new();
        for alloc in &config.genesis_allocs {
            let addr = Address::new(alloc.address);
            if let Some(acct) = db.cache.accounts.get(&addr) {
                let _ = state_trie.insert(addr.as_slice(), &encode_account_state(&acct.info));
            }
        }

        Self {
            db: Mutex::new(db),
            state_trie: Mutex::new(state_trie),
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
                Ok(result) => {
                    gas_used = gas_used.saturating_add(result.gas_used());
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

        // --- Trie maintenance ---

        // 1. Build per-block Transactions Trie
        let mut tx_trie = MptCalc::new();
        let tx_ops: Vec<_> = txs
            .iter()
            .enumerate()
            .map(|(i, tx_bytes)| (i.to_be_bytes().to_vec(), *tx_bytes))
            .collect();
        let tx_refs: Vec<(&[u8], Option<&[u8]>)> = tx_ops
            .iter()
            .map(|(k, v)| (k.as_slice(), Some(*v)))
            .collect();
        let _ = tx_trie.batch_update(&tx_refs);
        let _tx_root = tx_trie.root_hash().unwrap_or_default();

        // 2. Update persistent State Trie with all cached accounts
        let mut state_trie = self.state_trie.lock().unwrap_or_else(|e| e.into_inner());
        let state_ops: Vec<_> = db
            .cache
            .accounts
            .iter()
            .map(|(addr, acct)| (addr.as_slice().to_vec(), encode_account_state(&acct.info)))
            .collect();
        let state_refs: Vec<(&[u8], Option<&[u8]>)> = state_ops
            .iter()
            .map(|(k, v)| (k.as_slice(), Some(v.as_slice())))
            .collect();
        let _ = state_trie.batch_update(&state_refs);
        let _state_root = state_trie.root_hash().unwrap_or_default();

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
