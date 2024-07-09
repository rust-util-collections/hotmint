//! End-to-end throughput benchmark for the Hotmint consensus network.
//!
//! Spawns N in-process validators using channel networking and measures:
//! - Block commit rate (blocks/sec)
//! - Average time per committed block
//! - Total blocks committed across all validators
//!
//! Usage: cargo run --release --bin hotmint-bench-e2e

use ruc::*;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use hotmint::consensus::application::Application;
use hotmint::consensus::engine::ConsensusEngine;
use hotmint::consensus::network::ChannelNetwork;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::MemoryBlockStore;
use hotmint::crypto::Ed25519Signer;
use hotmint::prelude::*;
use tokio::sync::mpsc;

const NUM_VALIDATORS: u64 = 4;
const DURATION_SECS: u64 = 10;

struct BenchApp {
    commit_count: Arc<AtomicU64>,
}

impl Application for BenchApp {
    fn on_commit(&self, _block: &Block) -> Result<()> {
        self.commit_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    println!("=== Hotmint E2E Throughput Benchmark ===");
    println!("validators: {NUM_VALIDATORS}");
    println!("duration:   {DURATION_SECS}s");
    println!();

    let mut signers: Vec<Option<Ed25519Signer>> = (0..NUM_VALIDATORS)
        .map(|i| Some(Ed25519Signer::generate(ValidatorId(i))))
        .collect();

    let validator_infos: Vec<ValidatorInfo> = signers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let s = s.as_ref().unwrap();
            ValidatorInfo {
                id: ValidatorId(i as u64),
                public_key: Signer::public_key(s),
                power: 1,
            }
        })
        .collect();
    let validator_set = ValidatorSet::new(validator_infos);

    let mut receivers = HashMap::new();
    let mut all_senders: HashMap<
        ValidatorId,
        mpsc::UnboundedSender<(ValidatorId, ConsensusMessage)>,
    > = HashMap::new();

    for i in 0..NUM_VALIDATORS {
        let (tx, rx) = mpsc::unbounded_channel();
        receivers.insert(ValidatorId(i), rx);
        all_senders.insert(ValidatorId(i), tx);
    }

    let mut commit_counters = Vec::new();
    let mut handles = Vec::new();

    for i in 0..NUM_VALIDATORS {
        let vid = ValidatorId(i);
        let rx = pnk!(receivers.remove(&vid));
        let senders: Vec<(
            ValidatorId,
            mpsc::UnboundedSender<(ValidatorId, ConsensusMessage)>,
        )> = all_senders
            .iter()
            .map(|(&id, tx)| (id, tx.clone()))
            .collect();

        let network = ChannelNetwork::new(vid, senders);
        let store = MemoryBlockStore::new();
        let commit_count = Arc::new(AtomicU64::new(0));
        commit_counters.push(commit_count.clone());
        let app = BenchApp {
            commit_count: commit_count.clone(),
        };
        let signer = pnk!(signers[i as usize].take());
        let state = ConsensusState::new(vid, validator_set.clone());

        let engine = ConsensusEngine::new(
            state,
            Box::new(store),
            Box::new(network),
            Box::new(app),
            Box::new(signer),
            rx,
        );

        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    let start = Instant::now();
    tokio::time::sleep(tokio::time::Duration::from_secs(DURATION_SECS)).await;
    let elapsed = start.elapsed();

    // Collect results
    let counts: Vec<u64> = commit_counters
        .iter()
        .map(|c| c.load(Ordering::Relaxed))
        .collect();

    let min_commits = *counts.iter().min().unwrap();
    let max_commits = *counts.iter().max().unwrap();
    let avg_commits = counts.iter().sum::<u64>() / NUM_VALIDATORS;

    let blocks_per_sec = min_commits as f64 / elapsed.as_secs_f64();
    let ms_per_block = if min_commits > 0 {
        elapsed.as_millis() as f64 / min_commits as f64
    } else {
        f64::INFINITY
    };

    println!("=== Results ===");
    println!("elapsed:         {:.2}s", elapsed.as_secs_f64());
    println!("commits/node:    min={min_commits}  max={max_commits}  avg={avg_commits}");
    println!("throughput:      {blocks_per_sec:.1} blocks/sec (by slowest node)");
    println!("latency:         {ms_per_block:.1} ms/block");
    println!();

    for (i, count) in counts.iter().enumerate() {
        println!("  V{i}: {count} blocks committed");
    }

    for h in handles {
        h.abort();
    }
}
