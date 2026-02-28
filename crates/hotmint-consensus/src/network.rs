use hotmint_types::{ConsensusMessage, ValidatorId};

pub trait NetworkSink: Send + Sync {
    fn broadcast(&self, msg: ConsensusMessage);
    fn send_to(&self, target: ValidatorId, msg: ConsensusMessage);
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
