use ruc::*;

use std::collections::HashMap;

use futures::StreamExt;
use hotmint_consensus::network::NetworkSink;
use hotmint_types::{ConsensusMessage, ValidatorId};
use litep2p::config::ConfigBuilder;
use litep2p::protocol::notification::{
    ConfigBuilder as NotifConfigBuilder, NotificationEvent, NotificationHandle, ValidationResult,
};
use litep2p::protocol::request_response::{
    ConfigBuilder as ReqRespConfigBuilder, DialOptions, RequestResponseEvent, RequestResponseHandle,
};
use litep2p::transport::tcp::config::Config as TcpConfig;
use litep2p::types::multiaddr::Multiaddr;
use litep2p::{Litep2p, Litep2pEvent, PeerId};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

const NOTIF_PROTOCOL: &str = "/hotmint/consensus/notif/1";
const REQ_RESP_PROTOCOL: &str = "/hotmint/consensus/reqresp/1";
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
}

impl Default for PeerMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Commands sent from the NetworkSink to the NetworkService
enum NetCommand {
    Broadcast(Vec<u8>),
    SendTo(ValidatorId, Vec<u8>),
}

/// NetworkService wraps litep2p and provides consensus-level networking
pub struct NetworkService {
    litep2p: Litep2p,
    notif_handle: NotificationHandle,
    reqresp_handle: RequestResponseHandle,
    peer_map: PeerMap,
    msg_tx: mpsc::UnboundedSender<(ValidatorId, ConsensusMessage)>,
    cmd_rx: mpsc::UnboundedReceiver<NetCommand>,
}

impl NetworkService {
    /// Create the network service and a NetworkSink for the consensus engine.
    /// Returns (service, network_sink, msg_rx_for_engine)
    #[allow(clippy::type_complexity)]
    pub fn create(
        listen_addr: Multiaddr,
        peer_map: PeerMap,
        known_addresses: Vec<(PeerId, Vec<Multiaddr>)>,
    ) -> Result<(
        Self,
        Litep2pNetworkSink,
        mpsc::UnboundedReceiver<(ValidatorId, ConsensusMessage)>,
    )> {
        let (notif_config, notif_handle) = NotifConfigBuilder::new(NOTIF_PROTOCOL.into())
            .with_max_size(MAX_NOTIFICATION_SIZE)
            .with_auto_accept_inbound(true)
            .with_sync_channel_size(1024)
            .with_async_channel_size(1024)
            .build();

        let (reqresp_config, reqresp_handle) = ReqRespConfigBuilder::new(REQ_RESP_PROTOCOL.into())
            .with_max_size(MAX_NOTIFICATION_SIZE)
            .build();

        let mut config_builder = ConfigBuilder::new()
            .with_tcp(TcpConfig {
                listen_addresses: vec![listen_addr],
                ..Default::default()
            })
            .with_notification_protocol(notif_config)
            .with_request_response_protocol(reqresp_config);

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

        let sink = Litep2pNetworkSink { cmd_tx };

        Ok((
            Self {
                litep2p,
                notif_handle,
                reqresp_handle,
                peer_map,
                msg_tx,
                cmd_rx,
            },
            sink,
            msg_rx,
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
                let sender = self
                    .peer_map
                    .peer_to_validator
                    .get(&peer)
                    .copied()
                    .unwrap_or_default();
                match rmp_serde::from_slice::<ConsensusMessage>(&notification) {
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
                let sender = self
                    .peer_map
                    .peer_to_validator
                    .get(&peer)
                    .copied()
                    .unwrap_or_default();
                match rmp_serde::from_slice::<ConsensusMessage>(&request) {
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

    async fn handle_command(&mut self, cmd: NetCommand) {
        match cmd {
            NetCommand::Broadcast(bytes) => {
                // Send to all connected peers via notification
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
        }
    }
}

/// NetworkSink backed by litep2p, for use by the consensus engine.
/// Sends commands to the NetworkService via a channel.
pub struct Litep2pNetworkSink {
    cmd_tx: mpsc::UnboundedSender<NetCommand>,
}

impl NetworkSink for Litep2pNetworkSink {
    fn broadcast(&self, msg: ConsensusMessage) {
        if let Ok(bytes) = rmp_serde::to_vec(&msg) {
            let _ = self.cmd_tx.send(NetCommand::Broadcast(bytes));
        }
    }

    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage) {
        if let Ok(bytes) = rmp_serde::to_vec(&msg) {
            let _ = self.cmd_tx.send(NetCommand::SendTo(target, bytes));
        }
    }
}
