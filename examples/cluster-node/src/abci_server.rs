//! Minimal Rust ABCI server for testing dual-process mode.
//!
//! Usage: rust-abci-server <socket-path>

use std::sync::Arc;

use hotmint_abci::server::{ApplicationHandler, IpcApplicationServer};
use hotmint_types::context::OwnedBlockContext;
use hotmint_types::validator_update::EndBlockResponse;

struct NoopHandler;

impl ApplicationHandler for NoopHandler {
    fn execute_block(
        &self,
        _txs: Vec<Vec<u8>>,
        _ctx: OwnedBlockContext,
    ) -> Result<EndBlockResponse, String> {
        Ok(EndBlockResponse::default())
    }
}

#[tokio::main]
async fn main() {
    let socket_path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: rust-abci-server <socket-path>");
        std::process::exit(1);
    });

    eprintln!("Rust ABCI server starting on {socket_path}");
    let server = Arc::new(IpcApplicationServer::new(&socket_path, NoopHandler));
    server.run().await.unwrap();
}
