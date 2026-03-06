use std::sync::Arc;

use hotmint_api::rpc::{ConsensusStatus, RpcServer, RpcState};
use hotmint_api::types::RpcResponse;
use hotmint_consensus::store::MemoryBlockStore;
use hotmint_mempool::Mempool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::watch;

async fn setup_server() -> (String, tokio::task::JoinHandle<()>) {
    let mempool = Arc::new(Mempool::new(100, 1024));
    let (_status_tx, status_rx) = watch::channel(ConsensusStatus::new(1, 0, 0, 4, 0));
    let store = MemoryBlockStore::new_shared();
    let (_peer_tx, peer_info_rx) = watch::channel(vec![]);
    let (_vs_tx, validator_set_rx) = watch::channel(vec![]);

    let state = RpcState {
        validator_id: 42,
        mempool,
        status_rx,
        store,
        peer_info_rx,
        validator_set_rx,
        app: None,
    };

    let server = RpcServer::bind("127.0.0.1:0", state).await.unwrap();
    let addr = format!("{}", server.local_addr());
    let handle = tokio::spawn(async move { server.run().await });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    (addr, handle)
}

#[tokio::test]
async fn test_rpc_status() {
    let (addr, handle) = setup_server().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let req = r#"{"method":"status","params":null,"id":1}"#;
    stream
        .write_all(format!("{req}\n").as_bytes())
        .await
        .unwrap();

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&line).unwrap();
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["validator_id"], 42);
    assert_eq!(result["current_view"], 1);
    assert_eq!(result["mempool_size"], 0);

    handle.abort();
}

#[tokio::test]
async fn test_rpc_submit_tx() {
    let (addr, handle) = setup_server().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let req = r#"{"method":"submit_tx","params":"deadbeef","id":2}"#;
    stream
        .write_all(format!("{req}\n").as_bytes())
        .await
        .unwrap();

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&line).unwrap();
    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["accepted"], true);

    handle.abort();
}

#[tokio::test]
async fn test_rpc_invalid_method() {
    let (addr, handle) = setup_server().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let req = r#"{"method":"nonexistent","params":null,"id":3}"#;
    stream
        .write_all(format!("{req}\n").as_bytes())
        .await
        .unwrap();

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&line).unwrap();
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);

    handle.abort();
}

#[tokio::test]
async fn test_rpc_invalid_json() {
    let (addr, handle) = setup_server().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    stream.write_all(b"not json\n").await.unwrap();

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&line).unwrap();
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32700);

    handle.abort();
}

#[tokio::test]
async fn test_rpc_get_block() {
    let (addr, handle) = setup_server().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    // Genesis block is at height 0
    let req = r#"{"method":"get_block","params":{"height":0},"id":5}"#;
    stream
        .write_all(format!("{req}\n").as_bytes())
        .await
        .unwrap();

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&line).unwrap();
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["height"], 0);

    handle.abort();
}

#[tokio::test]
async fn test_rpc_get_block_not_found() {
    let (addr, handle) = setup_server().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let req = r#"{"method":"get_block","params":{"height":999},"id":6}"#;
    stream
        .write_all(format!("{req}\n").as_bytes())
        .await
        .unwrap();

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&line).unwrap();
    assert!(resp.error.is_some());

    handle.abort();
}

#[tokio::test]
async fn test_rpc_get_epoch() {
    let (addr, handle) = setup_server().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let req = r#"{"method":"get_epoch","params":null,"id":7}"#;
    stream
        .write_all(format!("{req}\n").as_bytes())
        .await
        .unwrap();

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&line).unwrap();
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["number"], 0);
    assert_eq!(result["validator_count"], 4);

    handle.abort();
}

#[tokio::test]
async fn test_rpc_get_peers() {
    let (addr, handle) = setup_server().await;

    let mut stream = TcpStream::connect(&addr).await.unwrap();
    let req = r#"{"method":"get_peers","params":null,"id":8}"#;
    stream
        .write_all(format!("{req}\n").as_bytes())
        .await
        .unwrap();

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let resp: RpcResponse = serde_json::from_str(&line).unwrap();
    assert!(resp.error.is_none());
    // Empty peer list in test
    assert!(resp.result.unwrap().as_array().unwrap().is_empty());

    handle.abort();
}
