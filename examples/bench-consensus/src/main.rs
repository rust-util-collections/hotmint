use ruc::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use hotmint::consensus::application::Application;
use hotmint::consensus::engine::ConsensusEngineBuilder;
use hotmint::consensus::network::ChannelNetwork;
use hotmint::consensus::pacemaker::PacemakerConfig;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::MemoryBlockStore;
use hotmint::crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint::prelude::*;

const NUM_VALIDATORS: u64 = 4;
const DURATION_SECS: u64 = 10;

/// Minimal application that creates a 1KB payload per block.
struct ThroughputApp {
    commit_count: Arc<AtomicU64>,
}

impl Application for ThroughputApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        // 1KB fixed payload (simulates a minimal transaction batch)
        let data = vec![0xABu8; 1024];
        let len = data.len() as u32;
        let mut payload = Vec::with_capacity(4 + data.len());
        payload.extend_from_slice(&len.to_le_bytes());
        payload.extend_from_slice(&data);
        payload
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

    let signer_refs: Vec<&dyn Signer> = signers.iter().map(|s| s as &dyn Signer).collect();
    let validator_set = ValidatorSet::from_signers(&signer_refs);

    let mesh = ChannelNetwork::create_mesh(NUM_VALIDATORS);
    assert_eq!(
        mesh.len(),
        signers.len(),
        "mesh and signers must have the same length"
    );

    let mut commit_counters = Vec::new();
    let mut handles = Vec::new();

    for (i, ((network, rx), signer)) in mesh.into_iter().zip(signers.into_iter()).enumerate() {
        let vid = ValidatorId(i as u64);

        let commit_count = Arc::new(AtomicU64::new(0));
        commit_counters.push(commit_count.clone());

        let store = MemoryBlockStore::new_shared();
        let state = ConsensusState::new(vid, validator_set.clone());

        let engine = ConsensusEngineBuilder::new()
            .state(state)
            .store(store)
            .network(Box::new(network))
            .app(Box::new(ThroughputApp {
                commit_count: commit_count.clone(),
            }))
            .signer(Box::new(signer))
            .messages(rx)
            .verifier(Box::new(Ed25519Verifier))
            .pacemaker(PacemakerConfig {
                base_timeout_ms,
                max_timeout_ms: base_timeout_ms * 15,
                backoff_multiplier: 1.5,
            })
            .build()
            .expect("all required fields set");

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

    let blocks_per_sec = min_commits as f64 / elapsed.as_secs_f64();
    let ms_per_block = if min_commits > 0 {
        elapsed.as_millis() as f64 / min_commits as f64
    } else {
        f64::INFINITY
    };

    println!("  Config: {label}");
    println!("    {NUM_VALIDATORS} validators, {DURATION_SECS}s duration, 1KB payload/block");
    println!(
        "    Result: {blocks_per_sec:.1} blocks/sec, {ms_per_block:.1} ms/block, {min_commits} blocks committed"
    );
    println!();
}

#[tokio::main]
async fn main() {
    println!("=== Consensus Throughput Benchmark ===\n");

    run_bench("Fast (timeout=500ms)", 500).await;
    run_bench("Normal (timeout=2000ms)", 2000).await;
    run_bench("Conservative (timeout=5000ms)", 5000).await;

    println!("Done.");
}
