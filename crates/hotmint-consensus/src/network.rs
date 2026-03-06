use hotmint_types::{ConsensusMessage, ValidatorId, ValidatorSet};

pub trait NetworkSink: Send + Sync {
    fn broadcast(&self, msg: ConsensusMessage);
    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage);
    /// Notify the network layer of a validator set change (epoch transition).
    /// Default is no-op for test stubs.
    fn on_epoch_change(&self, _new_validator_set: &ValidatorSet) {}
}

/// Channel-based network stub: routes messages via mpsc senders
pub struct ChannelNetwork {
    pub self_id: ValidatorId,
    pub senders: Vec<(
        ValidatorId,
        tokio::sync::mpsc::Sender<(ValidatorId, ConsensusMessage)>,
    )>,
}

impl ChannelNetwork {
    pub fn new(
        self_id: ValidatorId,
        senders: Vec<(
            ValidatorId,
            tokio::sync::mpsc::Sender<(ValidatorId, ConsensusMessage)>,
        )>,
    ) -> Self {
        Self { self_id, senders }
    }

    /// Create a fully-connected mesh of `n` channel networks.
    ///
    /// Returns one `(ChannelNetwork, Receiver)` pair per validator
    /// (indexed by `ValidatorId(0)` .. `ValidatorId(n-1)`),
    /// eliminating the manual HashMap plumbing.
    pub fn create_mesh(
        n: u64,
    ) -> Vec<(
        ChannelNetwork,
        tokio::sync::mpsc::Receiver<(ValidatorId, ConsensusMessage)>,
    )> {
        use std::collections::HashMap;

        let mut senders: HashMap<
            ValidatorId,
            tokio::sync::mpsc::Sender<(ValidatorId, ConsensusMessage)>,
        > = HashMap::new();
        let mut receivers: HashMap<
            ValidatorId,
            tokio::sync::mpsc::Receiver<(ValidatorId, ConsensusMessage)>,
        > = HashMap::new();

        for i in 0..n {
            let (tx, rx) = tokio::sync::mpsc::channel(8192);
            senders.insert(ValidatorId(i), tx);
            receivers.insert(ValidatorId(i), rx);
        }

        let all_senders: Vec<(
            ValidatorId,
            tokio::sync::mpsc::Sender<(ValidatorId, ConsensusMessage)>,
        )> = senders.iter().map(|(&id, tx)| (id, tx.clone())).collect();

        (0..n)
            .map(|i| {
                let vid = ValidatorId(i);
                let rx = receivers.remove(&vid).unwrap();
                let network = ChannelNetwork::new(vid, all_senders.clone());
                (network, rx)
            })
            .collect()
    }
}

impl NetworkSink for ChannelNetwork {
    fn broadcast(&self, msg: ConsensusMessage) {
        for (id, sender) in &self.senders {
            if *id != self.self_id {
                let _ = sender.try_send((self.self_id, msg.clone()));
            }
        }
    }

    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage) {
        for (id, sender) in &self.senders {
            if *id == target {
                let _ = sender.try_send((self.self_id, msg));
                return;
            }
        }
    }
}
