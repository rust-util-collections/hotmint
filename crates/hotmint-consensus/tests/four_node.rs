use ruc::*;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use hotmint_consensus::application::Application;
use hotmint_consensus::engine::ConsensusEngine;
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::Ed25519Signer;
use hotmint_types::*;
use tokio::sync::mpsc;

const NUM_VALIDATORS: u64 = 4;

struct TestApp {
    commit_count: Arc<AtomicU64>,
}

impl Application for TestApp {
    fn on_commit(&self, _block: &Block, _ctx: &BlockContext) -> Result<()> {
        self.commit_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn spawn_network(n: u64) -> (Vec<Arc<AtomicU64>>, Vec<tokio::task::JoinHandle<()>>) {
    let mut signers: Vec<Option<Ed25519Signer>> = (0..n)
        .map(|i| Some(Ed25519Signer::generate(ValidatorId(i))))
        .collect();

    let validator_infos: Vec<ValidatorInfo> = signers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let s = s.as_ref().unwrap();
            ValidatorInfo {
                id: ValidatorId(i as u64),
                public_key: hotmint_types::Signer::public_key(s),
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

    for i in 0..n {
        let (tx, rx) = mpsc::unbounded_channel();
        receivers.insert(ValidatorId(i), rx);
        all_senders.insert(ValidatorId(i), tx);
    }

    let mut counters = Vec::new();
    let mut handles = Vec::new();

    for i in 0..n {
        let vid = ValidatorId(i);
        let rx = receivers.remove(&vid).unwrap();
        let senders: Vec<(
            ValidatorId,
            mpsc::UnboundedSender<(ValidatorId, ConsensusMessage)>,
        )> = all_senders
            .iter()
            .map(|(&id, tx)| (id, tx.clone()))
            .collect();

        let network = ChannelNetwork::new(vid, senders);
        let store = Arc::new(RwLock::new(
            Box::new(MemoryBlockStore::new()) as Box<dyn hotmint_consensus::store::BlockStore>
        ));
        let commit_count = Arc::new(AtomicU64::new(0));
        counters.push(commit_count.clone());
        let app = TestApp {
            commit_count: commit_count.clone(),
        };
        let signer = signers[i as usize].take().unwrap();
        let state = ConsensusState::new(vid, validator_set.clone());

        let engine = ConsensusEngine::new(
            state,
            store,
            Box::new(network),
            Box::new(app),
            Box::new(signer),
            rx,
            None,
        );

        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    (counters, handles)
}

#[tokio::test]
async fn test_four_node_consensus_commits_blocks() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    let (counters, handles) = spawn_network(NUM_VALIDATORS);

    tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;

    // All validators should have committed at least 1 block
    for (i, counter) in counters.iter().enumerate() {
        let count = counter.load(Ordering::Relaxed);
        assert!(
            count >= 1,
            "validator {} committed {} blocks, expected >= 1",
            i,
            count
        );
    }

    // At least one validator should have committed multiple blocks
    let max_commits = counters
        .iter()
        .map(|c| c.load(Ordering::Relaxed))
        .max()
        .unwrap();
    assert!(
        max_commits >= 2,
        "max commits is {}, expected >= 2",
        max_commits
    );

    for h in handles {
        h.abort();
    }
}

#[tokio::test]
async fn test_consensus_tolerates_one_silent_validator() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();

    // Spawn 4 nodes but immediately abort one
    let (counters, handles) = spawn_network(NUM_VALIDATORS);

    // Kill validator 3 — simulate a silent/crashed node
    handles[3].abort();

    // With 3 out of 4 validators (quorum=3), consensus should still work
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    // The 3 active validators should commit blocks
    for (i, counter) in counters.iter().enumerate().take(3) {
        let count = counter.load(Ordering::Relaxed);
        assert!(
            count >= 1,
            "active validator {} committed {} blocks, expected >= 1",
            i,
            count
        );
    }

    for h in handles {
        h.abort();
    }
}
