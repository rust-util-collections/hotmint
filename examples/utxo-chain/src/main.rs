use ed25519_dalek::SigningKey;
use hotmint_consensus::engine::ConsensusEngineBuilder;
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint_types::*;
use rand::rngs::OsRng;
use tracing::{Level, info};

use utxo_chain::app::DemoUtxoApp;
use utxo_chain::hash_pubkey;

const NUM_VALIDATORS: u64 = 4;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    // Initialize vsdb in a temp directory
    let tmp = std::env::temp_dir().join("hotmint-utxo-demo");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    vsdb::vsdb_set_base_dir(&tmp).unwrap();

    // Generate demo keys
    let alice_key = SigningKey::generate(&mut OsRng);
    let bob_key = SigningKey::generate(&mut OsRng);
    let alice_pkh = hash_pubkey(&alice_key.verifying_key().to_bytes());
    let bob_pkh = hash_pubkey(&bob_key.verifying_key().to_bytes());

    info!("=== Hotmint UTXO Chain Demo ===");
    info!("Alice: {}... = 100 COIN", hex_short(&alice_pkh));
    info!("Bob:   {}... = 100 COIN", hex_short(&bob_pkh));
    info!("Each block: Alice sends 1 COIN to Bob\n");

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
            .app(Box::new(DemoUtxoApp::new(alice_key.clone(), &bob_key)))
            .signer(Box::new(signer))
            .messages(rx)
            .verifier(Box::new(Ed25519Verifier))
            .build()
            .expect("all required fields set");

        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    info!("All validators spawned, UTXO chain running...\n");

    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    info!("\n=== UTXO Chain Demo Complete ===");
    info!("Ran for 30 seconds");
}

fn hex_short(bytes: &[u8]) -> String {
    bytes[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}
