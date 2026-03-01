use std::sync::Mutex;

use ruc::*;
use tracing::info;

use hotmint_consensus::application::Application;
use hotmint_types::Block;
use hotmint_types::context::BlockContext;
use hotmint_types::validator::ValidatorId;
use hotmint_types::validator_update::EndBlockResponse;

use revm::context::TxEnv;
use revm::database::CacheDB;
use revm::database_interface::EmptyDB;
use revm::handler::ExecuteCommitEvm;
use revm::primitives::{Address, TxKind, U256};
use revm::state::AccountInfo;
use revm::{Context, MainBuilder, MainContext};

use crate::tx::EvmTx;

pub const ALICE: [u8; 20] = [0xAA; 20];
pub const BOB: [u8; 20] = [0xBB; 20];

const ETH: u128 = 1_000_000_000_000_000_000; // 1 ETH in wei

/// EVM application backed by revm.
/// Each validator runs its own instance with identical genesis state.
/// Deterministic execution ensures all replicas reach the same state.
pub struct EvmApp {
    pub db: Mutex<CacheDB<EmptyDB>>,
    pub validator_id: ValidatorId,
}

impl EvmApp {
    pub fn new(validator_id: ValidatorId) -> Self {
        let mut db = CacheDB::new(EmptyDB::default());

        // Genesis: Alice and Bob each get 100 ETH
        let alice_addr = Address::new(ALICE);
        let bob_addr = Address::new(BOB);

        db.insert_account_info(
            alice_addr,
            AccountInfo {
                balance: U256::from(100u64) * U256::from(ETH),
                nonce: 0,
                ..Default::default()
            },
        );
        db.insert_account_info(
            bob_addr,
            AccountInfo {
                balance: U256::from(100u64) * U256::from(ETH),
                nonce: 0,
                ..Default::default()
            },
        );

        Self {
            db: Mutex::new(db),
            validator_id,
        }
    }

    fn get_balance(&self, addr: &[u8; 20]) -> U256 {
        let db = self.db.lock().unwrap();
        let address = Address::new(*addr);
        db.cache
            .accounts
            .get(&address)
            .map(|a| a.info.balance)
            .unwrap_or_default()
    }
}

impl Application for EvmApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        // Read Alice's current nonce from the EVM state
        let db = self.db.lock().unwrap();
        let alice_addr = Address::new(ALICE);
        let nonce = db
            .cache
            .accounts
            .get(&alice_addr)
            .map(|a| a.info.nonce)
            .unwrap_or(0);
        drop(db);
        let tx = EvmTx::transfer(ALICE, BOB, ETH, nonce);
        crate::tx::encode_payload(&[tx])
    }

    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        let mut db = self.db.lock().unwrap();

        for tx_bytes in txs {
            let Some(tx) = EvmTx::decode(tx_bytes) else {
                continue;
            };

            let caller = Address::new(tx.from);
            let to = Address::new(tx.to);

            let tx_env = TxEnv {
                caller,
                kind: TxKind::Call(to),
                value: U256::from(tx.value),
                gas_limit: tx.gas_limit,
                nonce: tx.nonce,
                data: tx.data.into(),
                ..Default::default()
            };

            let mut evm = Context::mainnet().with_db(&mut *db).build_mainnet();

            match evm.transact_commit(tx_env) {
                Ok(_result) => {
                    // Transaction executed successfully
                }
                Err(e) => {
                    tracing::warn!(
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
        let alice_bal = self.get_balance(&ALICE);
        let bob_bal = self.get_balance(&BOB);

        // Format as ETH (divide by 10^18)
        let alice_eth = alice_bal / U256::from(ETH);
        let bob_eth = bob_bal / U256::from(ETH);

        info!(
            validator = %self.validator_id,
            height = ctx.height.as_u64(),
            alice_eth = %alice_eth,
            bob_eth = %bob_eth,
            "block committed"
        );
        Ok(())
    }

    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        match path {
            "balance" if data.len() == 20 => {
                let addr: [u8; 20] = data.try_into().unwrap();
                let bal = self.get_balance(&addr);
                Ok(bal.to_be_bytes::<32>().to_vec())
            }
            _ => Ok(vec![]),
        }
    }
}
