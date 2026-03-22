use ruc::*;

use std::collections::{HashMap, HashSet};
use std::iter;
use std::mem;
use std::time::Instant;

use futures::StreamExt;
use hotmint_consensus::network::NetworkSink;
use hotmint_types::sync::{SyncRequest, SyncResponse};
use hotmint_types::{ConsensusMessage, ValidatorId};
use litep2p::config::ConfigBuilder;
use litep2p::protocol::notification::{
    ConfigBuilder as NotifConfigBuilder, NotificationEvent, NotificationHandle, ValidationResult,
};
use litep2p::protocol::request_response::{
    ConfigBuilder as ReqRespConfigBuilder, DialOptions, RequestResponseEvent, RequestResponseHandle,
};
use litep2p::transport::tcp::config::Config as TcpConfig;
use litep2p::types::RequestId;
use litep2p::types::multiaddr::Multiaddr;
use litep2p::{Litep2p, Litep2pEvent, PeerId};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, trace, warn};

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::codec;
use crate::peer::{PeerBook, PeerInfo, PeerRole};
use crate::pex::{PexConfig, PexRequest, PexResponse};

const NOTIF_PROTOCOL: &str = "/hotmint/consensus/notif/1";
const REQ_RESP_PROTOCOL: &str = "/hotmint/consensus/reqresp/1";
const SYNC_PROTOCOL: &str = "/hotmint/sync/1";
const PEX_PROTOCOL: &str = "/hotmint/pex/1";
const MAX_NOTIFICATION_SIZE: usize = 16 * 1024 * 1024;
const MAINTENANCE_INTERVAL_SECS: u64 = 10;

/// Maps ValidatorId <-> PeerId for routing
#[derive(Clone)]
pub struct PeerMap {
    pub validator_to_peer: HashMap<ValidatorId, PeerId>,
    pub peer_to_validator: HashMap<PeerId, ValidatorId>,
}

impl PeerMap {
    pub fn new() -> Self {
        Self {
            validator_to_peer: HashMap::new(),
            peer_to_validator: HashMap::new(),
        }
    }

    pub fn insert(&mut self, vid: ValidatorId, pid: PeerId) {
        self.validator_to_peer.insert(vid, pid);
        self.peer_to_validator.insert(pid, vid);
    }

    pub fn remove(&mut self, vid: ValidatorId) -> Option<PeerId> {
        if let Some(pid) = self.validator_to_peer.remove(&vid) {
            self.peer_to_validator.remove(&pid);
            Some(pid)
        } else {
            None
        }
    }
}

impl Default for PeerMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Status of a peer for external queries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatus {
    pub validator_id: ValidatorId,
    pub peer_id: String,
}

/// Commands sent from the NetworkSink to the NetworkService
pub enum NetCommand {
    Broadcast(Vec<u8>),
    SendTo(ValidatorId, Vec<u8>),
    AddPeer(ValidatorId, PeerId, Vec<Multiaddr>),
    RemovePeer(ValidatorId),
    /// Send a sync request to a specific peer
    SyncRequest(PeerId, Vec<u8>),
    /// Respond to a sync request
    SyncRespond(RequestId, Vec<u8>),
    /// Update peer_map from new validator set (epoch transition)
    EpochChange(Vec<(ValidatorId, hotmint_types::crypto::PublicKey)>),
}

/// Incoming sync request forwarded to the sync responder
pub struct IncomingSyncRequest {
    pub request_id: RequestId,
    pub peer: PeerId,
    pub request: SyncRequest,
}

/// All handles returned by [`NetworkService::create`].
pub struct NetworkServiceHandles {
    pub service: NetworkService,
    pub sink: Litep2pNetworkSink,
    pub msg_rx: mpsc::Receiver<(Option<ValidatorId>, ConsensusMessage)>,
    pub sync_req_rx: mpsc::Receiver<IncomingSyncRequest>,
    pub sync_resp_rx: mpsc::Receiver<SyncResponse>,
    pub peer_info_rx: watch::Receiver<Vec<PeerStatus>>,
    pub connected_count_rx: watch::Receiver<usize>,
    /// Number of peers with an open notification (consensus) substream.
    /// Reaches >0 once the notification handshake completes, which is
    /// later than the raw connection and avoids the need for a fixed sleep.
    pub notif_connected_count_rx: watch::Receiver<usize>,
}

