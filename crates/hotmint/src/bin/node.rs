use ruc::*;

use std::sync::{Arc, RwLock};

use clap::{Parser, Subcommand};
use tokio::sync::watch;
use tracing::{Level, info};

use hotmint::abci::client::IpcApplicationClient;
use hotmint::config::{self, GenesisDoc, NodeConfig, PrivValidatorKey};
use hotmint::consensus::application::Application;
use hotmint::consensus::engine::{ConsensusEngine, EngineConfig};
use hotmint::consensus::pacemaker::PacemakerConfig;
use hotmint::consensus::state::ConsensusState;
use hotmint::consensus::store::BlockStore;
use hotmint::crypto::{Ed25519Signer, Ed25519Verifier};
use hotmint::mempool::Mempool;
use hotmint::network::service::{NetworkService, PeerMap};
use hotmint::prelude::*;
use hotmint::storage::block_store::VsdbBlockStore;
use hotmint::storage::consensus_state::PersistentConsensusState;

#[derive(Parser)]
#[command(name = "hotmint-node", about = "Hotmint BFT consensus node")]
struct Cli {
    /// Path to hotmint home directory.
    #[arg(long, global = true)]
    home: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new node directory.
    Init,
    /// Run the consensus node.
    Node {
        /// Override ABCI application socket path.
        #[arg(long)]
        proxy_app: Option<String>,
        /// Override P2P listen address (multiaddr).
        #[arg(long)]
        p2p_laddr: Option<String>,
        /// Override JSON-RPC listen address.
        #[arg(long)]
        rpc_laddr: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let home = config::resolve_home(cli.home.as_deref());

    match cli.command {
        Command::Init => {
            if let Err(e) = config::init_node_dir(&home) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Command::Node {
            proxy_app,
            p2p_laddr,
            rpc_laddr,
        } => {
            tracing_subscriber::fmt()
                .with_max_level(Level::INFO)
                .with_target(false)
                .init();

            if let Err(e) = run_node(&home, proxy_app, p2p_laddr, rpc_laddr).await {
                eprintln!("Fatal: {e}");
                std::process::exit(1);
            }
        }
    }
}

async fn run_node(
    home: &std::path::Path,
    proxy_app_override: Option<String>,
    p2p_laddr_override: Option<String>,
    rpc_laddr_override: Option<String>,
) -> Result<()> {
    let config_dir = home.join("config");
    let data_dir = home.join("data");

    // 1. Load config
    let mut config = NodeConfig::load(&config_dir.join("config.toml")).c(d!(
        "failed to load config.toml — run `hotmint-node init` first"
    ))?;

    // Apply CLI overrides
    if let Some(pa) = proxy_app_override {
        config.proxy_app = pa;
    }
    if let Some(pl) = p2p_laddr_override {
        config.p2p.laddr = pl;
    }
    if let Some(rl) = rpc_laddr_override {
        config.rpc.laddr = rl;
    }

    // 2. Load private key
    let priv_key = PrivValidatorKey::load(&config_dir.join("priv_validator_key.json"))
        .c(d!("failed to load priv_validator_key.json"))?;
    let signing_key = priv_key.to_signing_key()?;
    let litep2p_keypair = priv_key.to_litep2p_keypair()?;

    // 3. Load genesis
    let genesis =
        GenesisDoc::load(&config_dir.join("genesis.json")).c(d!("failed to load genesis.json"))?;
    let validator_set = genesis.to_validator_set()?;

    // 4. Find our validator ID
    let our_pk_hex = &priv_key.public_key;
    let our_gv = genesis
        .validators
        .iter()
        .find(|v| &v.public_key == our_pk_hex)
        .ok_or_else(|| eg!("this node's public key not found in genesis validator set"))?;
    let our_vid = ValidatorId(our_gv.id);

    info!(
        validator_id = %our_vid,
        validators = validator_set.validator_count(),
        quorum = validator_set.quorum_threshold(),
        "starting hotmint node"
    );

    // 5. Set up persistent storage
    std::fs::create_dir_all(&data_dir).c(d!("create data dir"))?;
    vsdb::vsdb_set_base_dir(&data_dir).c(d!("set vsdb base dir"))?;

    let store: Arc<RwLock<Box<dyn BlockStore>>> =
        Arc::new(RwLock::new(Box::new(VsdbBlockStore::new())));

    // 6. Restore consensus state
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
    if let Some(mut epoch) = pcs.load_current_epoch() {
        // rebuild_index is needed because ValidatorSet.index_map is #[serde(skip)]
        epoch.validator_set.rebuild_index();
        state.validator_set = epoch.validator_set.clone();
        state.current_epoch = epoch;
    }

    // 7. Parse peers and create network
    let (peer_map, known_addresses) = if config.p2p.persistent_peers.is_empty() {
        (PeerMap::new(), vec![])
    } else {
        config::parse_persistent_peers(&config.p2p.persistent_peers, &genesis)?
    };

    let listen_addr: litep2p::types::multiaddr::Multiaddr = config
        .p2p
        .laddr
        .parse()
        .c(d!("invalid p2p listen address: {}", config.p2p.laddr))?;

    let (
        network_service,
        network_sink,
        msg_rx,
        sync_req_rx,
        mut sync_resp_rx,
        peer_info_rx,
        connected_count_rx,
    ) = {
        let peer_book_path = home.join("data").join("peer_book.json");
        let peer_book = hotmint::network::peer::PeerBook::load(&peer_book_path)
            .unwrap_or_else(|_| hotmint::network::peer::PeerBook::new(&peer_book_path));
        let peer_book = std::sync::Arc::new(std::sync::RwLock::new(peer_book));
        NetworkService::create(
            listen_addr,
            peer_map,
            known_addresses,
            Some(litep2p_keypair),
            peer_book,
            config.pex.clone(),
        )?
    };

    // 8. Create ABCI application client and verify connectivity
    let proxy_path = config
        .proxy_app
        .strip_prefix("unix://")
        .unwrap_or(&config.proxy_app);
    let ipc_client = IpcApplicationClient::new(proxy_path);
    ipc_client.check_connection().c(d!(
        "ABCI application not reachable at '{}' — start your application first",
        proxy_path
    ))?;

    // Save state for sync (before engine consumes everything)
    let ipc_client_for_sync = IpcApplicationClient::new(proxy_path);
    let mut engine_state_epoch = state.current_epoch.clone();
    let mut engine_state_height = state.last_committed_height;

    // 9. Wrap with status channel for RPC
    let (status_tx, status_rx) = watch::channel((
        0u64,
        state.last_committed_height.as_u64(),
        state.current_epoch.number.as_u64(),
        validator_set.validator_count(),
        state.current_epoch.start_view.as_u64(),
    ));
    let sync_status_rx = status_tx.subscribe();

    // Validator set watch channel (updated on epoch transitions via on_commit)
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

    let app = AppWithStatus {
        inner: ipc_client,
        status_tx,
        vs_tx,
    };

    // 10. Create mempool
    let mempool = Arc::new(Mempool::new(
        config.mempool.max_txs,
        config.mempool.max_tx_bytes,
    ));

    // 11. Create RPC server
    let rpc_state = hotmint::api::rpc::RpcState {
        validator_id: our_vid.0,
        mempool: mempool.clone(),
        status_rx,
        store: store.clone(),
        peer_info_rx,
        validator_set_rx: vs_rx,
    };
    let rpc_server = hotmint::api::rpc::RpcServer::bind(&config.rpc.laddr, rpc_state)
        .await
        .c(d!("failed to bind RPC server"))?;

    info!(rpc_addr = %config.rpc.laddr, "RPC server listening");

    let sync_sink = network_sink.clone();

    // 12. Spawn network + RPC before sync (sync needs the network running)
    tokio::spawn(async move { network_service.run().await });
    tokio::spawn(async move { rpc_server.run().await });

    // Sync responder: answer incoming sync requests from peers
    {
        let store = store.clone();
        let sync_status_rx = sync_status_rx;
        let mut sync_req_rx = sync_req_rx;
        let sync_sink = sync_sink.clone();
        tokio::spawn(async move {
            use hotmint_types::sync::{SyncRequest, SyncResponse};
            while let Some(req) = sync_req_rx.recv().await {
                let resp = match req.request {
                    SyncRequest::GetStatus => {
                        let (view, height, epoch, _, _) = *sync_status_rx.borrow();
                        SyncResponse::Status {
                            last_committed_height: Height(height),
                            current_view: ViewNumber(view),
                            epoch: EpochNumber(epoch),
                        }
                    }
                    SyncRequest::GetBlocks {
                        from_height,
                        to_height,
                    } => {
                        // Clamp range to MAX_SYNC_BATCH to prevent DoS
                        let clamped =
                            Height(to_height.as_u64().min(
                                from_height.as_u64() + hotmint_types::sync::MAX_SYNC_BATCH - 1,
                            ));
                        let blocks = store
                            .read()
                            .unwrap()
                            .get_blocks_in_range(from_height, clamped);
                        SyncResponse::Blocks(blocks)
                    }
                };
                sync_sink.send_sync_response(req.request_id, &resp);
            }
        });
    }

    // 14. Sync catch-up before starting consensus (if peers configured)
    if !config.p2p.persistent_peers.is_empty() {
        use hotmint_types::sync::SyncRequest;

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
            // Find first peer's PeerId for sync requests
            let first_peer_vid = config
                .p2p
                .persistent_peers
                .first()
                .and_then(|p| p.split_once('@'))
                .and_then(|(id, _)| id.parse::<u64>().ok())
                .map(ValidatorId);

            if let Some(vid) = first_peer_vid {
                let bridge_sink = sync_sink.clone();
                let (sync_tx, mut sync_bridge_rx) = tokio::sync::mpsc::channel::<SyncRequest>(16);

                // Look up PeerId from genesis
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
                    // Bridge task: forward SyncRequest to network
                    let bridge = tokio::spawn(async move {
                        while let Some(req) = sync_bridge_rx.recv().await {
                            bridge_sink.send_sync_request(peer_id, &req);
                        }
                    });

                    info!("starting block sync with V{}", vid.0);
                    let mut sync_store = VsdbBlockStore::new();
                    if let Err(e) = hotmint::consensus::sync::sync_to_tip(
                        &mut sync_store,
                        &ipc_client_for_sync,
                        &mut engine_state_epoch,
                        &mut engine_state_height,
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

    // 15. Write back synced state and create consensus engine
    // (Engine is created after sync so it starts with up-to-date state)
    state.last_committed_height = engine_state_height;
    state.current_epoch = engine_state_epoch;
    state.validator_set = state.current_epoch.validator_set.clone();

    let signer = Ed25519Signer::new(signing_key, our_vid);
    let pacemaker_config = PacemakerConfig {
        base_timeout_ms: config.consensus.base_timeout_ms,
        max_timeout_ms: config.consensus.max_timeout_ms,
        backoff_multiplier: config.consensus.backoff_multiplier,
    };
    let engine = ConsensusEngine::new(
        state,
        store.clone(),
        Box::new(network_sink),
        Box::new(app),
        Box::new(signer),
        msg_rx,
        EngineConfig {
            verifier: Box::new(Ed25519Verifier),
            pacemaker: Some(pacemaker_config),
            persistence: Some(Box::new(pcs)),
        },
    );

    info!("consensus engine starting");

    // 16. Run consensus engine (blocks forever)
    engine.run().await;

    Ok(())
}

/// Wrapper that implements `Application` by delegating to the IPC client,
/// while also updating the RPC status watch channel on each commit.
struct AppWithStatus {
    inner: IpcApplicationClient,
    status_tx: watch::Sender<hotmint::api::rpc::StatusTuple>,
    vs_tx: watch::Sender<Vec<hotmint::api::types::ValidatorInfoResponse>>,
}

impl Application for AppWithStatus {
    fn create_payload(&self, ctx: &BlockContext) -> Vec<u8> {
        self.inner.create_payload(ctx)
    }

    fn validate_block(&self, block: &Block, ctx: &BlockContext) -> bool {
        self.inner.validate_block(block, ctx)
    }

    fn validate_tx(&self, tx: &[u8], ctx: Option<&hotmint_types::context::TxContext>) -> bool {
        self.inner.validate_tx(tx, ctx)
    }

    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        self.inner.execute_block(txs, ctx)
    }

    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()> {
        self.inner.on_commit(block, ctx)?;
        let _ = self.status_tx.send((
            ctx.view.as_u64(),
            ctx.height.as_u64(),
            ctx.epoch.as_u64(),
            ctx.validator_set.validator_count(),
            ctx.epoch_start_view.as_u64(),
        ));
        // Update validator set for RPC
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

    fn on_evidence(&self, proof: &EquivocationProof) -> Result<()> {
        self.inner.on_evidence(proof)
    }

    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        self.inner.query(path, data)
    }
}
