use ruc::*;

use std::collections::HashMap;

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
use tracing::{debug, info, warn};

const NOTIF_PROTOCOL: &str = "/hotmint/consensus/notif/1";
const REQ_RESP_PROTOCOL: &str = "/hotmint/consensus/reqresp/1";
const SYNC_PROTOCOL: &str = "/hotmint/sync/1";
const MAX_NOTIFICATION_SIZE: usize = 16 * 1024 * 1024;

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
}

/// Incoming sync request forwarded to the sync responder
pub struct IncomingSyncRequest {
    pub request_id: RequestId,
    pub peer: PeerId,
    pub request: SyncRequest,
}

/// NetworkService wraps litep2p and provides consensus-level networking
pub struct NetworkService {
    litep2p: Litep2p,
    notif_handle: NotificationHandle,
    reqresp_handle: RequestResponseHandle,
    sync_handle: RequestResponseHandle,
    peer_map: PeerMap,
    msg_tx: mpsc::UnboundedSender<(ValidatorId, ConsensusMessage)>,
    cmd_rx: mpsc::UnboundedReceiver<NetCommand>,
    sync_req_tx: mpsc::UnboundedSender<IncomingSyncRequest>,
    sync_resp_tx: mpsc::UnboundedSender<SyncResponse>,
    peer_info_tx: watch::Sender<Vec<PeerStatus>>,
}