/// NetworkService wraps litep2p and provides consensus-level networking
pub struct NetworkService {
    litep2p: Litep2p,
    notif_handle: NotificationHandle,
    reqresp_handle: RequestResponseHandle,
    sync_handle: RequestResponseHandle,
    pex_handle: RequestResponseHandle,
    peer_map: PeerMap,
    peer_book: Arc<RwLock<PeerBook>>,
    pex_config: PexConfig,
    persistent_peers: HashMap<ValidatorId, PeerId>,
    /// Addresses to dial at startup (from persistent_peers config).
    initial_dial_addresses: Vec<Multiaddr>,
    /// Whether to relay received consensus messages to other connected peers.
    relay_consensus: bool,
    /// Public keys of known validators, used to verify individual message signatures
    /// before relaying.  Updated on epoch transitions via [`NetCommand::EpochChange`].
    validator_keys: HashMap<ValidatorId, hotmint_types::crypto::PublicKey>,
    /// Validators in round-robin order for leader-for-view computation.
    /// Must match the ordering in `ValidatorSet::validators()`.
    validator_ids_ordered: Vec<ValidatorId>,
    /// Blake3 hash of the chain identifier — used for relay signature verification.
    chain_id_hash: [u8; 32],
    msg_tx: mpsc::Sender<(Option<ValidatorId>, ConsensusMessage)>,
    cmd_rx: mpsc::Receiver<NetCommand>,
    sync_req_tx: mpsc::Sender<IncomingSyncRequest>,
    sync_resp_tx: mpsc::Sender<SyncResponse>,
    peer_info_tx: watch::Sender<Vec<PeerStatus>>,
    connected_count_tx: watch::Sender<usize>,
    /// Tracks peers with an open notification substream (post-handshake).
    notif_connected_peers: HashSet<PeerId>,
    notif_connected_count_tx: watch::Sender<usize>,
    connected_peers: HashSet<PeerId>,
    /// Two-set rotation for relay deduplication: when the active set fills up,
    /// swap it to the backup position (clearing the old backup). Messages are
    /// checked against both sets, so recent history is always preserved across
    /// rotations. This avoids the brief relay-window that a single-set clear
    /// would create.
    seen_active: HashSet<u64>,
    seen_backup: HashSet<u64>,
    /// Reliable channel for epoch changes (F-02).
    epoch_rx: watch::Receiver<Option<Vec<(ValidatorId, hotmint_types::crypto::PublicKey)>>>,
    /// Per-peer rate limiting for PEX requests (F-09).
    pex_rate_limit: HashMap<PeerId, Instant>,
}

