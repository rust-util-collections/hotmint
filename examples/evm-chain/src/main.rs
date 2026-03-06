use hotmint_consensus::engine::ConsensusEngineBuilder;
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint_types::*;
use tracing::{Level, info};

use evm_chain::app::DemoEvmApp;

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
        evm_chain::app::ALICE[0],
        evm_chain::app::ALICE[1],
        evm_chain::app::BOB[0],
        evm_chain::app::BOB[1],
    );
    info!("Each block: Alice sends 1 ETH to Bob\n");

    let signers: Vec<Ed25519Signer> = (0..NUM_VALIDATORS)
        .map(|i| Ed25519Signer::generate(ValidatorId(i)))
        .collect();

    let signer_refs: Vec<&dyn Signer> = signers.iter().map(|s| s as &dyn Signer).collect();
    let validator_set = ValidatorSet::from_signers(&signer_refs);

    info!(
        "Validators: {}, Quorum: {}",
        validator_set.validator_count(),
        validator_set.quorum_threshold()
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
            .app(Box::new(DemoEvmApp::new()))
            .signer(Box::new(signer))
            .messages(rx)
            .verifier(Box::new(Ed25519Verifier))
            .build()
            .expect("all required fields set");

        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    info!("All validators spawned, EVM chain running...\n");

    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    info!("\n=== EVM Chain Demo Complete ===");
    info!("Ran for 30 seconds");
}
