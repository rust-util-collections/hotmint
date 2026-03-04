use ruc::*;

use hotmint_consensus::application::Application;
use hotmint_types::Block;
use hotmint_types::context::BlockContext;
use hotmint_types::validator_update::EndBlockResponse;

pub use hotmint_evm::{Address, ETH, EvmApplication, EvmConfig, EvmTx, GenesisAccount, U256};

pub const ALICE: [u8; 20] = [0xAA; 20];
pub const BOB: [u8; 20] = [0xBB; 20];

/// Demo wrapper that adds auto-transfer payload generation on top of EvmApplication.
pub struct DemoEvmApp {
    inner: EvmApplication,
}

impl Default for DemoEvmApp {
    fn default() -> Self {
        Self::new()
    }
}

impl DemoEvmApp {
    pub fn new() -> Self {
        let config = EvmConfig {
            genesis_allocs: vec![
                GenesisAccount::funded(ALICE, U256::from(100u64) * U256::from(ETH)),
                GenesisAccount::funded(BOB, U256::from(100u64) * U256::from(ETH)),
            ],
            log_on_commit: true,
            ..Default::default()
        };
        Self {
            inner: EvmApplication::new(config),
        }
    }
}

impl Application for DemoEvmApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        let nonce = self.inner.get_nonce(&ALICE);
        let tx = EvmTx::transfer(ALICE, BOB, ETH, nonce);
        hotmint_evm::encode_payload(&[tx])
    }

    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        self.inner.execute_block(txs, ctx)
    }

    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()> {
        self.inner.on_commit(block, ctx)
    }

    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        self.inner.query(path, data)
    }
}
