use hotmint_types::Block;
use hotmint_types::context::{OwnedBlockContext, TxContext};
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator_update::EndBlockResponse;

use hotmint_abci_proto::pb;
use prost::Message;

/// IPC request sent from the consensus engine (client) to the application (server).
#[derive(Debug)]
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
#[derive(Debug)]
pub enum Response {
    CreatePayload(Vec<u8>),
    ValidateBlock(bool),
    ValidateTx(bool),
    ExecuteBlock(Result<EndBlockResponse, String>),
    OnCommit(Result<(), String>),
    OnEvidence(Result<(), String>),
    Query(Result<Vec<u8>, String>),
}

// ---- Protobuf encode/decode for Request ----

pub fn encode_request(req: &Request) -> Vec<u8> {
    let proto_req = match req {
        Request::CreatePayload(ctx) => pb::Request {
            request: Some(pb::request::Request::CreatePayload(ctx.into())),
        },
        Request::ValidateBlock { block, ctx } => pb::Request {
            request: Some(pb::request::Request::ValidateBlock(
                pb::ValidateBlockRequest {
                    block: Some(block.into()),
                    ctx: Some(ctx.into()),
                },
            )),
        },
        Request::ValidateTx { tx, ctx } => pb::Request {
            request: Some(pb::request::Request::ValidateTx(pb::ValidateTxRequest {
                tx: tx.clone(),
                ctx: ctx.as_ref().map(|c| c.into()),
            })),
        },
        Request::ExecuteBlock { txs, ctx } => pb::Request {
            request: Some(pb::request::Request::ExecuteBlock(
                pb::ExecuteBlockRequest {
                    txs: txs.clone(),
                    ctx: Some(ctx.into()),
                },
            )),
        },
        Request::OnCommit { block, ctx } => pb::Request {
            request: Some(pb::request::Request::OnCommit(pb::OnCommitRequest {
                block: Some(block.into()),
                ctx: Some(ctx.into()),
            })),
        },
        Request::OnEvidence(proof) => pb::Request {
            request: Some(pb::request::Request::OnEvidence(proof.into())),
        },
        Request::Query { path, data } => pb::Request {
            request: Some(pb::request::Request::Query(pb::QueryRequest {
                path: path.clone(),
                data: data.clone(),
            })),
        },
    };
    proto_req.encode_to_vec()
}

pub fn decode_request(buf: &[u8]) -> Result<Request, prost::DecodeError> {
    let proto_req = pb::Request::decode(buf)?;
    let req = match proto_req
        .request
        .ok_or_else(|| prost::DecodeError::new("missing request oneof"))?
    {
        pb::request::Request::CreatePayload(ctx) => Request::CreatePayload(ctx.into()),
        pb::request::Request::ValidateBlock(r) => Request::ValidateBlock {
            block: r
                .block
                .ok_or_else(|| prost::DecodeError::new("missing block"))?
                .into(),
            ctx: r
                .ctx
                .ok_or_else(|| prost::DecodeError::new("missing ctx"))?
                .into(),
        },
        pb::request::Request::ValidateTx(r) => Request::ValidateTx {
            tx: r.tx,
            ctx: r.ctx.map(Into::into),
        },
        pb::request::Request::ExecuteBlock(r) => Request::ExecuteBlock {
            txs: r.txs,
            ctx: r
                .ctx
                .ok_or_else(|| prost::DecodeError::new("missing ctx"))?
                .into(),
        },
        pb::request::Request::OnCommit(r) => Request::OnCommit {
            block: r
                .block
                .ok_or_else(|| prost::DecodeError::new("missing block"))?
                .into(),
            ctx: r
                .ctx
                .ok_or_else(|| prost::DecodeError::new("missing ctx"))?
                .into(),
        },
        pb::request::Request::OnEvidence(proof) => Request::OnEvidence(proof.into()),
        pb::request::Request::Query(r) => Request::Query {
            path: r.path,
            data: r.data,
        },
    };
    Ok(req)
}

// ---- Protobuf encode/decode for Response ----

pub fn encode_response(resp: &Response) -> Vec<u8> {
    let proto_resp = match resp {
        Response::CreatePayload(payload) => pb::Response {
            response: Some(pb::response::Response::CreatePayload(
                pb::CreatePayloadResponse {
                    payload: payload.clone(),
                },
            )),
        },
        Response::ValidateBlock(ok) => pb::Response {
            response: Some(pb::response::Response::ValidateBlock(
                pb::ValidateBlockResponse { ok: *ok },
            )),
        },
        Response::ValidateTx(ok) => pb::Response {
            response: Some(pb::response::Response::ValidateTx(pb::ValidateTxResponse {
                ok: *ok,
            })),
        },
        Response::ExecuteBlock(result) => pb::Response {
            response: Some(pb::response::Response::ExecuteBlock(
                pb::ExecuteBlockResponse {
                    result: result.as_ref().ok().map(|r| r.into()),
                    error: result.as_ref().err().cloned().unwrap_or_default(),
                },
            )),
        },
        Response::OnCommit(result) => pb::Response {
            response: Some(pb::response::Response::OnCommit(pb::OnCommitResponse {
                error: result.as_ref().err().cloned().unwrap_or_default(),
            })),
        },
        Response::OnEvidence(result) => pb::Response {
            response: Some(pb::response::Response::OnEvidence(pb::OnEvidenceResponse {
                error: result.as_ref().err().cloned().unwrap_or_default(),
            })),
        },
        Response::Query(result) => pb::Response {
            response: Some(pb::response::Response::Query(pb::QueryResponse {
                data: result.as_ref().ok().cloned().unwrap_or_default(),
                error: result.as_ref().err().cloned().unwrap_or_default(),
            })),
        },
    };
    proto_resp.encode_to_vec()
}

pub fn decode_response(buf: &[u8]) -> Result<Response, prost::DecodeError> {
    let proto_resp = pb::Response::decode(buf)?;
    let resp = match proto_resp
        .response
        .ok_or_else(|| prost::DecodeError::new("missing response oneof"))?
    {
        pb::response::Response::CreatePayload(r) => Response::CreatePayload(r.payload),
        pb::response::Response::ValidateBlock(r) => Response::ValidateBlock(r.ok),
        pb::response::Response::ValidateTx(r) => Response::ValidateTx(r.ok),
        pb::response::Response::ExecuteBlock(r) => {
            if r.error.is_empty() {
                let ebr = r.result.map(Into::into).unwrap_or_default();
                Response::ExecuteBlock(Ok(ebr))
            } else {
                Response::ExecuteBlock(Err(r.error))
            }
        }
        pb::response::Response::OnCommit(r) => {
            if r.error.is_empty() {
                Response::OnCommit(Ok(()))
            } else {
                Response::OnCommit(Err(r.error))
            }
        }
        pb::response::Response::OnEvidence(r) => {
            if r.error.is_empty() {
                Response::OnEvidence(Ok(()))
            } else {
                Response::OnEvidence(Err(r.error))
            }
        }
        pb::response::Response::Query(r) => {
            if r.error.is_empty() {
                Response::Query(Ok(r.data))
            } else {
                Response::Query(Err(r.error))
            }
        }
    };
    Ok(resp)
}

/// Write a length-prefixed frame to an async writer.
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

/// Read a length-prefixed frame from an async reader.
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
