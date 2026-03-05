use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use ruc::*;

use hotmint_consensus::application::Application;
use hotmint_consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::pacemaker::PacemakerConfig;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint_types::context::BlockContext;
use hotmint_types::validator_update::EndBlockResponse;
use hotmint_types::*;
use tokio::sync::mpsc;

use evm_chain::app::{ALICE, BOB};
use evm_chain::{EvmTx, encode_payload};

use revm::context::TxEnv;
use revm::database::CacheDB;
use revm::database_interface::EmptyDB;
use revm::handler::ExecuteCommitEvm;
use revm::primitives::{Address, TxKind, U256};
use revm::state::AccountInfo;
use revm::{Context, MainBuilder, MainContext};

const NUM_VALIDATORS: u64 = 4;
const DURATION_SECS: u64 = 10;
const TXS_PER_BLOCK: usize = 10;
const ETH: u128 = 1_000_000_000_000_000_000;

/// EVM benchmark app: generates N transfers per block, executes via revm.
struct EvmBenchApp {
    db: Mutex<CacheDB<EmptyDB>>,
    commit_count: Arc<AtomicU64>,
    tx_count: Arc<AtomicU64>,
}

impl EvmBenchApp {
    fn new(commit_count: Arc<AtomicU64>, tx_count: Arc<AtomicU64>) -> Self {
        let mut db = CacheDB::new(EmptyDB::default());
        let alice = Address::new(ALICE);
        let bob = Address::new(BOB);
        db.insert_account_info(
            alice,
            AccountInfo {
                balance: U256::from(1_000_000u64) * U256::from(ETH),
                nonce: 0,
                ..Default::default()
            },
        );
        db.insert_account_info(
            bob,
            AccountInfo {
                balance: U256::from(1_000_000u64) * U256::from(ETH),
                nonce: 0,
                ..Default::default()
            },
        );
        Self {
            db: Mutex::new(db),
            commit_count,
            tx_count,
        }
    }
}

impl Application for EvmBenchApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        let db = self.db.lock().unwrap();
        let nonce = db
            .cache
            .accounts
            .get(&Address::new(ALICE))
            .map(|a| a.info.nonce)
            .unwrap_or(0);
        drop(db);

        let txs: Vec<EvmTx> = (0..TXS_PER_BLOCK)
            .map(|i| EvmTx::transfer(ALICE, BOB, ETH / 100, nonce + i as u64))
            .collect();
        encode_payload(&txs)
    }

    fn execute_block(&self, txs: &[&[u8]], _ctx: &BlockContext) -> Result<EndBlockResponse> {
        let mut db = self.db.lock().unwrap();
        let mut executed = 0u64;

        for tx_bytes in txs {
            let Some(etx) = EvmTx::decode(tx_bytes) else {
                continue;
            };
            let tx_env = TxEnv {
                caller: Address::new(etx.from),
                kind: TxKind::Call(Address::new(etx.to)),
                value: U256::from(etx.value),
                gas_limit: etx.gas_limit,
                nonce: etx.nonce,
                data: etx.data.into(),
                ..Default::default()
            };
            let mut evm = Context::mainnet().with_db(&mut *db).build_mainnet();
            if evm.transact_commit(tx_env).is_ok() {
                executed += 1;
            }
        }

        self.tx_count.fetch_add(executed, Ordering::Relaxed);
        Ok(EndBlockResponse::default())
    }

    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        self.commit_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

async fn run_bench(label: &str, base_timeout_ms: u64) {
    let signers: Vec<Ed25519Signer> = (0..NUM_VALIDATORS)
        .map(|i| Ed25519Signer::generate(ValidatorId(i)))
        .collect();

    let validator_infos: Vec<ValidatorInfo> = signers
        .iter()
        .map(|s| ValidatorInfo {
            id: Signer::validator_id(s),
            public_key: Signer::public_key(s),
            power: 1,
        })
        .collect();
    let validator_set = ValidatorSet::new(validator_infos);

    let mut receivers = HashMap::new();
    let mut all_senders: HashMap<ValidatorId, mpsc::Sender<(ValidatorId, ConsensusMessage)>> =
        HashMap::new();

    for i in 0..NUM_VALIDATORS {
        let (tx, rx) = mpsc::channel(8192);
        receivers.insert(ValidatorId(i), rx);
        all_senders.insert(ValidatorId(i), tx);
    }

    let total_txs = Arc::new(AtomicU64::new(0));
    let mut commit_counters = Vec::new();
    let mut handles = Vec::new();

    for (i, signer) in signers.into_iter().enumerate() {
        let vid = ValidatorId(i as u64);
        let rx = receivers.remove(&vid).unwrap();
        let senders: Vec<_> = all_senders
            .iter()
            .map(|(&id, tx)| (id, tx.clone()))
            .collect();

        let commit_count = Arc::new(AtomicU64::new(0));
        commit_counters.push(commit_count.clone());

        let network = ChannelNetwork::new(vid, senders);
        let store: Arc<RwLock<Box<dyn hotmint_consensus::store::BlockStore>>> =
            Arc::new(RwLock::new(Box::new(MemoryBlockStore::new())));

        let app = EvmBenchApp::new(commit_count.clone(), total_txs.clone());
        let state = ConsensusState::new(vid, validator_set.clone());

        let engine = ConsensusEngine::new(
            state,
            store,
            Box::new(network),
            Box::new(app),
            Box::new(signer),
            rx,
            EngineConfig {
                verifier: Box::new(Ed25519Verifier),
                pacemaker: Some(PacemakerConfig {
                    base_timeout_ms,
                    max_timeout_ms: base_timeout_ms * 15,
                    backoff_multiplier: 1.5,
                }),
                persistence: None,
            },
        );

        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    let start = Instant::now();
    tokio::time::sleep(tokio::time::Duration::from_secs(DURATION_SECS)).await;
    let elapsed = start.elapsed();

    for h in &handles {
        h.abort();
    }

    let min_commits = commit_counters
        .iter()
        .map(|c| c.load(Ordering::Relaxed))
        .min()
        .unwrap_or(0);
    let txs = total_txs.load(Ordering::Relaxed);

    let blocks_per_sec = min_commits as f64 / elapsed.as_secs_f64();
    let tx_per_sec = txs as f64 / elapsed.as_secs_f64();
    let ms_per_block = if min_commits > 0 {
        elapsed.as_millis() as f64 / min_commits as f64
    } else {
        f64::INFINITY
    };

    println!("  Config: {label}");
    println!(
        "    {NUM_VALIDATORS} validators, {DURATION_SECS}s, {TXS_PER_BLOCK} EVM transfers/block"
    );
    println!(
        "    Result: {blocks_per_sec:.1} blocks/sec, {tx_per_sec:.0} tx/sec, {ms_per_block:.1} ms/block"
    );
    println!("    Total: {min_commits} blocks, {txs} transactions executed");
    println!();
}

#[tokio::main]
async fn main() {
    println!("=== EVM Throughput Benchmark ({TXS_PER_BLOCK} transfers/block) ===\n");

    run_bench("Fast (timeout=500ms)", 500).await;
    run_bench("Normal (timeout=2000ms)", 2000).await;
    run_bench("Conservative (timeout=5000ms)", 5000).await;

    println!("Done.");
}
