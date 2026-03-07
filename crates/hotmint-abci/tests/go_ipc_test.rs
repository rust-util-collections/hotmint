use std::process::{Command, Stdio};
use std::time::Duration;

use hotmint_abci::client::IpcApplicationClient;
use hotmint_consensus::application::Application;
use hotmint_types::block::{BlockHash, Height};
use hotmint_types::context::BlockContext;
use hotmint_types::crypto::PublicKey;
use hotmint_types::epoch::EpochNumber;
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};
use hotmint_types::view::ViewNumber;

fn make_validator_set() -> ValidatorSet {
    ValidatorSet::new(vec![ValidatorInfo {
        id: ValidatorId(0),
        public_key: PublicKey(vec![0]),
        power: 1,
    }])
}

fn make_block_context(vs: &ValidatorSet) -> BlockContext<'_> {
    BlockContext {
        height: Height(1),
        view: ViewNumber(0),
        proposer: ValidatorId(0),
        epoch: EpochNumber(0),
        epoch_start_view: ViewNumber(0),
        validator_set: vs,
    }
}

/// Integration test: Rust IpcApplicationClient → Go testserver.
///
/// This test requires Go to be installed and accessible in PATH.
/// It builds and runs the Go test server, then connects via IPC.
#[test]
fn rust_to_go_ipc() {
    let dir = std::env::temp_dir().join(format!("hotmint-go-ipc-{}", std::process::id()));
    let sock_path = dir.join("go-test.sock");
    std::fs::create_dir_all(&dir).unwrap();

    // Build the Go test server.
    let go_server_dir = std::env::current_dir().unwrap().join("../../sdk/go");

    let build_status = Command::new("go")
        .args([
            "build",
            "-o",
            dir.join("testserver").to_str().unwrap(),
            "./cmd/testserver",
        ])
        .current_dir(&go_server_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status();

    match build_status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!("go build failed with status: {status}");
            eprintln!("skipping Go integration test (go build failed)");
            return;
        }
        Err(e) => {
            eprintln!("go not available: {e}");
            eprintln!("skipping Go integration test (go not installed)");
            return;
        }
    }

    // Start the Go server.
    let mut go_server = Command::new(dir.join("testserver"))
        .arg(sock_path.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start Go test server");

    // Wait for the socket to appear.
    for _ in 0..50 {
        if sock_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        sock_path.exists(),
        "Go server did not create socket at {:?}",
        sock_path
    );

    let client = IpcApplicationClient::new(&sock_path);
    let vs = make_validator_set();
    let ctx = make_block_context(&vs);

    // Test create_payload — Go testApp returns "test".
    let payload = client.create_payload(&ctx);
    assert_eq!(payload, b"test", "create_payload mismatch");

    // Test validate_block — BaseApplication returns true.
    let block = hotmint_types::Block {
        height: Height(1),
        parent_hash: BlockHash::default(),
        view: ViewNumber(0),
        proposer: ValidatorId(0),
        payload: vec![],
        app_hash: BlockHash::default(),
        hash: BlockHash::default(),
    };
    assert!(
        client.validate_block(&block, &ctx),
        "validate_block should return true"
    );

    // Test validate_tx — BaseApplication returns true.
    assert!(
        client.validate_tx(&[1, 2, 3], None),
        "validate_tx should return true"
    );

    // Test execute_block — returns empty EndBlockResponse.
    let result = client.execute_block(&[&[1]], &ctx);
    assert!(result.is_ok(), "execute_block failed: {:?}", result.err());

    // Test query — Go testApp echoes data back.
    let result = client.query("/state", &[42]).unwrap();
    assert_eq!(result, &[42], "query echo mismatch");

    // Test on_commit — should succeed.
    let result = client.on_commit(&block, &ctx);
    assert!(result.is_ok(), "on_commit failed: {:?}", result.err());

    // Cleanup.
    let _ = go_server.kill();
    let _ = go_server.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
