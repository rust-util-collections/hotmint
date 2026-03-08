//! Embedded single-process hotmint node for real P2P cluster testing.
//!
//! Uses the same config files as `hotmint-node` (config.toml, genesis.json,
//! priv_validator_key.json, node_key.json) but embeds a no-op Application
//! directly — no ABCI socket needed.
//!
//! Usage:
//!   cluster-node [--home /path/to/node/home]

use ruc::*;

use std::sync::{Arc, RwLock};

use clap::Parser;
use tokio::sync::watch;
use tracing::{Level, info};

use hotmint::api::rpc::ConsensusStatus;
use hotmint::config::{self, GenesisDoc, NodeConfig, NodeKey, PrivValidatorKey};
use hotmint::consensus::application::{Application, NoopApplication};
use hotmint::consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint::consensus::pacemaker::PacemakerConfig;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::BlockStore;
use hotmint::crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint::network::service::{NetworkService, PeerMap};
use hotmint::prelude::*;
use hotmint::storage::block_store::VsdbBlockStore;
use hotmint::storage::consensus_state::PersistentConsensusState;

#[derive(Parser)]
#[command(name = "cluster-node", about = "Embedded hotmint node (no ABCI)")]
struct Cli {
    /// Path to hotmint home directory.
    #[arg(long)]
    home: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let home = config::resolve_home(cli.home.as_deref());

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    if let Err(e) = run(&home).await {
        eprintln!("Fatal: {e}");
        std::process::exit(1);
    }
}

