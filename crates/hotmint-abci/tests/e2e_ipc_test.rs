use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use hotmint_abci::client::IpcApplicationClient;
use hotmint_abci::server::{ApplicationHandler, IpcApplicationServer};
use hotmint_consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint_types::context::OwnedBlockContext;
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};
use hotmint_types::validator_update::EndBlockResponse;
use hotmint_types::*;
use tokio::sync::mpsc;

const NUM_VALIDATORS: u64 = 4;

/// Test ApplicationHandler that counts commits via IPC.
struct CommitCounter {
    commit_count: Arc<AtomicU64>,
}

impl ApplicationHandler for CommitCounter {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ipc_consensus_e2e() {
    let dir = std::env::temp_dir().join(format!("hotmint-ipc-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Each validator gets its own IPC server (server handles one connection at a time)
    let commit_count = Arc::new(AtomicU64::new(0));
    let mut sock_paths = Vec::new();
    let mut server_handles = Vec::new();
    for i in 0..NUM_VALIDATORS {
        let path = dir.join(format!("app-{i}.sock"));
        let handler = CommitCounter {
            commit_count: commit_count.clone(),
        };
        let server = Arc::new(IpcApplicationServer::new(&path, handler));
        let server_clone = Arc::clone(&server);
        server_handles.push(tokio::spawn(async move {
            let _ = server_clone.run().await;
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

    let mut handles = Vec::new();
    for (i, signer) in signers.into_iter().enumerate() {
        let vid = ValidatorId(i as u64);
        let rx = receivers.remove(&vid).unwrap();
        let senders: Vec<_> = all_senders
            .iter()
            .map(|(&id, tx)| (id, tx.clone()))
            .collect();

        // Each validator connects to its own IPC server
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
                pacemaker: None,
                persistence: None,
            },
        );

        handles.push(tokio::spawn(async move { engine.run().await }));
    }

    // Run for 5 seconds
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Abort all engines
    for h in &handles {
        h.abort();
    }
    for h in &server_handles {
        h.abort();
    }

    let commits = commit_count.load(Ordering::Relaxed);
    assert!(
        commits >= 1,
        "expected at least 1 commit via IPC, got {commits}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
