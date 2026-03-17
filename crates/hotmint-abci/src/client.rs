use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ruc::*;

use hotmint_consensus::application::Application;
use hotmint_types::Block;
use hotmint_types::context::{BlockContext, OwnedBlockContext, TxContext};
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator_update::EndBlockResponse;

use crate::protocol::{self, Request, Response};

/// IPC client that implements [`Application`] by forwarding every call over a
/// Unix domain socket using length-prefixed protobuf frames.
pub struct IpcApplicationClient {
    socket_path: PathBuf,
    conn: Mutex<Option<UnixStream>>,
}

impl IpcApplicationClient {
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            conn: Mutex::new(None),
        }
    }

    /// Try to connect to the ABCI socket. Returns an error if unreachable.
    pub fn check_connection(&self) -> Result<()> {
        let stream = UnixStream::connect(&self.socket_path).c(d!("connect to ABCI socket"))?;
        let mut guard = self.conn.lock().map_err(|e| eg!(e.to_string()))?;
        *guard = Some(stream);
        Ok(())
    }

    /// Send a request and wait for the response, lazily connecting on first use.
    fn call(&self, req: &Request) -> Result<Response> {
        let payload = protocol::encode_request(req);

        let mut guard = self.conn.lock().map_err(|e| eg!(e.to_string()))?;
        if guard.is_none() {
            let stream = UnixStream::connect(&self.socket_path).c(d!("connect to IPC socket"))?;
            *guard = Some(stream);
        }
        let stream = guard.as_mut().unwrap();

        write_frame_sync(stream, &payload).c(d!("write request frame"))?;
        let resp_bytes = read_frame_sync(stream).c(d!("read response frame"))?;
        let resp = protocol::decode_response(&resp_bytes)
            .map_err(|e| eg!(e.to_string()))
            .c(d!("decode response"))?;
        Ok(resp)
    }
}

fn write_frame_sync(w: &mut impl Write, payload: &[u8]) -> std::io::Result<()> {
    let len = payload.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame_sync(r: &mut impl Read) -> std::io::Result<Vec<u8>> {
    const MAX_FRAME: usize = 64 * 1024 * 1024;
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame size {len} exceeds max {MAX_FRAME}"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

impl Application for IpcApplicationClient {
    fn create_payload(&self, ctx: &BlockContext) -> Vec<u8> {
        let req = Request::CreatePayload(OwnedBlockContext::from(ctx));
        match self.call(&req) {
            Ok(Response::CreatePayload(payload)) => payload,
            Ok(other) => {
                tracing::error!(?other, "IPC_FAULT: unexpected response for create_payload");
                vec![]
            }
            Err(e) => {
                tracing::error!(%e, "IPC_FAULT: create_payload call failed — proposing empty block");
                vec![]
            }
        }
    }

    fn validate_block(&self, block: &Block, ctx: &BlockContext) -> bool {
        let req = Request::ValidateBlock {
            block: block.clone(),
            ctx: OwnedBlockContext::from(ctx),
        };
        match self.call(&req) {
            Ok(Response::ValidateBlock(ok)) => ok,
            Ok(other) => {
                panic!("IPC_FAULT: unexpected response for validate_block: {other:?}");
            }
            Err(e) => {
                panic!("IPC_FAULT: validate_block call failed — cannot safely validate block without ABCI: {e}");
            }
        }
    }

    fn validate_tx(&self, tx: &[u8], ctx: Option<&TxContext>) -> bool {
        let req = Request::ValidateTx {
            tx: tx.to_vec(),
            ctx: ctx.cloned(),
        };
        match self.call(&req) {
            Ok(Response::ValidateTx(ok)) => ok,
            Ok(other) => {
                tracing::error!(?other, "IPC_FAULT: unexpected response for validate_tx");
                false
            }
            Err(e) => {
                tracing::error!(%e, "IPC_FAULT: validate_tx call failed — rejecting tx");
                false
            }
        }
    }

    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        let req = Request::ExecuteBlock {
            txs: txs.iter().map(|t| t.to_vec()).collect(),
            ctx: OwnedBlockContext::from(ctx),
        };
        match self.call(&req)? {
            Response::ExecuteBlock(result) => result.map_err(|e| eg!(e)),
            other => Err(eg!(format!(
                "unexpected response for execute_block: {other:?}"
            ))),
        }
    }

    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()> {
        let req = Request::OnCommit {
            block: block.clone(),
            ctx: OwnedBlockContext::from(ctx),
        };
        match self.call(&req)? {
            Response::OnCommit(result) => result.map_err(|e| eg!(e)),
            other => Err(eg!(format!("unexpected response for on_commit: {other:?}"))),
        }
    }

    fn on_evidence(&self, proof: &EquivocationProof) -> Result<()> {
        let req = Request::OnEvidence(proof.clone());
        match self.call(&req)? {
            Response::OnEvidence(result) => result.map_err(|e| eg!(e)),
            other => Err(eg!(format!(
                "unexpected response for on_evidence: {other:?}"
            ))),
        }
    }

    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        let req = Request::Query {
            path: path.to_string(),
            data: data.to_vec(),
        };
        match self.call(&req)? {
            Response::Query(result) => result.map_err(|e| eg!(e)),
            other => Err(eg!(format!("unexpected response for query: {other:?}"))),
        }
    }
}
