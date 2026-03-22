use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use std::time::Duration;
use tokio::sync::RwLock;

use hotmint_abci::client::IpcApplicationClient;
use hotmint_consensus::application::Application;
use hotmint_consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint_consensus::network::ChannelNetwork;
use hotmint_consensus::state::ConsensusState;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};
use hotmint_types::*;
use tokio::sync::mpsc;

const NUM_VALIDATORS: u64 = 4;

fn build_go_testserver() -> Option<std::path::PathBuf> {
    let go_server_dir = std::env::current_dir().unwrap().join("../../sdk/go");
    let binary = std::env::temp_dir().join(format!("hotmint-go-testserver-{}", std::process::id()));

    let status = Command::new("go")
        .args(["build", "-o", binary.to_str().unwrap(), "./cmd/testserver"])
        .current_dir(&go_server_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status();

    match status {
        Ok(s) if s.success() => Some(binary),
        Ok(_) => {
            eprintln!("go build failed, skipping Go e2e test");
            None
        }
        Err(e) => {
            eprintln!("go not available ({e}), skipping Go e2e test");
            None
        }
    }
}

fn start_go_server(binary: &std::path::Path, sock_path: &std::path::Path) -> Child {
    Command::new(binary)
        .arg(sock_path.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start Go server")
}

fn wait_for_socket(path: &std::path::Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("Go server did not create socket at {:?}", path);
}

/// End-to-end test: 4 Rust consensus engines talking through Go ABCI servers.
///
/// This test requires Go to be installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn go_ipc_consensus_e2e() {
    let binary = match build_go_testserver() {
        Some(b) => b,
        None => return,
    };

    let dir = std::env::temp_dir().join(format!("hotmint-go-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Start a Go ABCI server for each validator.
    let mut go_servers: Vec<Child> = Vec::new();
    let mut sock_paths = Vec::new();
    for i in 0..NUM_VALIDATORS {
        let path = dir.join(format!("go-app-{i}.sock"));
        let child = start_go_server(&binary, &path);
        go_servers.push(child);
        sock_paths.push(path);
    }

    // Wait for all sockets.
    for path in &sock_paths {
        wait_for_socket(path);
    }

    // Create validators.
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

    // Set up channel-based networking.
    let mut receivers = HashMap::new();
    let mut all_senders: HashMap<
        ValidatorId,
        mpsc::Sender<(Option<ValidatorId>, ConsensusMessage)>,
    > = HashMap::new();
    for i in 0..NUM_VALIDATORS {
        let (tx, rx) = mpsc::channel(8192);
        receivers.insert(ValidatorId(i), rx);
        all_senders.insert(ValidatorId(i), tx);
    }

    // Start consensus engines connected to Go ABCI servers.
    let mut handles = Vec::new();
    for (i, signer) in signers.into_iter().enumerate() {
        let vid = ValidatorId(i as u64);
        let rx = receivers.remove(&vid).unwrap();
        let senders: Vec<_> = all_senders
            .iter()
            .map(|(&id, tx)| (id, tx.clone()))
            .collect();

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

    // Run for 5 seconds.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Abort engines first — this releases the IPC connections so we can
    // open a new one to query the Go server (single-connection model).
    for h in &handles {
        h.abort();
    }
    // Allow the engine tasks to finish and drop their connections.
    for h in handles {
        let _ = h.await;
    }
    // Small delay for Go server to accept new connections.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Query commit count from one Go server via IPC.
    let client = IpcApplicationClient::new(&sock_paths[0]);
    let count_bytes = client.query("commits", &[]).unwrap();
    let commits = u64::from_le_bytes(count_bytes.try_into().unwrap_or([0; 8]));

    // Kill Go servers.
    for mut child in go_servers {
        let _ = child.kill();
        let _ = child.wait();
    }

    assert!(
        commits >= 1,
        "expected at least 1 commit via Go IPC, got {commits}"
    );
    eprintln!("Go e2e: {commits} commits in 5 seconds");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&binary);
}