impl NetworkService {
    /// Create the network service and a NetworkSink for the consensus engine.
    ///
    /// Returns:
    /// - `NetworkService` — run with `.run()`
    /// - `Litep2pNetworkSink` — for consensus engine + RPC peer management
    /// - `msg_rx` — consensus messages for the engine
    /// - `sync_req_rx` — incoming sync requests for the sync responder
    /// - `sync_resp_rx` — sync responses for the sync requester
    /// - `peer_info_rx` — peer list updates for RPC
    #[allow(clippy::type_complexity)]
    pub fn create(
        listen_addr: Multiaddr,
        peer_map: PeerMap,
        known_addresses: Vec<(PeerId, Vec<Multiaddr>)>,
        keypair: Option<litep2p::crypto::ed25519::Keypair>,
    ) -> Result<(
        Self,
        Litep2pNetworkSink,
        mpsc::UnboundedReceiver<(ValidatorId, ConsensusMessage)>,
        mpsc::UnboundedReceiver<IncomingSyncRequest>,
        mpsc::UnboundedReceiver<SyncResponse>,
        watch::Receiver<Vec<PeerStatus>>,
    )> {
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

        let mut config_builder = ConfigBuilder::new()
            .with_tcp(TcpConfig {
                listen_addresses: vec![listen_addr],
                ..Default::default()
            })
            .with_notification_protocol(notif_config)
            .with_request_response_protocol(reqresp_config)
            .with_request_response_protocol(sync_config);

        if let Some(kp) = keypair {
            config_builder = config_builder.with_keypair(kp);
        }

        if !known_addresses.is_empty() {
            config_builder = config_builder.with_known_addresses(known_addresses.into_iter());
        }

        let litep2p =
            Litep2p::new(config_builder.build()).c(d!("failed to create litep2p instance"))?;

        info!(peer_id = %litep2p.local_peer_id(), "litep2p started");
        for addr in litep2p.listen_addresses() {
            info!(address = %addr, "listening on");
        }

        let (msg_tx, msg_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (sync_req_tx, sync_req_rx) = mpsc::unbounded_channel();
        let (sync_resp_tx, sync_resp_rx) = mpsc::unbounded_channel();

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

        let sink = Litep2pNetworkSink {
            cmd_tx: cmd_tx.clone(),
        };

        Ok((
            Self {
                litep2p,
                notif_handle,
                reqresp_handle,
                sync_handle,
                peer_map,
                msg_tx,
                cmd_rx,
                sync_req_tx,
                sync_resp_tx,
                peer_info_tx,
            },
            sink,
            msg_rx,
            sync_req_rx,
            sync_resp_rx,
            peer_info_rx,
        ))
    }

    pub fn local_peer_id(&self) -> &PeerId {
        self.litep2p.local_peer_id()
    }

    /// Run the network event loop
    pub async fn run(mut self) {
        loop {
            tokio::select! {
                event = self.notif_handle.next() => {
                    if let Some(event) = event {
                        self.handle_notification_event(event);
                    }
                }
                event = self.reqresp_handle.next() => {
                    if let Some(event) = event {
                        self.handle_reqresp_event(event);
                    }
                }
                event = self.sync_handle.next() => {
                    if let Some(event) = event {
                        self.handle_sync_event(event);
                    }
                }
                event = self.litep2p.next_event() => {
                    if let Some(event) = event {
                        self.handle_litep2p_event(event);
                    }
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
            }
        }
    }

    fn handle_notification_event(&mut self, event: NotificationEvent) {
        match event {
            NotificationEvent::ValidateSubstream { peer, .. } => {
                self.notif_handle
                    .send_validation_result(peer, ValidationResult::Accept);
            }
            NotificationEvent::NotificationStreamOpened { peer, .. } => {
                info!(peer = %peer, "notification stream opened");
            }
            NotificationEvent::NotificationStreamClosed { peer } => {
                debug!(peer = %peer, "notification stream closed");
            }
            NotificationEvent::NotificationReceived { peer, notification } => {
                let Some(sender) = self.peer_map.peer_to_validator.get(&peer).copied() else {
                    warn!(peer = %peer, "dropping notification from unknown peer");
                    return;
                };
                match serde_cbor_2::from_slice::<ConsensusMessage>(&notification) {
                    Ok(msg) => {
                        let _ = self.msg_tx.send((sender, msg));
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to decode notification");
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
                match serde_cbor_2::from_slice::<ConsensusMessage>(&request) {
                    Ok(msg) => {
                        let _ = self.msg_tx.send((sender, msg));
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

    fn handle_sync_event(&mut self, event: RequestResponseEvent) {
        match event {
            RequestResponseEvent::RequestReceived {
                peer,
                request_id,
                request,
                ..
            } => {
                // Forward sync request to the sync responder
                match serde_cbor_2::from_slice::<SyncRequest>(&request) {
                    Ok(req) => {
                        let _ = self.sync_req_tx.send(IncomingSyncRequest {
                            request_id,
                            peer,
                            request: req,
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to decode sync request");
                        let err_resp = SyncResponse::Error(format!("decode error: {e}"));
                        if let Ok(bytes) = serde_cbor_2::to_vec(&err_resp) {
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
                match serde_cbor_2::from_slice::<SyncResponse>(&response) {
                    Ok(resp) => {
                        let _ = self.sync_resp_tx.send(resp);
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to decode sync response");
                    }
                }
            }
            RequestResponseEvent::RequestFailed { peer, error, .. } => {
                debug!(peer = %peer, error = ?error, "sync request failed");
                let _ = self
                    .sync_resp_tx
                    .send(SyncResponse::Error(format!("request failed: {error:?}")));
            }
        }
    }

    fn handle_litep2p_event(&mut self, event: Litep2pEvent) {
        match event {
            Litep2pEvent::ConnectionEstablished { peer, endpoint } => {
                info!(peer = %peer, endpoint = ?endpoint, "connection established");
            }
            Litep2pEvent::ConnectionClosed { peer, .. } => {
                debug!(peer = %peer, "connection closed");
            }
            Litep2pEvent::DialFailure { address, error, .. } => {
                warn!(address = %address, error = ?error, "dial failed");
            }
            _ => {}
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
                for &peer in self.peer_map.peer_to_validator.keys() {
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
        }
    }
}

/// NetworkSink backed by litep2p, for use by the consensus engine.
/// Also provides methods for peer management and sync.
#[derive(Clone)]
pub struct Litep2pNetworkSink {
    cmd_tx: mpsc::UnboundedSender<NetCommand>,
}

impl Litep2pNetworkSink {
    pub fn add_peer(&self, vid: ValidatorId, pid: PeerId, addrs: Vec<Multiaddr>) {
        let _ = self.cmd_tx.send(NetCommand::AddPeer(vid, pid, addrs));
    }

    pub fn remove_peer(&self, vid: ValidatorId) {
        let _ = self.cmd_tx.send(NetCommand::RemovePeer(vid));
    }

    pub fn send_sync_request(&self, peer_id: PeerId, request: &SyncRequest) {
        if let Ok(bytes) = serde_cbor_2::to_vec(request) {
            let _ = self.cmd_tx.send(NetCommand::SyncRequest(peer_id, bytes));
        }
    }

    pub fn send_sync_response(&self, request_id: RequestId, response: &SyncResponse) {
        if let Ok(bytes) = serde_cbor_2::to_vec(response) {
            let _ = self.cmd_tx.send(NetCommand::SyncRespond(request_id, bytes));
        }
    }
}

impl NetworkSink for Litep2pNetworkSink {
    fn broadcast(&self, msg: ConsensusMessage) {
        if let Ok(bytes) = serde_cbor_2::to_vec(&msg) {
            let _ = self.cmd_tx.send(NetCommand::Broadcast(bytes));
        }
    }

    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage) {
        if let Ok(bytes) = serde_cbor_2::to_vec(&msg) {
            let _ = self.cmd_tx.send(NetCommand::SendTo(target, bytes));
        }
    }
}
