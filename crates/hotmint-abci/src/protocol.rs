use serde::{Deserialize, Serialize};

use hotmint_types::Block;
use hotmint_types::context::{OwnedBlockContext, TxContext};
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator_update::EndBlockResponse;

/// IPC request sent from the consensus engine (client) to the application (server).
#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    CreatePayload(OwnedBlockContext),
    ValidateBlock {
        block: Block,
        ctx: OwnedBlockContext,
    },
    ValidateTx {
        tx: Vec<u8>,
        ctx: Option<TxContext>,
    },
    ExecuteBlock {
        txs: Vec<Vec<u8>>,
        ctx: OwnedBlockContext,
    },
    OnCommit {
        block: Block,
        ctx: OwnedBlockContext,
    },
    OnEvidence(EquivocationProof),
    Query {
        path: String,
        data: Vec<u8>,
    },
}

/// IPC response sent from the application (server) back to the consensus engine (client).
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    CreatePayload(Vec<u8>),
    ValidateBlock(bool),
    ValidateTx(bool),
    ExecuteBlock(Result<EndBlockResponse, String>),
    OnCommit(Result<(), String>),
    OnEvidence(Result<(), String>),
    Query(Result<Vec<u8>, String>),
}

/// Write a length-prefixed CBOR frame to an async writer.
pub async fn write_frame(
    writer: &mut (impl tokio::io::AsyncWriteExt + Unpin),
    payload: &[u8],
) -> std::io::Result<()> {
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await
}

/// Maximum IPC frame size (64 MB).
const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

/// Read a length-prefixed CBOR frame from an async reader.
pub async fn read_frame(
    reader: &mut (impl tokio::io::AsyncReadExt + Unpin),
) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame size {len} exceeds max {MAX_FRAME_SIZE}"),
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}