async fn run(home: &std::path::Path) -> Result<()> {
    let config_dir = home.join("config");
    let data_dir = home.join("data");

    let config =
        NodeConfig::load(&config_dir.join("config.toml")).c(d!("failed to load config.toml"))?;
    let priv_key = PrivValidatorKey::load(&config_dir.join("priv_validator_key.json"))
        .c(d!("failed to load priv_validator_key.json"))?;
    let signing_key = priv_key.to_signing_key()?;
    let node_key =
        NodeKey::load(&config_dir.join("node_key.json")).c(d!("failed to load node_key.json"))?;
    let litep2p_keypair = node_key.to_litep2p_keypair()?;

    let genesis =
        GenesisDoc::load(&config_dir.join("genesis.json")).c(d!("failed to load genesis.json"))?;
    let validator_set = genesis.to_validator_set()?;

    let our_pk_hex = &priv_key.public_key;
    let our_gv = genesis
        .validators
        .iter()
        .find(|v| &v.public_key == our_pk_hex)
        .ok_or_else(|| eg!("this node's public key not found in genesis"))?;
    let our_vid = ValidatorId(our_gv.id);

    info!(
        validator_id = %our_vid,
        validators = validator_set.validator_count(),
        quorum = validator_set.quorum_threshold(),
        "starting embedded cluster node"
    );

    // Storage
    std::fs::create_dir_all(&data_dir).c(d!("create data dir"))?;
    vsdb::vsdb_set_base_dir(&data_dir).c(d!("set vsdb base dir"))?;

    let store: Arc<RwLock<Box<dyn BlockStore>>> =
        Arc::new(RwLock::new(Box::new(VsdbBlockStore::new())));

    // Restore state
    let pcs = PersistentConsensusState::new();
    let mut state = ConsensusState::new(our_vid, validator_set.clone());
    if let Some(view) = pcs.load_current_view() {
        state.current_view = view;
    }
    if let Some(qc) = pcs.load_locked_qc() {
        state.locked_qc = Some(qc);
    }
    if let Some(qc) = pcs.load_highest_qc() {
        state.highest_qc = Some(qc);
    }
    if let Some(h) = pcs.load_last_committed_height() {
        state.last_committed_height = h;
    }
    if let Some(epoch) = pcs.load_current_epoch() {
        state.validator_set = epoch.validator_set.clone();
        state.current_epoch = epoch;
    }

    // Network
    let (peer_map, known_addresses) = if config.p2p.persistent_peers.is_empty() {
        (PeerMap::new(), vec![])
    } else {
        config::parse_persistent_peers(&config.p2p.persistent_peers, &genesis)?
    };

    let listen_addr: litep2p::types::multiaddr::Multiaddr = config
        .p2p
        .laddr
        .parse()
        .c(d!("invalid p2p listen address"))?;

    let hotmint::network::service::NetworkServiceHandles {
        service: network_service,
        sink: network_sink,
        msg_rx,
        sync_req_rx,
        mut sync_resp_rx,
        peer_info_rx,
        connected_count_rx,
    } = {
        let peer_book_path = home.join("data").join("peer_book.json");
        let peer_book = hotmint::network::peer::PeerBook::load(&peer_book_path)
            .unwrap_or_else(|_| hotmint::network::peer::PeerBook::new(&peer_book_path));
        let peer_book = Arc::new(RwLock::new(peer_book));
        NetworkService::create(
            listen_addr,
            peer_map,
            known_addresses,
            Some(litep2p_keypair),
            peer_book,
            {
                let mut pex = config.pex.clone();
                pex.private_peer_ids = config.p2p.private_peer_ids.clone();
                pex
            },
        )?
    };

    // Embedded application with status tracking
    let (status_tx, status_rx) = watch::channel(ConsensusStatus::new(
        0,
        state.last_committed_height.as_u64(),
        state.current_epoch.number.as_u64(),
        validator_set.validator_count(),
        state.current_epoch.start_view.as_u64(),
    ));

    let initial_vs: Vec<hotmint::api::types::ValidatorInfoResponse> = validator_set
        .validators()
        .iter()
        .map(|v| hotmint::api::types::ValidatorInfoResponse {
            id: v.id.0,
            power: v.power,
            public_key: hex::encode(&v.public_key.0),
        })
        .collect();
    let (vs_tx, vs_rx) = watch::channel(initial_vs);

    let status_tx2 = status_tx.clone();
    let app: Box<dyn Application> = Box::new(StatusApp {
        inner: NoopApplication,
        status_tx,
        vs_tx,
    });

    // RPC
    let rpc_state = hotmint::api::rpc::RpcState {
        validator_id: our_vid.0,
        mempool: Arc::new(hotmint::mempool::Mempool::new(
            config.mempool.max_txs,
            config.mempool.max_tx_bytes,
        )),
        status_rx,
        store: store.clone(),
        peer_info_rx,
        validator_set_rx: vs_rx,
        app: None,
    };
    let rpc_server = hotmint::api::rpc::RpcServer::bind(&config.rpc.laddr, rpc_state)
        .await
        .c(d!("failed to bind RPC server"))?;
    info!(rpc_addr = %config.rpc.laddr, "RPC server listening");

    let sync_sink = network_sink.clone();

    tokio::spawn(async move { network_service.run().await });
    tokio::spawn(async move { rpc_server.run().await });

    // Sync responder
    {
        let store = store.clone();
        let sync_status_rx = status_tx2.subscribe();
        let mut sync_req_rx = sync_req_rx;
        let sync_sink = sync_sink.clone();
        tokio::spawn(async move {
            use hotmint_types::sync::{SyncRequest, SyncResponse};
            while let Some(req) = sync_req_rx.recv().await {
                let resp = match req.request {
                    SyncRequest::GetStatus => {
                        let s = *sync_status_rx.borrow();
                        SyncResponse::Status {
                            last_committed_height: Height(s.last_committed_height),
                            current_view: ViewNumber(s.current_view),
                            epoch: EpochNumber(s.epoch_number),
                        }
                    }
                    SyncRequest::GetBlocks {
                        from_height,
                        to_height,
                    } => {
                        let clamped =
                            Height(to_height.as_u64().min(
                                from_height.as_u64() + hotmint_types::sync::MAX_SYNC_BATCH - 1,
                            ));
                        let s = store.read().unwrap();
                        let blocks = s.get_blocks_in_range(from_height, clamped);
                        let blocks_with_qcs: Vec<_> = blocks
                            .into_iter()
                            .map(|b| {
                                let qc = s.get_commit_qc(b.height);
                                (b, qc)
                            })
                            .collect();
                        drop(s);
                        SyncResponse::Blocks(blocks_with_qcs)
                    }
                };
                sync_sink.send_sync_response(req.request_id, &resp);
            }
        });
    }

    // Block sync
    let mut engine_state_epoch = state.current_epoch.clone();
    let mut engine_state_height = state.last_committed_height;

    if !config.p2p.persistent_peers.is_empty() {
        info!("waiting for peer connection before sync...");
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        let mut count_rx = connected_count_rx;
        loop {
            if *count_rx.borrow() > 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                info!("no peers connected within timeout, skipping sync");
                break;
            }
            let _ =
                tokio::time::timeout(tokio::time::Duration::from_millis(500), count_rx.changed())
                    .await;
        }

        if *count_rx.borrow() > 0 {
            let first_peer_vid = config
                .p2p
                .persistent_peers
                .first()
                .and_then(|p| p.split_once('@'))
                .and_then(|(id, _)| id.parse::<u64>().ok())
                .map(ValidatorId);

            if let Some(vid) = first_peer_vid {
                let bridge_sink = sync_sink.clone();
                let (sync_tx, mut sync_bridge_rx) =
                    tokio::sync::mpsc::channel::<hotmint_types::sync::SyncRequest>(16);

                let sync_peer_id =
                    genesis
                        .validators
                        .iter()
                        .find(|v| v.id == vid.0)
                        .and_then(|gv| {
                            let pk = hex::decode(&gv.public_key).ok()?;
                            let lpk =
                                litep2p::crypto::ed25519::PublicKey::try_from_bytes(&pk).ok()?;
                            Some(lpk.to_peer_id())
                        });

                if let Some(peer_id) = sync_peer_id {
                    let bridge = tokio::spawn(async move {
                        while let Some(req) = sync_bridge_rx.recv().await {
                            bridge_sink.send_sync_request(peer_id, &req);
                        }
                    });

                    info!("starting block sync with V{}", vid.0);
                    let sync_app = NoopApplication;
                    let mut sync_store = VsdbBlockStore::new();
                    let mut engine_state_app_hash = hotmint_types::BlockHash::GENESIS;
                    if let Err(e) = hotmint::consensus::sync::sync_to_tip(
                        &mut sync_store,
                        &sync_app,
                        &mut engine_state_epoch,
                        &mut engine_state_height,
                        &mut engine_state_app_hash,
                        &sync_tx,
                        &mut sync_resp_rx,
                    )
                    .await
                    {
                        info!(%e, "sync completed with error, continuing from current state");
                    }
                    bridge.abort();
                }
            }
        }
    }

    // Start consensus
    state.last_committed_height = engine_state_height;
    state.current_epoch = engine_state_epoch;
    state.validator_set = state.current_epoch.validator_set.clone();

    let signer = Ed25519Signer::new(signing_key, our_vid);
    let engine = ConsensusEngine::new(
        state,
        store,
        Box::new(network_sink),
        app,
        Box::new(signer),
        msg_rx,
        EngineConfig {
            verifier: Box::new(Ed25519Verifier),
            pacemaker: Some(PacemakerConfig {
                base_timeout_ms: config.consensus.base_timeout_ms,
                max_timeout_ms: config.consensus.max_timeout_ms,
                backoff_multiplier: config.consensus.backoff_multiplier,
            }),
            persistence: Some(Box::new(pcs)),
        },
    );

    info!("consensus engine starting");
    engine.run().await;
    Ok(())
}

