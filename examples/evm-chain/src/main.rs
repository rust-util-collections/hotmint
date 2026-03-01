mod app;
mod tx;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use hotmint_consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint_types::*;
use tokio::sync::mpsc;
use tracing::{Level, info};

use app::EvmApp;

const NUM_VALIDATORS: u64 = 4;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    info!("=== Hotmint EVM Chain Demo ===");
    info!("Starting {} validator EVM chain", NUM_VALIDATORS);
    info!(
        "Genesis: Alice (0x{:02x}{:02x}...) = 100 ETH, Bob (0x{:02x}{:02x}...) = 100 ETH",
        app::ALICE[0],
        app::ALICE[1],
        app::BOB[0],
        app::BOB[1],
    );
    info!("Each block: Alice sends 1 ETH to Bob\n");

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

    info!(
        "Validators: {}, Quorum: {}",
        validator_set.validator_count(),
        validator_set.quorum_threshold()
    );

    // Create message channels
    let mut receivers = HashMap::new();
    let mut all_senders: HashMap<ValidatorId, mpsc::Sender<(ValidatorId, ConsensusMessage)>> =
        HashMap::new();

    for i in 0..NUM_VALIDATORS {
        let (tx, rx) = mpsc::channel(8192);
        let vid = ValidatorId(i);
        receivers.insert(vid, rx);
        all_senders.insert(vid, tx);
    }

    // Track commits
    let commit_count = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();

    for (i, signer) in signers.into_iter().enumerate() {
        let vid = ValidatorId(i as u64);
        let rx = receivers.remove(&vid).unwrap();

        let senders: Vec<(ValidatorId, mpsc::Sender<(ValidatorId, ConsensusMessage)>)> =
            all_senders
                .iter()
                .map(|(&id, tx)| (id, tx.clone()))
                .collect();

        let network = ChannelNetwork::new(vid, senders);
        let store: Arc<RwLock<Box<dyn hotmint_consensus::store::BlockStore>>> =
            Arc::new(RwLock::new(Box::new(MemoryBlockStore::new())));

        let app = EvmApp::new(vid);
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
                pacemaker: None,
                persistence: None,
            },
        );

        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    info!("All validators spawned, EVM chain running...\n");

    // Run for 30 seconds
    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    let total = commit_count.load(Ordering::Relaxed);
    info!("\n=== EVM Chain Demo Complete ===");
    info!(
        "Ran for 30 seconds, {} total commits across validators",
        total
    );
}
