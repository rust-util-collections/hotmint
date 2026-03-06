use ruc::*;

use std::sync::atomic::{AtomicU64, Ordering};

use hotmint::consensus::application::Application;
use hotmint::consensus::engine::ConsensusEngineBuilder;
use hotmint::consensus::network::ChannelNetwork;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::MemoryBlockStore;
use hotmint::crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint::prelude::*;
use tracing::{Level, info};

const NUM_VALIDATORS: u64 = 4;

struct CountingApp {
    validator_id: ValidatorId,
    commit_count: AtomicU64,
}

impl Application for CountingApp {
    fn on_commit(&self, block: &Block, _ctx: &BlockContext) -> Result<()> {
        let count = self.commit_count.fetch_add(1, Ordering::Relaxed) + 1;
        info!(
            validator = %self.validator_id,
            height = block.height.as_u64(),
            hash = %block.hash,
            total_commits = count,
            "block committed"
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("hotmint-demo: 4-node in-process consensus demo");
        println!("Usage: hotmint-demo");
        println!();
        println!(
            "Runs {} validators connected via in-memory channels for 30 seconds.",
            NUM_VALIDATORS
        );
        return;
    }

    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    info!("starting hotmint with {} validators", NUM_VALIDATORS);

    let signers: Vec<Ed25519Signer> = (0..NUM_VALIDATORS)
        .map(|i| Ed25519Signer::generate(ValidatorId(i)))
        .collect();

    let signer_refs: Vec<&dyn Signer> = signers.iter().map(|s| s as &dyn Signer).collect();
    let validator_set = ValidatorSet::from_signers(&signer_refs);

    info!(
        validators = NUM_VALIDATORS,
        quorum = validator_set.quorum_threshold(),
        max_faulty_power = validator_set.max_faulty_power(),
        "validator set created"
    );

    let mesh = ChannelNetwork::create_mesh(NUM_VALIDATORS);
    assert_eq!(
        mesh.len(),
        signers.len(),
        "mesh and signers must have the same length"
    );

    let mut handles = Vec::new();

    for (i, ((network, rx), signer)) in mesh.into_iter().zip(signers.into_iter()).enumerate() {
        let vid = ValidatorId(i as u64);
        let store = MemoryBlockStore::new_shared();
        let state = ConsensusState::new(vid, validator_set.clone());

        let engine = ConsensusEngineBuilder::new()
            .state(state)
            .store(store)
            .network(Box::new(network))
            .app(Box::new(CountingApp {
                validator_id: vid,
                commit_count: AtomicU64::new(0),
            }))
            .signer(Box::new(signer))
            .messages(rx)
            .verifier(Box::new(Ed25519Verifier))
            .build()
            .expect("all required fields set");

        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    info!("all validators spawned, consensus running...");

    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    info!("shutting down after 30 seconds");
}
