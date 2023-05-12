use hotmint_types::{
    DoubleCertificate, Height, QuorumCertificate, ValidatorId, ValidatorSet, ViewNumber,
};

/// Role of the validator in the current view
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewRole {
    Leader,
    Replica,
}

/// Step within a view (state machine progression)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewStep {
    Entered,
    WaitingForStatus,
    Proposed,
    WaitingForProposal,
    Voted,
    CollectingVotes,
    Prepared,
    SentVote2,
    Done,
}

/// Mutable consensus state for a single validator
pub struct ConsensusState {
    pub validator_id: ValidatorId,
    pub validator_set: ValidatorSet,
    pub current_view: ViewNumber,
    pub role: ViewRole,
    pub step: ViewStep,
    pub locked_qc: Option<QuorumCertificate>,
    pub highest_double_cert: Option<DoubleCertificate>,
    pub highest_qc: Option<QuorumCertificate>,
    pub last_committed_height: Height,
}

impl ConsensusState {
    pub fn new(validator_id: ValidatorId, validator_set: ValidatorSet) -> Self {
        Self {
            validator_id,
            validator_set,
            current_view: ViewNumber::GENESIS,
            role: ViewRole::Replica,
            step: ViewStep::Entered,
            locked_qc: None,
            highest_double_cert: None,
            highest_qc: None,
            last_committed_height: Height::GENESIS,
        }
    }

    pub fn is_leader(&self) -> bool {
        self.role == ViewRole::Leader
    }

    pub fn update_highest_qc(&mut self, qc: &QuorumCertificate) {
        let dominated = self.highest_qc.as_ref().is_none_or(|h| qc.view > h.view);
        if dominated {
            self.highest_qc = Some(qc.clone());
        }
    }

    pub fn update_locked_qc(&mut self, qc: &QuorumCertificate) {
        let dominated = self.locked_qc.as_ref().is_none_or(|h| qc.view > h.view);
        if dominated {
            self.locked_qc = Some(qc.clone());
        }
    }
}
