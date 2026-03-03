use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use hotmint_abci::client::IpcApplicationClient;
use hotmint_abci::server::{ApplicationHandler, IpcApplicationServer};
use hotmint_consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::pacemaker::PacemakerConfig;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint_types::context::OwnedBlockContext;
use hotmint_types::validator_update::EndBlockResponse;
use hotmint_types::*;
use tokio::sync::mpsc;

const NUM_VALIDATORS: u64 = 4;
const DURATION_SECS: u64 = 10;

/// IPC server handler: minimal logic, just counts commits.
struct BenchHandler {
    commit_count: Arc<AtomicU64>,
}

impl ApplicationHandler for BenchHandler {
    fn create_payload(&self, _ctx: OwnedBlockContext) -> Vec<u8> {
        // 1KB payload
        let data = vec![0xABu8; 1024];
        let len = data.len() as u32;
        let mut payload = Vec::with_capacity(4 + data.len());
        payload.extend_from_slice(&len.to_le_bytes());
        payload.extend_from_slice(&data);
        payload
    }

    fn execute_block(
        &self,
        _txs: Vec<Vec<u8>>,
        _ctx: OwnedBlockContext,
    ) -> Result<EndBlockResponse, String> {
        Ok(EndBlockResponse::default())
    }

    fn on_commit(&self, _block: Block, _ctx: OwnedBlockContext) -> Result<(), String> {
        self.commit_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

async fn run_bench(label: &str, base_timeout_ms: u64) {
    let dir = std::env::temp_dir().join(format!("hotmint-bench-ipc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let commit_count = Arc::new(AtomicU64::new(0));

    // Start one IPC server per validator (server handles one connection at a time)
    let mut sock_paths = Vec::new();
    let mut server_handles = Vec::new();
    for i in 0..NUM_VALIDATORS {
        let path = dir.join(format!("app-{i}.sock"));
        let handler = BenchHandler {
            commit_count: commit_count.clone(),
        };
        let server = Arc::new(IpcApplicationServer::new(&path, handler));
        let s = Arc::clone(&server);
        server_handles.push(tokio::spawn(async move {
            let _ = s.run().await;
        }));
        sock_paths.push(path);
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Create validators
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

    let mut commit_counters = Vec::new();
    let mut handles = Vec::new();

    for (i, signer) in signers.into_iter().enumerate() {
        let vid = ValidatorId(i as u64);
        let rx = receivers.remove(&vid).unwrap();
        let senders: Vec<_> = all_senders
            .iter()
            .map(|(&id, tx)| (id, tx.clone()))
            .collect();

        let per_node_commits = Arc::new(AtomicU64::new(0));
        commit_counters.push(per_node_commits.clone());

        // Connect to this validator's IPC server
        let ipc_client = IpcApplicationClient::new(&sock_paths[i]);

        let network = ChannelNetwork::new(vid, senders);
        let store: Arc<RwLock<Box<dyn hotmint_consensus::store::BlockStore>>> =
            Arc::new(RwLock::new(Box::new(MemoryBlockStore::new())));
        let state = ConsensusState::new(vid, validator_set.clone());

        let engine = ConsensusEngine::new(
            state,
            store,
            Box::new(network),
            Box::new(ipc_client),
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
    tokio::time::sleep(std::time::Duration::from_secs(DURATION_SECS)).await;
    let elapsed = start.elapsed();

    for h in &handles {
        h.abort();
    }
    for h in &server_handles {
        h.abort();
    }

    let server_commits = commit_count.load(Ordering::Relaxed);
    let blocks_per_sec = server_commits as f64 / elapsed.as_secs_f64() / NUM_VALIDATORS as f64;
    let ms_per_block = if server_commits > 0 {
        elapsed.as_millis() as f64 * NUM_VALIDATORS as f64 / server_commits as f64
    } else {
        f64::INFINITY
    };

    println!("  Config: {label}");
    println!(
        "    {NUM_VALIDATORS} validators, {DURATION_SECS}s, Unix socket IPC, 1KB payload/block"
    );
    println!(
        "    Result: {blocks_per_sec:.1} blocks/sec, {ms_per_block:.1} ms/block, {server_commits} total server commits"
    );
    println!();

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::main]
async fn main() {
    println!("=== IPC Throughput Benchmark (Unix socket, 4 validators) ===\n");

    run_bench("Fast (timeout=500ms)", 500).await;
    run_bench("Normal (timeout=2000ms)", 2000).await;
    run_bench("Conservative (timeout=5000ms)", 5000).await;

    println!("Done.");
}
