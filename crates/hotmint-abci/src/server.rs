use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tokio::net::UnixListener;

use hotmint_types::Block;
use hotmint_types::context::{OwnedBlockContext, TxContext};
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator_update::EndBlockResponse;

use crate::protocol::{self, Request, Response};

/// Owned-data callback interface for applications running in a separate process.
///
/// This is the cross-process counterpart of `hotmint_consensus::Application`.
/// All parameters are owned so they can be deserialized from the wire.
pub trait ApplicationHandler: Send + Sync {
    fn create_payload(&self, ctx: OwnedBlockContext) -> Vec<u8> {
        let _ = ctx;
        vec![]
    }

    fn validate_block(&self, block: Block, ctx: OwnedBlockContext) -> bool {
        let _ = (block, ctx);
        true
    }

    fn validate_tx(&self, tx: Vec<u8>, ctx: Option<TxContext>) -> bool {
        let _ = (tx, ctx);
        true
    }

    fn execute_block(
        &self,
        txs: Vec<Vec<u8>>,
        ctx: OwnedBlockContext,
    ) -> Result<EndBlockResponse, String>;

    fn on_commit(&self, block: Block, ctx: OwnedBlockContext) -> Result<(), String> {
        let _ = (block, ctx);
        Ok(())
    }

    fn on_evidence(&self, proof: EquivocationProof) -> Result<(), String> {
        let _ = proof;
        Ok(())
    }

    fn query(&self, path: String, data: Vec<u8>) -> Result<Vec<u8>, String> {
        let _ = (path, data);
        Ok(vec![])
    }
}

/// IPC server that listens on a Unix domain socket and dispatches incoming
/// requests to an [`ApplicationHandler`].
pub struct IpcApplicationServer<H> {
    socket_path: PathBuf,
    handler: H,
}

impl<H> Drop for IpcApplicationServer<H> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl<H: ApplicationHandler + 'static> IpcApplicationServer<H> {
    pub fn new(socket_path: impl AsRef<Path>, handler: H) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            handler,
        }
    }

    /// Run the server, accepting connections and processing requests.
    ///
    /// This handles one connection at a time (matching the single-threaded
    /// consensus engine model). The server runs until the listener is dropped
    /// or the task is cancelled.
    pub async fn run(&self) -> io::Result<()> {
        // Remove stale socket file if present.
        let _ = fs::remove_file(&self.socket_path);
        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(path = %self.socket_path.display(), "IPC server listening");

        loop {
            let (mut stream, _addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!(error = %e, "IPC accept failed, continuing");
                    continue;
                }
            };
            tracing::debug!("IPC client connected");

            loop {
                let frame = match protocol::read_frame(&mut stream).await {
                    Ok(f) => f,
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                        tracing::debug!("IPC client disconnected");
                        break;
                    }
                    Err(e) => {
                        tracing::error!(%e, "read_frame error");
                        break;
                    }
                };

                let req: Request = match protocol::decode_request(&frame) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!(%e, "failed to decode request");
                        break;
                    }
                };

                let resp = self.dispatch(req);
                let resp_bytes = protocol::encode_response(&resp);

                if let Err(e) = protocol::write_frame(&mut stream, &resp_bytes).await {
                    tracing::error!(%e, "write_frame error");
                    break;
                }
            }
        }
    }

    fn dispatch(&self, req: Request) -> Response {
        match req {
            Request::CreatePayload(ctx) => {
                Response::CreatePayload(self.handler.create_payload(ctx))
            }
            Request::ValidateBlock { block, ctx } => {
                Response::ValidateBlock(self.handler.validate_block(block, ctx))
            }
            Request::ValidateTx { tx, ctx } => {
                Response::ValidateTx(self.handler.validate_tx(tx, ctx))
            }
            Request::ExecuteBlock { txs, ctx } => {
                Response::ExecuteBlock(self.handler.execute_block(txs, ctx))
            }
            Request::OnCommit { block, ctx } => {
                Response::OnCommit(self.handler.on_commit(block, ctx))
            }
            Request::OnEvidence(proof) => Response::OnEvidence(self.handler.on_evidence(proof)),
            Request::Query { path, data } => Response::Query(self.handler.query(path, data)),
        }
    }
}