/// No-op application with RPC status updates on commit.
struct StatusApp<A: Application> {
    inner: A,
    status_tx: watch::Sender<ConsensusStatus>,
    vs_tx: watch::Sender<Vec<hotmint::api::types::ValidatorInfoResponse>>,
}

impl<A: Application> Application for StatusApp<A> {
    fn create_payload(&self, ctx: &hotmint_types::context::BlockContext) -> Vec<u8> {
        self.inner.create_payload(ctx)
    }
    fn validate_block(&self, block: &Block, ctx: &hotmint_types::context::BlockContext) -> bool {
        self.inner.validate_block(block, ctx)
    }
    fn validate_tx(&self, tx: &[u8], ctx: Option<&hotmint_types::context::TxContext>) -> bool {
        self.inner.validate_tx(tx, ctx)
    }
    fn execute_block(
        &self,
        txs: &[&[u8]],
        ctx: &hotmint_types::context::BlockContext,
    ) -> ruc::Result<hotmint_types::EndBlockResponse> {
        self.inner.execute_block(txs, ctx)
    }
    fn on_commit(
        &self,
        block: &Block,
        ctx: &hotmint_types::context::BlockContext,
    ) -> ruc::Result<()> {
        self.inner.on_commit(block, ctx)?;
        let _ = self.status_tx.send(ConsensusStatus::new(
            ctx.view.as_u64(),
            ctx.height.as_u64(),
            ctx.epoch.as_u64(),
            ctx.validator_set.validator_count(),
            ctx.epoch_start_view.as_u64(),
        ));
        let vs: Vec<hotmint::api::types::ValidatorInfoResponse> = ctx
            .validator_set
            .validators()
            .iter()
            .map(|v| hotmint::api::types::ValidatorInfoResponse {
                id: v.id.0,
                power: v.power,
                public_key: hex::encode(&v.public_key.0),
            })
            .collect();
        let _ = self.vs_tx.send(vs);
        Ok(())
    }
    fn on_evidence(&self, proof: &EquivocationProof) -> ruc::Result<()> {
        self.inner.on_evidence(proof)
    }
    fn query(&self, path: &str, data: &[u8]) -> ruc::Result<Vec<u8>> {
        self.inner.query(path, data)
    }
}
