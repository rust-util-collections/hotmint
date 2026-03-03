use ruc::*;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use hotmint::consensus::application::Application;
use hotmint::consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint::consensus::network::ChannelNetwork;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::MemoryBlockStore;
use hotmint::crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint::prelude::*;
use tokio::sync::mpsc;
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

    info!(
        validators = NUM_VALIDATORS,
        quorum = validator_set.quorum_threshold(),
        max_faulty_power = validator_set.max_faulty_power(),
        "validator set created"
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

    for i in 0..NUM_VALIDATORS {
        let vid = ValidatorId(i);
        let rx = pnk!(receivers.remove(&vid));

        let senders: Vec<(ValidatorId, mpsc::Sender<(ValidatorId, ConsensusMessage)>)> =
            all_senders
                .iter()
                .map(|(&id, tx)| (id, tx.clone()))
                .collect();

        let network = ChannelNetwork::new(vid, senders);
        let store = Arc::new(RwLock::new(
            Box::new(MemoryBlockStore::new()) as Box<dyn hotmint::consensus::store::BlockStore>
        ));
        let app = CountingApp {
            validator_id: vid,
            commit_count: AtomicU64::new(0),
        };
        let signer = pnk!(signers[i as usize].take());
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

    info!("all validators spawned, consensus running...");

    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    info!("shutting down after 30 seconds");
}
