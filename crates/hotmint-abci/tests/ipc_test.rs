use std::sync::Arc;

use hotmint_abci::client::IpcApplicationClient;
use hotmint_abci::server::{ApplicationHandler, IpcApplicationServer};
use hotmint_consensus::application::Application;
use hotmint_types::Block;
use hotmint_types::block::{BlockHash, Height};
use hotmint_types::context::{BlockContext, OwnedBlockContext, TxContext};
use hotmint_types::crypto::PublicKey;
use hotmint_types::epoch::EpochNumber;
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};
use hotmint_types::validator_update::EndBlockResponse;
use hotmint_types::view::ViewNumber;

/// A simple application handler that echoes back identifiable data.
struct EchoHandler;

impl ApplicationHandler for EchoHandler {
    fn create_payload(&self, ctx: OwnedBlockContext) -> Vec<u8> {
        // Return the height as payload bytes.
        ctx.height.as_u64().to_le_bytes().to_vec()
    }

    fn validate_block(&self, _block: Block, _ctx: OwnedBlockContext) -> bool {
        true
    }

    fn validate_tx(&self, tx: Vec<u8>, _ctx: Option<TxContext>) -> bool {
        // Accept if first byte is non-zero.
        tx.first().copied().unwrap_or(0) != 0
    }

    fn execute_block(
        &self,
        _txs: Vec<Vec<u8>>,
        _ctx: OwnedBlockContext,
    ) -> Result<EndBlockResponse, String> {
        Ok(EndBlockResponse::default())
    }

    fn on_commit(&self, _block: Block, _ctx: OwnedBlockContext) -> Result<(), String> {
        Ok(())
    }

    fn on_evidence(
        &self,
        _proof: hotmint_types::evidence::EquivocationProof,
    ) -> Result<(), String> {
        Ok(())
    }

    fn query(&self, path: String, data: Vec<u8>) -> Result<Vec<u8>, String> {
        // Echo: path bytes ++ data.
        let mut out = path.into_bytes();
        out.extend_from_slice(&data);
        Ok(out)
    }
}

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

fn make_block() -> Block {
    Block {
        height: Height(1),
        parent_hash: BlockHash::default(),
        view: ViewNumber(0),
        proposer: ValidatorId(0),
        payload: vec![1, 2, 3],
        app_hash: BlockHash::default(),
        hash: BlockHash::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ipc_roundtrip() {
    let dir = std::env::temp_dir().join(format!("hotmint-ipc-test-{}", std::process::id()));
    let sock_path = dir.join("test.sock");
    std::fs::create_dir_all(&dir).unwrap();

    let server = Arc::new(IpcApplicationServer::new(&sock_path, EchoHandler));
    let server_handle = {
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            let _ = server.run().await;
        })
    };

    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = IpcApplicationClient::new(&sock_path);
    let vs = make_validator_set();
    let ctx = make_block_context(&vs);

    // create_payload
    let payload = client.create_payload(&ctx);
    assert_eq!(payload, 1u64.to_le_bytes().to_vec());

    // validate_block
    let block = make_block();
    assert!(client.validate_block(&block, &ctx));

    // validate_tx — accepted
    assert!(client.validate_tx(&[1, 2, 3], None));
    // validate_tx — rejected (first byte is zero)
    assert!(!client.validate_tx(&[0, 1, 2], None));

    // execute_block
    let result = client.execute_block(&[&[1, 2], &[3, 4]], &ctx);
    assert!(result.is_ok());

    // on_commit
    let result = client.on_commit(&block, &ctx);
    assert!(result.is_ok());

    // query
    let result = client.query("hello/", &[42]).unwrap();
    assert_eq!(result, b"hello/\x2a");

    server_handle.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