impl NetworkService {
    /// Create the network service and all handles for the consensus engine.
    pub fn create(
        listen_addr: Multiaddr,
        peer_map: PeerMap,
        known_addresses: Vec<(PeerId, Vec<Multiaddr>)>,
        keypair: Option<litep2p::crypto::ed25519::Keypair>,
        peer_book: Arc<RwLock<PeerBook>>,
        pex_config: PexConfig,
        relay_consensus: bool,
        initial_validators: Vec<(ValidatorId, hotmint_types::crypto::PublicKey)>,
        chain_id_hash: [u8; 32],
    ) -> Result<NetworkServiceHandles> {
        let (notif_config, notif_handle) = NotifConfigBuilder::new(NOTIF_PROTOCOL.into())
            .with_max_size(MAX_NOTIFICATION_SIZE)
            .with_handshake(vec![])
            .with_auto_accept_inbound(true)
            .with_sync_channel_size(1024)
            .with_async_channel_size(1024)
            .build();

        let (reqresp_config, reqresp_handle) = ReqRespConfigBuilder::new(REQ_RESP_PROTOCOL.into())
            .with_max_size(MAX_NOTIFICATION_SIZE)
            .build();

        let (sync_config, sync_handle) = ReqRespConfigBuilder::new(SYNC_PROTOCOL.into())
            .with_max_size(MAX_NOTIFICATION_SIZE)
            .build();

        let (pex_config_proto, pex_handle) = ReqRespConfigBuilder::new(PEX_PROTOCOL.into())
            .with_max_size(1024 * 1024) // 1MB for peer lists
            .build();

        let mut config_builder = ConfigBuilder::new()
            .with_tcp(TcpConfig {
                listen_addresses: vec![listen_addr],
                ..Default::default()
            })
            .with_notification_protocol(notif_config)
            .with_request_response_protocol(reqresp_config)
            .with_request_response_protocol(sync_config)
            .with_request_response_protocol(pex_config_proto);

        if let Some(kp) = keypair {
            config_builder = config_builder.with_keypair(kp);
        }

        // Collect dial addresses with /p2p/<peer_id> suffix for startup dialing
        let initial_dial_addresses: Vec<Multiaddr> = known_addresses
            .iter()
            .flat_map(|(pid, addrs)| {
                addrs.iter().map(move |addr| {
                    let mut full = addr.clone();
                    full.push(litep2p::types::multiaddr::Protocol::P2p((*pid).into()));
                    full
                })
            })
            .collect();

        if !known_addresses.is_empty() {
            config_builder = config_builder.with_known_addresses(known_addresses.into_iter());
        }

        let litep2p =
            Litep2p::new(config_builder.build()).c(d!("failed to create litep2p instance"))?;

        info!(peer_id = %litep2p.local_peer_id(), "litep2p started");
        for addr in litep2p.listen_addresses() {
            info!(address = %addr, "listening on");
        }

        let (msg_tx, msg_rx) = mpsc::channel(8192);
        let (cmd_tx, cmd_rx) = mpsc::channel(4096);
        let (sync_req_tx, sync_req_rx) = mpsc::channel(256);
        let (sync_resp_tx, sync_resp_rx) = mpsc::channel(256);

        // Build initial peer info
        let initial_peers: Vec<PeerStatus> = peer_map
            .validator_to_peer
            .iter()
            .map(|(&vid, pid)| PeerStatus {
                validator_id: vid,
                peer_id: pid.to_string(),
            })
            .collect();
        let (peer_info_tx, peer_info_rx) = watch::channel(initial_peers);

        let (epoch_tx, epoch_rx) =
            watch::channel::<Option<Vec<(ValidatorId, hotmint_types::crypto::PublicKey)>>>(None);

        let sink = Litep2pNetworkSink {
            cmd_tx: cmd_tx.clone(),
            epoch_tx,
        };

        let (connected_count_tx, connected_count_rx) = watch::channel(0usize);
        let (notif_connected_count_tx, notif_connected_count_rx) = watch::channel(0usize);

        // Save persistent peers for auto-reconnect
        let persistent_peers: HashMap<ValidatorId, PeerId> = peer_map.validator_to_peer.clone();

        let validator_ids_ordered: Vec<ValidatorId> =
            initial_validators.iter().map(|(vid, _)| *vid).collect();
        let validator_keys: HashMap<ValidatorId, hotmint_types::crypto::PublicKey> =
            initial_validators.into_iter().collect();

        Ok(NetworkServiceHandles {
            service: Self {
                litep2p,
                notif_handle,
                reqresp_handle,
                sync_handle,
                pex_handle,
                peer_map,
                peer_book,
                pex_config,
                persistent_peers,
                initial_dial_addresses,
                relay_consensus,
                validator_keys,
                validator_ids_ordered,
                chain_id_hash,
                msg_tx,
                cmd_rx,
                sync_req_tx,
                sync_resp_tx,
                peer_info_tx,
                connected_count_tx,
                notif_connected_peers: HashSet::new(),
                notif_connected_count_tx,
                connected_peers: HashSet::new(),
                seen_active: HashSet::new(),
                seen_backup: HashSet::new(),
                epoch_rx,
                pex_rate_limit: HashMap::new(),
            },
            sink,
            msg_rx,
            sync_req_rx,
            sync_resp_rx,
            peer_info_rx,
            connected_count_rx,
            notif_connected_count_rx,
        })
    }

    pub fn local_peer_id(&self) -> &PeerId {
        self.litep2p.local_peer_id()
    }

