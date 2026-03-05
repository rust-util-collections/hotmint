use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use ed25519_dalek::SigningKey;
use hotmint_consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint_types::*;
use hotmint_utxo::hash_pubkey;
use rand::rngs::OsRng;
use tokio::sync::mpsc;
use tracing::{Level, info};

use utxo_chain::app::DemoUtxoApp;

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

    let mut receivers = HashMap::new();
    let mut all_senders: HashMap<ValidatorId, mpsc::Sender<(ValidatorId, ConsensusMessage)>> =
        HashMap::new();

    for i in 0..NUM_VALIDATORS {
        let (tx, rx) = mpsc::channel(8192);
        let vid = ValidatorId(i);
        receivers.insert(vid, rx);
        all_senders.insert(vid, tx);
    }

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

        let app = DemoUtxoApp::new(alice_key.clone(), &bob_key);
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
