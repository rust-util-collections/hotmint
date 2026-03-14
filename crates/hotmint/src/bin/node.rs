use ruc::*;

use std::sync::{Arc, RwLock};

use clap::{Parser, Subcommand};
use tokio::sync::watch;
use tracing::{Level, info};

use hotmint::abci::client::IpcApplicationClient;
use hotmint::api::rpc::ConsensusStatus;
use hotmint::config::{self, GenesisDoc, NodeConfig, NodeKey, PrivValidatorKey};
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
    /// Generate a new validator key (Ed25519 signing keypair).
    GenValidatorKey {
        /// Output file path (default: priv_validator_key.json).
        #[arg(short, long, default_value = "priv_validator_key.json")]
        output: String,
        /// Validator ID to assign (default: 0).
        #[arg(long, default_value_t = 0)]
        validator_id: u64,
    },
    /// Generate a new node key (Ed25519 P2P identity keypair).
    GenNodeKey {
        /// Output file path (default: node_key.json).
        #[arg(short, long, default_value = "node_key.json")]
        output: String,
    },
    /// Display validator info from an existing key file.
    ShowValidator {
        /// Path to priv_validator_key.json.
        #[arg(short, long, default_value = "priv_validator_key.json")]
        file: String,
    },
    /// Display node ID (PeerId) from an existing node key file.
    ShowNodeId {
        /// Path to node_key.json.
        #[arg(short, long, default_value = "node_key.json")]
        file: String,
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
        Command::GenValidatorKey {
            output,
            validator_id,
        } => {
            let mut key = PrivValidatorKey::generate();
            key.validator_id = validator_id;
            let path = std::path::Path::new(&output);
            if let Err(e) = key.save(path) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            println!("Generated validator key:");
            println!("  Validator ID: {}", key.validator_id);
            println!("  Public key:   {}", key.public_key);
            println!("  File:         {}", path.display());
        }
        Command::GenNodeKey { output } => {
            let key = NodeKey::generate();
            let path = std::path::Path::new(&output);
            if let Err(e) = key.save(path) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            println!("Generated node key:");
            println!("  Public key: {}", key.public_key);
            match key.peer_id() {
                Ok(pid) => println!("  Peer ID:    {}", pid),
                Err(e) => eprintln!("  Peer ID:    (error: {e})"),
            }
            println!("  File:       {}", path.display());
        }
        Command::ShowValidator { file } => {
            let path = std::path::Path::new(&file);
            match PrivValidatorKey::load(path) {
                Ok(key) => {
                    println!("Validator key: {}", path.display());
                    println!("  Validator ID: {}", key.validator_id);
                    println!("  Public key:   {}", key.public_key);
                    match key.peer_id() {
                        Ok(pid) => println!("  Peer ID:      {}", pid),
                        Err(e) => eprintln!("  Peer ID:      (error: {e})"),
                    }
                }
                Err(e) => {
                    eprintln!("Error loading {}: {e}", path.display());
                    std::process::exit(1);
                }
            }
        }
        Command::ShowNodeId { file } => {
            let path = std::path::Path::new(&file);
            match NodeKey::load(path) {
                Ok(key) => {
                    println!("Node key: {}", path.display());
                    println!("  Public key: {}", key.public_key);
                    match key.peer_id() {
                        Ok(pid) => println!("  Peer ID:    {}", pid),
                        Err(e) => eprintln!("  Peer ID:    (error: {e})"),
                    }
                }
                Err(e) => {
                    eprintln!("Error loading {}: {e}", path.display());
                    std::process::exit(1);
                }
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

    // 2. Load validator key (consensus signing) and node key (P2P identity)
    let priv_key = PrivValidatorKey::load(&config_dir.join("priv_validator_key.json"))
        .c(d!("failed to load priv_validator_key.json"))?;
    let signing_key = priv_key.to_signing_key()?;
    let node_key =
        NodeKey::load(&config_dir.join("node_key.json")).c(d!("failed to load node_key.json"))?;
    let litep2p_keypair = node_key.to_litep2p_keypair()?;

    // 3. Load genesis
    let genesis =
        GenesisDoc::load(&config_dir.join("genesis.json")).c(d!("failed to load genesis.json"))?;
    let validator_set = genesis.to_validator_set()?;

    // 4. Find our validator ID.
    // If not in genesis, assign a sentinel ID — the node runs as a fullnode
    // (observes consensus, syncs blocks, serves RPC) but does not vote or propose.
    // If later added to the validator set via epoch transition, it automatically
    // begins participating in consensus.
    let our_pk_hex = &priv_key.public_key;
    let is_fullnode;
    let our_vid = if let Some(gv) = genesis
        .validators
        .iter()
        .find(|v| &v.public_key == our_pk_hex)
    {
        is_fullnode = config.node.mode == "fullnode";
        ValidatorId(gv.id)
    } else {
        is_fullnode = true;
        // Use u64::MAX as sentinel to avoid collision with any real validator ID.
        ValidatorId(u64::MAX)
    };

    if is_fullnode {
        info!(
            node_id = our_vid.0,
            validators = validator_set.validator_count(),
            "starting hotmint fullnode (sync-only, no consensus participation)"
        );
    } else {
        info!(
            validator_id = %our_vid,
            validators = validator_set.validator_count(),
            quorum = validator_set.quorum_threshold(),
            "starting hotmint validator node"
        );
    }

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
    if let Some(epoch) = pcs.load_current_epoch() {
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
        let peer_book = std::sync::Arc::new(std::sync::RwLock::new(peer_book));
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
            config.node.relay_consensus,
        )?
    };

    // 8. Create application (ABCI client or embedded noop for fullnode)
    let use_abci = !is_fullnode || !config.proxy_app.is_empty();
    let (app_box, sync_app_box): (Box<dyn Application>, Box<dyn Application>) = if use_abci {
        let proxy_path = config
            .proxy_app
            .strip_prefix("unix://")
            .unwrap_or(&config.proxy_app);
        let ipc_client = IpcApplicationClient::new(proxy_path);
        ipc_client.check_connection().c(d!(
            "ABCI application not reachable at '{}' — start your application first",
            proxy_path
        ))?;
        let ipc_client_for_sync = IpcApplicationClient::new(proxy_path);
        (Box::new(ipc_client), Box::new(ipc_client_for_sync))
    } else {
        info!("fullnode mode: using embedded no-op application");
        (
            Box::new(hotmint::consensus::application::NoopApplication),
            Box::new(hotmint::consensus::application::NoopApplication),
        )
    };
    let mut engine_state_epoch = state.current_epoch.clone();
    let mut engine_state_height = state.last_committed_height;

    // 9. Wrap with status channel for RPC
    let (status_tx, status_rx) = watch::channel(ConsensusStatus::new(
        0,
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

    let app: Arc<dyn Application> = Arc::new(AppWithStatus {
        inner: app_box,
        status_tx,
        vs_tx,
    });

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
        app: Some(app.clone()),
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
                        // Clamp range to MAX_SYNC_BATCH to prevent DoS
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

    // 14. Sync catch-up before starting consensus (if peers configured)
    if !config.p2p.persistent_peers.is_empty() {
        use hotmint_types::sync::SyncRequest;

        info!("waiting for peer connection before sync...");
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(15);
        let mut count_rx = connected_count_rx;
        loop {
            if *count_rx.borrow() > 0 {
                // Wait for subprotocol handshakes to complete after transport connects
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
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
            // Collect all peers' PeerIds from persistent_peers config
            let sync_peers: Vec<(ValidatorId, litep2p::PeerId)> = config
                .p2p
                .persistent_peers
                .iter()
                .filter_map(|p| {
                    let (id_str, _) = p.split_once('@')?;
                    let vid = ValidatorId(id_str.parse::<u64>().ok()?);
                    let peer_id = genesis
                        .validators
                        .iter()
                        .find(|v| v.id == vid.0)
                        .and_then(|gv| {
                            let pk = hex::decode(&gv.public_key).ok()?;
                            let lpk =
                                litep2p::crypto::ed25519::PublicKey::try_from_bytes(&pk).ok()?;
                            Some(lpk.to_peer_id())
                        })?;
                    Some((vid, peer_id))
                })
                .collect();

            // Try syncing from each peer in order until one succeeds.
            // Use the main store so synced blocks are available for:
            // - accurate view estimation after sync
            // - serving sync requests to other nodes
            let mut synced = false;
            let mut engine_state_app_hash = hotmint_types::BlockHash::GENESIS;
            for (vid, peer_id) in &sync_peers {
                let bridge_sink = sync_sink.clone();
                let pid = *peer_id;
                let (sync_tx, mut sync_bridge_rx) =
                    tokio::sync::mpsc::channel::<SyncRequest>(16);

                let bridge = tokio::spawn(async move {
                    while let Some(req) = sync_bridge_rx.recv().await {
                        bridge_sink.send_sync_request(pid, &req);
                    }
                });

                // Drain any residual responses from a previous peer iteration
                while sync_resp_rx.try_recv().is_ok() {}

                info!("starting block sync with V{}", vid.0);
                let mut store_guard = store.write().unwrap();
                match hotmint::consensus::sync::sync_to_tip(
                    store_guard.as_mut(),
                    sync_app_box.as_ref(),
                    &mut engine_state_epoch,
                    &mut engine_state_height,
                    &mut engine_state_app_hash,
                    &sync_tx,
                    &mut sync_resp_rx,
                )
                .await
                {
                    Ok(()) => {
                        drop(store_guard);
                        bridge.abort();
                        synced = true;
                        break;
                    }
                    Err(e) => {
                        drop(store_guard);
                        info!(%e, peer = vid.0, "sync from peer failed, trying next");
                        bridge.abort();
                    }
                }
            }
            if !synced && !sync_peers.is_empty() {
                info!("all sync peers failed, continuing from current state");
            }
        }
    }

    // 15. Write back synced state and create consensus engine
    // (Engine is created after sync so it starts with up-to-date state)
    state.last_committed_height = engine_state_height;
    state.current_epoch = engine_state_epoch;
    state.validator_set = state.current_epoch.validator_set.clone();

    // Advance current_view to match synced state so the engine joins the correct view.
    // Read the actual block.view from the last committed block (accurate even when
    // the network experienced view timeouts where view >> height).
    if engine_state_height > Height::GENESIS {
        let synced_view = {
            let s = store.read().unwrap();
            s.get_block_by_height(engine_state_height)
                .map(|b| ViewNumber(b.view.as_u64() + 1))
                .unwrap_or(ViewNumber(engine_state_height.as_u64() + 1))
        };
        if synced_view > state.current_view {
            info!(
                synced_view = synced_view.as_u64(),
                "advancing view to match synced state"
            );
            state.current_view = synced_view;
        }
    }

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
        Box::new(ArcApp(app)),
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

/// Wrapper that implements `Application` by delegating to an inner application,
/// while also updating the RPC status watch channel on each commit.
struct AppWithStatus {
    inner: Box<dyn Application>,
    status_tx: watch::Sender<ConsensusStatus>,
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
        let _ = self.status_tx.send(ConsensusStatus::new(
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

/// Newtype wrapper to use `Arc<dyn Application>` as `Box<dyn Application>`.
struct ArcApp(Arc<dyn Application>);

impl Application for ArcApp {
    fn create_payload(&self, ctx: &BlockContext) -> Vec<u8> {
        self.0.create_payload(ctx)
    }
    fn validate_block(&self, block: &Block, ctx: &BlockContext) -> bool {
        self.0.validate_block(block, ctx)
    }
    fn validate_tx(&self, tx: &[u8], ctx: Option<&hotmint_types::context::TxContext>) -> bool {
        self.0.validate_tx(tx, ctx)
    }
    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        self.0.execute_block(txs, ctx)
    }
    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()> {
        self.0.on_commit(block, ctx)
    }
    fn on_evidence(&self, proof: &EquivocationProof) -> Result<()> {
        self.0.on_evidence(proof)
    }
    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        self.0.query(path, data)
    }
}