    /// Run the network event loop
    pub async fn run(mut self) {
        // Dial all persistent peers at startup using full multiaddrs.
        for addr in mem::take(&mut self.initial_dial_addresses) {
            if let Err(e) = self.litep2p.dial_address(addr.clone()).await {
                debug!(address = %addr, error = ?e, "initial dial failed (will retry)");
            }
        }

        let mut maintenance_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(MAINTENANCE_INTERVAL_SECS));
        let mut pex_interval = tokio::time::interval(tokio::time::Duration::from_secs(
            self.pex_config.request_interval_secs,
        ));
        loop {
            tokio::select! {
                event = self.notif_handle.next() => {
                    if let Some(event) = event {
                        self.handle_notification_event(event).await;
                    }
                }
                event = self.reqresp_handle.next() => {
                    if let Some(event) = event {
                        self.handle_reqresp_event(event);
                    }
                }
                event = self.sync_handle.next() => {
                    if let Some(event) = event {
                        self.handle_sync_event(event).await;
                    }
                }
                event = self.pex_handle.next() => {
                    if let Some(event) = event {
                        self.handle_pex_event(event).await;
                    }
                }
                event = self.litep2p.next_event() => {
                    if let Some(event) = event {
                        self.handle_litep2p_event(event).await;
                    }
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                Ok(()) = self.epoch_rx.changed() => {
                    let epoch_val = self.epoch_rx.borrow_and_update().clone();
                    if let Some(validators) = epoch_val {
                        self.handle_epoch_change(validators).await;
                    }
                }
                _ = maintenance_interval.tick() => {
                    self.run_maintenance().await;
                }
                _ = pex_interval.tick() => {
                    if self.pex_config.enabled {
                        self.run_pex_round().await;
                    }
                }
            }
        }
    }

    async fn handle_notification_event(&mut self, event: NotificationEvent) {
        match event {
            NotificationEvent::ValidateSubstream { peer, .. } => {
                self.notif_handle
                    .send_validation_result(peer, ValidationResult::Accept);
            }
            NotificationEvent::NotificationStreamOpened { peer, .. } => {
                info!(peer = %peer, "notification stream opened");
                self.notif_connected_peers.insert(peer);
                let _ = self
                    .notif_connected_count_tx
                    .send(self.notif_connected_peers.len());
            }
            NotificationEvent::NotificationStreamClosed { peer } => {
                debug!(peer = %peer, "notification stream closed");
                self.notif_connected_peers.remove(&peer);
                let _ = self
                    .notif_connected_count_tx
                    .send(self.notif_connected_peers.len());
            }
            NotificationEvent::NotificationReceived { peer, notification } => {
                // Determine the sender ValidatorId (None if peer is not a known validator)
                let sender: Option<ValidatorId> =
                    self.peer_map.peer_to_validator.get(&peer).copied();

                match codec::decode::<ConsensusMessage>(&notification) {
                    Ok(msg) => {
                        // F-08: Only deliver to consensus if sender is a known validator.
                        if sender.is_some()
                            && let Err(e) = self.msg_tx.try_send((sender, msg.clone()))
                        {
                            warn!("consensus message dropped (notification): {e}");
                        }

                        // Relay: re-broadcast to other connected peers (with two-set dedup).
                        // Only relay from known validators whose individual message signature
                        // is valid, preventing unknown peers from using this node as a DoS
                        // amplifier.
                        if self.relay_consensus
                            && let Some(sid) = sender
                            && hotmint_consensus::engine::verify_relay_sender(
                                sid,
                                &msg,
                                &self.validator_keys,
                                &self.validator_ids_ordered,
                                &self.chain_id_hash,
                            )
                        {
                            let msg_hash = u64::from_le_bytes(
                                blake3::hash(&notification).as_bytes()[..8]
                                    .try_into()
                                    .unwrap(),
                            );

                            // Check both sets to avoid re-relay across rotations
                            if !self.seen_active.contains(&msg_hash)
                                && !self.seen_backup.contains(&msg_hash)
                            {
                                self.seen_active.insert(msg_hash);
                                let raw = notification.to_vec();
                                for &other in &self.connected_peers {
                                    if other != peer {
                                        let _ = self
                                            .notif_handle
                                            .send_sync_notification(other, raw.clone());
                                    }
                                }
                                // Rotate: move active→backup, clear old backup
                                if self.seen_active.len() > 10_000 {
                                    self.seen_backup = mem::take(&mut self.seen_active);
                                }
                            }
                            // F-31: Reward peer for a valid relayed consensus message.
                            self.peer_book
                                .write()
                                .await
                                .adjust_score(&peer.to_string(), 1);
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, peer = %peer, "failed to decode notification");
                        self.peer_book
                            .write()
                            .await
                            .adjust_score(&peer.to_string(), -10);
                    }
                }
            }
            NotificationEvent::NotificationStreamOpenFailure { peer, error } => {
                warn!(peer = %peer, error = ?error, "notification stream open failed");
            }
        }
    }

