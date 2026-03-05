use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use ed25519_dalek::SigningKey;
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
use rand::rngs::OsRng;
use tokio::sync::mpsc;

use hotmint_utxo::*;

const NUM_VALIDATORS: u64 = 4;
const DURATION_SECS: u64 = 10;
const TXS_PER_BLOCK: usize = 10;

/// Benchmark UTXO app: generates N transfers per block.
struct UtxoBenchApp {
    inner: UtxoApplication,
    alice_key: SigningKey,
    bob_pkh: [u8; 32],
    nonce: Mutex<u64>,
    commit_count: Arc<AtomicU64>,
    tx_count: Arc<AtomicU64>,
}

impl UtxoBenchApp {
    fn new(
        alice_key: SigningKey,
        bob_pkh: [u8; 32],
        commit_count: Arc<AtomicU64>,
        tx_count: Arc<AtomicU64>,
    ) -> Self {
        let alice_pk = alice_key.verifying_key().to_bytes();
        let alice_pkh = hash_pubkey(&alice_pk);

        // Create many genesis UTXOs for Alice (one per expected tx)
        let genesis_utxos: Vec<GenesisUtxo> = (0..10_000u64)
            .map(|_| GenesisUtxo {
                value: COIN,
                pubkey_hash: alice_pkh,
            })
            .collect();

        let config = UtxoConfig {
            genesis_utxos,
            log_on_commit: false,
            ..Default::default()
        };

        Self {
            inner: UtxoApplication::new(config),
            alice_key,
            bob_pkh,
            nonce: Mutex::new(0),
            commit_count,
            tx_count,
        }
    }
}

impl Application for UtxoBenchApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        let alice_pk = self.alice_key.verifying_key().to_bytes();
        let mut nonce = self.nonce.lock().unwrap();
        let start = *nonce as usize;

        let mut txs = Vec::with_capacity(TXS_PER_BLOCK);
        for i in start..start + TXS_PER_BLOCK {
            // Each genesis UTXO has a deterministic txid
            let genesis_txid = *blake3::hash(&(i as u64).to_le_bytes()).as_bytes();
            let outpoint = OutPoint {
                txid: genesis_txid,
                vout: 0,
            };

            let mut tx = UtxoTx {
                inputs: vec![TxInput {
                    prev_out: outpoint,
                    signature: vec![0u8; 64],
                    pubkey: alice_pk,
                }],
                outputs: vec![TxOutput {
                    value: COIN,
                    pubkey_hash: self.bob_pkh,
                }],
            };

            let hash = tx.signing_hash();
            let sig: ed25519_dalek::Signature = ed25519_dalek::Signer::sign(&self.alice_key, &hash);
            tx.inputs[0].signature = sig.to_bytes().to_vec();
            txs.push(tx);
        }

        *nonce += TXS_PER_BLOCK as u64;
        encode_payload(&txs)
    }

    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        let result = self.inner.execute_block(txs, ctx)?;
        self.tx_count.fetch_add(txs.len() as u64, Ordering::Relaxed);
        Ok(result)
    }

    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        self.commit_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

async fn run_bench(label: &str, base_timeout_ms: u64) {
    let alice_key = SigningKey::generate(&mut OsRng);
    let bob_key = SigningKey::generate(&mut OsRng);
    let bob_pkh = hash_pubkey(&bob_key.verifying_key().to_bytes());

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

        let app = UtxoBenchApp::new(alice_key.clone(), bob_pkh, commit_count, total_txs.clone());
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
        "    {NUM_VALIDATORS} validators, {DURATION_SECS}s, {TXS_PER_BLOCK} UTXO transfers/block"
    );
    println!(
        "    Result: {blocks_per_sec:.1} blocks/sec, {tx_per_sec:.0} tx/sec, {ms_per_block:.1} ms/block"
    );
    println!("    Total: {min_commits} blocks, {txs} transactions executed");
    println!();
}

#[tokio::main]
async fn main() {
    // Initialize vsdb
    let tmp = std::env::temp_dir().join("hotmint-utxo-bench");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    vsdb::vsdb_set_base_dir(&tmp).unwrap();

    println!("=== UTXO Throughput Benchmark ({TXS_PER_BLOCK} transfers/block) ===\n");

    run_bench("Fast (timeout=500ms)", 500).await;
    run_bench("Normal (timeout=2000ms)", 2000).await;

    println!("Done.");
}