    fn handle_reqresp_event(&mut self, event: RequestResponseEvent) {
        match event {
            RequestResponseEvent::RequestReceived {
                peer,
                request_id,
                request,
                ..
            } => {
                let Some(sender) = self.peer_map.peer_to_validator.get(&peer).copied() else {
                    warn!(peer = %peer, "dropping request from unknown peer");
                    self.reqresp_handle.reject_request(request_id);
                    return;
                };
                match codec::decode::<ConsensusMessage>(&request) {
                    Ok(msg) => {
                        if let Err(e) = self.msg_tx.try_send((Some(sender), msg)) {
                            warn!("consensus message dropped (reqresp): {e}");
                        }
                        self.reqresp_handle.send_response(request_id, vec![]);
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to decode request");
                        self.reqresp_handle.reject_request(request_id);
                    }
                }
            }
            RequestResponseEvent::ResponseReceived { .. } => {}
            RequestResponseEvent::RequestFailed { peer, error, .. } => {
                debug!(peer = %peer, error = ?error, "request failed");
            }
        }
    }

    async fn handle_sync_event(&mut self, event: RequestResponseEvent) {
        match event {
            RequestResponseEvent::RequestReceived {
                peer,
                request_id,
                request,
                ..
            } => {
                if !self.connected_peers.contains(&peer) {
                    warn!(peer = %peer, "rejecting sync request from unconnected peer");
                    self.sync_handle.reject_request(request_id);
                    return;
                }
                match codec::decode::<SyncRequest>(&request) {
                    Ok(req) => {
                        if let Err(e) = self.sync_req_tx.try_send(IncomingSyncRequest {
                            request_id,
                            peer,
                            request: req,
                        }) {
                            warn!("sync request dropped: {e}");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, peer = %peer, "failed to decode sync request");
                        self.peer_book
                            .write()
                            .await
                            .adjust_score(&peer.to_string(), -5);
                        let err_resp = SyncResponse::Error(format!("decode error: {e}"));
                        if let Ok(bytes) = codec::encode(&err_resp) {
                            self.sync_handle.send_response(request_id, bytes);
                        } else {
                            self.sync_handle.reject_request(request_id);
                        }
                    }
                }
            }
            RequestResponseEvent::ResponseReceived {
                request_id: _,
                response,
                ..
            } => {
                // Forward sync response to the sync requester
                match codec::decode::<SyncResponse>(&response) {
                    Ok(resp) => {
                        if let Err(e) = self.sync_resp_tx.try_send(resp) {
                            warn!("sync response dropped: {e}");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to decode sync response");
                    }
                }
            }
            RequestResponseEvent::RequestFailed { peer, error, .. } => {
                debug!(peer = %peer, error = ?error, "sync request failed");
                if let Err(e) = self
                    .sync_resp_tx
                    .try_send(SyncResponse::Error(format!("request failed: {error:?}")))
                {
                    warn!("sync error response dropped: {e}");
                }
            }
        }
    }

    async fn handle_pex_event(&mut self, event: RequestResponseEvent) {
        match event {
            RequestResponseEvent::RequestReceived {
                peer,
                request_id,
                request,
                ..
            } => {
                // P3: Only accept PEX from known peers (peers in peer_map or connected)
                if !self.peer_map.peer_to_validator.contains_key(&peer)
                    && !self.connected_peers.contains(&peer)
                {
                    warn!(peer = %peer, "rejecting PEX request from unknown peer");
                    self.pex_handle.reject_request(request_id);
                    return;
                }
                // F-09: Rate limit PEX requests per peer (max 1 every 10s).
                let now = Instant::now();
                if let Some(last) = self.pex_rate_limit.get(&peer)
                    && now.duration_since(*last) < std::time::Duration::from_secs(10)
                {
                    self.pex_handle.reject_request(request_id);
                    return;
                }
                self.pex_rate_limit.insert(peer, now);
                match serde_cbor_2::from_slice::<PexRequest>(&request) {
                    Ok(PexRequest::GetPeers) => {
                        let book = self.peer_book.read().await;
                        let private = &self.pex_config.private_peer_ids;
                        let peers: Vec<PeerInfo> = book
                            .get_random_peers(self.pex_config.max_peers_per_response)
                            .into_iter()
                            .filter(|p| p.peer_id != peer.to_string())
                            // P4: exclude private peers from PEX responses
                            .filter(|p| !private.contains(&p.peer_id))
                            .cloned()
                            .collect();
                        let resp = PexResponse::Peers(peers);
                        if let Ok(bytes) = serde_cbor_2::to_vec(&resp) {
                            self.pex_handle.send_response(request_id, bytes);
                        }
                    }
                    Ok(PexRequest::Advertise {
                        role,
                        validator_id,
                        addresses,
                    }) => {
                        // F-10: Reject if role is "validator" but validator_id is None.
                        if role == PeerRole::Validator && validator_id.is_none() {
                            warn!(peer = %peer, "PEX Advertise claims validator role without validator_id");
                            self.pex_handle.reject_request(request_id);
                            return;
                        }
                        // P2: If claiming validator_id, verify PeerId matches peer_map
                        if let Some(vid) = validator_id
                            && let Some(&expected_peer) =
                                self.peer_map.validator_to_peer.get(&ValidatorId(vid))
                            && expected_peer != peer
                        {
                            warn!(
                                peer = %peer,
                                claimed_vid = vid,
                                "PEX Advertise validator_id mismatch, rejecting"
                            );
                            self.pex_handle.reject_request(request_id);
                            return;
                        }
                        // F-10: Limit addresses to max 8 to prevent abuse.
                        let addresses: Vec<_> = addresses.iter().take(8).cloned().collect();
                        let mut info = PeerInfo::new(
                            peer,
                            role,
                            addresses.iter().filter_map(|a| a.parse().ok()).collect(),
                        );
                        if let Some(vid) = validator_id {
                            info = info.with_validator(ValidatorId(vid));
                        }
                        self.peer_book.write().await.add_peer(info);
                        if let Ok(bytes) = serde_cbor_2::to_vec(&PexResponse::Ack) {
                            self.pex_handle.send_response(request_id, bytes);
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to decode PEX request");
                        self.pex_handle.reject_request(request_id);
                    }
                }
            }
            RequestResponseEvent::ResponseReceived { response, .. } => {
                if let Ok(PexResponse::Peers(peers)) = serde_cbor_2::from_slice(&response) {
                    let mut book = self.peer_book.write().await;
                    for peer in peers {
                        if !peer.is_banned() {
                            book.add_peer(peer);
                        }
                    }
                }
            }
            // F-27: Log unhandled PEX events instead of silently dropping.
            other => {
                trace!("unhandled PEX event: {other:?}");
            }
        }
    }

    /// Periodic connection maintenance: reconnect persistent peers, save peer book.
    async fn run_maintenance(&mut self) {
        // 1. Reconnect disconnected persistent peers.
        // Collect addresses outside the lock scope to avoid holding it across await.
        let to_dial: Vec<(PeerId, Vec<Multiaddr>)> = {
            let book = self.peer_book.read().await;
            self.persistent_peers
                .values()
                .filter(|pid| !self.connected_peers.contains(pid))
                .filter_map(|&pid| {
                    book.get(&pid.to_string()).map(|info| {
                        let addrs: Vec<Multiaddr> = info
                            .addresses
                            .iter()
                            .filter_map(|a| a.parse().ok())
                            .collect();
                        (pid, addrs)
                    })
                })
                .collect()
        };

        for (pid, addrs) in to_dial {
            if !addrs.is_empty() {
                self.litep2p
                    .add_known_address(pid, addrs.clone().into_iter());
                // F-07: Also actively dial the first address to trigger reconnection.
                let mut dial_addr = addrs[0].clone();
                dial_addr.push(litep2p::types::multiaddr::Protocol::P2p(pid.into()));
                if let Err(e) = self.litep2p.dial_address(dial_addr).await {
                    debug!(peer = %pid, error = ?e, "persistent peer redial failed");
                }
            }
        }

        // 2. Try to connect to peers from book if under target
        let max = self.pex_config.max_peers;
        if self.connected_peers.len() < max * 4 / 5 {
            let book = self.peer_book.read().await;
            let candidates = book.get_random_peers(5);
            for peer in candidates {
                if let Ok(pid) = peer.peer_id.parse::<PeerId>()
                    && !self.connected_peers.contains(&pid)
                {
                    let addrs: Vec<Multiaddr> = peer
                        .addresses
                        .iter()
                        .filter_map(|a| a.parse().ok())
                        .collect();
                    if !addrs.is_empty() {
                        self.litep2p.add_known_address(pid, addrs.into_iter());
                    }
                }
            }
        }

        // 3. Prune stale peers (older than 24 hours) and persist
        self.peer_book.write().await.prune_stale(86400);
        if let Err(e) = self.peer_book.read().await.save() {
            warn!(%e, "failed to save peer book");
        }
    }

    /// Send a PEX GetPeers request to a random connected peer.
    async fn run_pex_round(&mut self) {
        if self.connected_peers.is_empty() {
            return;
        }
        // Pick a random connected peer
        let peers: Vec<PeerId> = self.connected_peers.iter().copied().collect();
        let idx = rand::random::<usize>() % peers.len();
        let target = peers[idx];

        if let Ok(bytes) = serde_cbor_2::to_vec(&PexRequest::GetPeers) {
            let _ = self
                .pex_handle
                .send_request(target, bytes, DialOptions::Reject)
                .await;
        }
    }

    async fn handle_litep2p_event(&mut self, event: Litep2pEvent) {
        match event {
            Litep2pEvent::ConnectionEstablished { peer, endpoint } => {
                // Enforce total connection limit
                if self.connected_peers.len() >= self.pex_config.max_peers {
                    warn!(
                        peer = %peer,
                        total = self.connected_peers.len(),
                        max = self.pex_config.max_peers,
                        "connection limit reached, ignoring new peer"
                    );
                    return;
                }

                info!(peer = %peer, endpoint = ?endpoint, "connection established");
                self.connected_peers.insert(peer);
                let _ = self.connected_count_tx.send(self.connected_peers.len());

                // Open notification substream to this peer for consensus messages.
                if let Err(e) = self.notif_handle.try_open_substream_batch(iter::once(peer)) {
                    debug!(peer = %peer, error = ?e, "failed to open notification substream");
                }

                // Update last_seen in peer book
                if let Some(info) = self.peer_book.write().await.get_mut(&peer.to_string()) {
                    info.touch();
                }
            }
            Litep2pEvent::ConnectionClosed { peer, .. } => {
                debug!(peer = %peer, "connection closed");
                self.connected_peers.remove(&peer);
                let _ = self.connected_count_tx.send(self.connected_peers.len());
                // Eagerly remove from notif set; NotificationStreamClosed may
                // arrive late or not at all when the TCP connection drops first.
                if self.notif_connected_peers.remove(&peer) {
                    let _ = self
                        .notif_connected_count_tx
                        .send(self.notif_connected_peers.len());
                }
            }
            Litep2pEvent::DialFailure { address, error, .. } => {
                warn!(address = %address, error = ?error, "dial failed");
            }
            // F-27: Log unhandled litep2p events instead of silently dropping.
            other => {
                trace!(?other, "unhandled litep2p event");
            }
        }
    }

    fn update_peer_info(&self) {
        let peers: Vec<PeerStatus> = self
            .peer_map
            .validator_to_peer
            .iter()
            .map(|(&vid, pid)| PeerStatus {
                validator_id: vid,
                peer_id: pid.to_string(),
            })
            .collect();
        let _ = self.peer_info_tx.send(peers);
    }

    async fn handle_command(&mut self, cmd: NetCommand) {
        match cmd {
            NetCommand::Broadcast(bytes) => {
                // Broadcast to all connected peers (validators + fullnodes).
                for &peer in &self.connected_peers {
                    let _ = self
                        .notif_handle
                        .send_sync_notification(peer, bytes.clone());
                }
            }
            NetCommand::SendTo(target, bytes) => {
                if let Some(&peer_id) = self.peer_map.validator_to_peer.get(&target) {
                    let _ = self
                        .reqresp_handle
                        .send_request(peer_id, bytes, DialOptions::Reject)
                        .await;
                }
            }
            NetCommand::AddPeer(vid, pid, addrs) => {
                info!(validator = %vid, peer = %pid, "adding peer");
                self.peer_map.insert(vid, pid);
                self.litep2p.add_known_address(pid, addrs.into_iter());
                self.update_peer_info();
            }
            NetCommand::RemovePeer(vid) => {
                if let Some(pid) = self.peer_map.remove(vid) {
                    info!(validator = %vid, peer = %pid, "removed peer");
                } else {
                    warn!(validator = %vid, "peer not found for removal");
                }
                self.update_peer_info();
            }
            NetCommand::SyncRequest(peer_id, bytes) => {
                let _ = self
                    .sync_handle
                    .send_request(peer_id, bytes, DialOptions::Reject)
                    .await;
            }
            NetCommand::SyncRespond(request_id, bytes) => {
                self.sync_handle.send_response(request_id, bytes);
            }
            NetCommand::EpochChange(validators) => {
                self.handle_epoch_change(validators).await;
            }
        }
    }

    async fn handle_epoch_change(
        &mut self,
        validators: Vec<(ValidatorId, hotmint_types::crypto::PublicKey)>,
    ) {
        // Rebuild peer_map entries for new validators using PeerBook
        for (vid, pubkey) in &validators {
            if self.peer_map.validator_to_peer.contains_key(vid) {
                continue;
            }
            // Try to find PeerId from PeerBook by looking up the public key
            let pk_bytes = &pubkey.0;
            if let Ok(lpk) = litep2p::crypto::ed25519::PublicKey::try_from_bytes(pk_bytes) {
                let peer_id = lpk.to_peer_id();
                info!(validator = %vid, peer = %peer_id, "adding new epoch validator to peer_map");
                self.peer_map.insert(*vid, peer_id);
            }
        }
        // Remove validators no longer in the set
        let new_ids: HashSet<ValidatorId> = validators.iter().map(|(vid, _)| *vid).collect();
        let to_remove: Vec<ValidatorId> = self
            .peer_map
            .validator_to_peer
            .keys()
            .filter(|vid| !new_ids.contains(vid))
            .copied()
            .collect();
        for vid in to_remove {
            info!(validator = %vid, "removing validator from peer_map after epoch change");
            self.peer_map.remove(vid);
        }
        // Update relay verification key map to match new epoch
        self.validator_ids_ordered = validators.iter().map(|(vid, _)| *vid).collect();
        self.validator_keys = validators.into_iter().collect();
        // F-28: Rebuild persistent_peers from updated peer_map
        self.persistent_peers = self.peer_map.validator_to_peer.clone();
        self.update_peer_info();
    }
}

/// NetworkSink backed by litep2p, for use by the consensus engine.
/// Also provides methods for peer management and sync.
#[derive(Clone)]
pub struct Litep2pNetworkSink {
    cmd_tx: mpsc::Sender<NetCommand>,
    epoch_tx: watch::Sender<Option<Vec<(ValidatorId, hotmint_types::crypto::PublicKey)>>>,
}

impl Litep2pNetworkSink {
    pub fn add_peer(&self, vid: ValidatorId, pid: PeerId, addrs: Vec<Multiaddr>) {
        if let Err(e) = self.cmd_tx.try_send(NetCommand::AddPeer(vid, pid, addrs)) {
            warn!("add_peer cmd dropped: {e}");
        }
    }

    pub fn remove_peer(&self, vid: ValidatorId) {
        if let Err(e) = self.cmd_tx.try_send(NetCommand::RemovePeer(vid)) {
            warn!("remove_peer cmd dropped: {e}");
        }
    }

    pub fn send_sync_request(&self, peer_id: PeerId, request: &SyncRequest) {
        match codec::encode(request) {
            Ok(bytes) => {
                if let Err(e) = self
                    .cmd_tx
                    .try_send(NetCommand::SyncRequest(peer_id, bytes))
                {
                    warn!("sync request cmd dropped: {e}");
                }
            }
            Err(e) => warn!("sync request encode failed: {e}"),
        }
    }

    pub fn send_sync_response(&self, request_id: RequestId, response: &SyncResponse) {
        match codec::encode(response) {
            Ok(bytes) => {
                if let Err(e) = self
                    .cmd_tx
                    .try_send(NetCommand::SyncRespond(request_id, bytes))
                {
                    warn!("sync response cmd dropped: {e}");
                }
            }
            Err(e) => warn!("sync response encode failed: {e}"),
        }
    }
}

impl NetworkSink for Litep2pNetworkSink {
    fn broadcast(&self, msg: ConsensusMessage) {
        match codec::encode(&msg) {
            Ok(bytes) => {
                if let Err(e) = self.cmd_tx.try_send(NetCommand::Broadcast(bytes)) {
                    warn!("broadcast cmd dropped: {e}");
                }
            }
            Err(e) => warn!("broadcast encode failed: {e}"),
        }
    }

    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage) {
        match codec::encode(&msg) {
            Ok(bytes) => {
                if let Err(e) = self.cmd_tx.try_send(NetCommand::SendTo(target, bytes)) {
                    warn!("send_to cmd dropped for {target}: {e}");
                }
            }
            Err(e) => warn!("send_to encode failed for {target}: {e}"),
        }
    }

    fn on_epoch_change(&self, new_validator_set: &hotmint_types::ValidatorSet) {
        let validators: Vec<_> = new_validator_set
            .validators()
            .iter()
            .map(|v| (v.id, v.public_key.clone()))
            .collect();
        // F-02: Use dedicated watch channel so epoch changes are never dropped.
        let _ = self.epoch_tx.send(Some(validators));
    }
}
