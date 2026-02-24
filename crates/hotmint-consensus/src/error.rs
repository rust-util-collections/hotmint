use hotmint_types::{BlockHash, EpochNumber, EquivocationProof, ValidatorId, ViewNumber};
use std::fmt;

#[derive(Debug)]
pub enum ConsensusError {
    InvalidProposal(String),
    InvalidVote(String),
    InvalidCertificate(String),
    SafetyViolation(String),
    NotLeader {
        view: ViewNumber,
        leader: ValidatorId,
    },
    StaleMessage {
        msg_view: ViewNumber,
        current_view: ViewNumber,
    },
    MissingBlock(BlockHash),
    NetworkError(String),
    EpochMismatch {
        expected: EpochNumber,
        got: EpochNumber,
    },
    Equivocation(EquivocationProof),
}

impl fmt::Display for ConsensusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProposal(s) => write!(f, "invalid proposal: {s}"),
            Self::InvalidVote(s) => write!(f, "invalid vote: {s}"),
            Self::InvalidCertificate(s) => write!(f, "invalid certificate: {s}"),
            Self::SafetyViolation(s) => write!(f, "safety violation: {s}"),
            Self::NotLeader { view, leader } => {
                write!(f, "not leader for {view}, leader is {leader}")
            }
            Self::StaleMessage {
                msg_view,
                current_view,
            } => {
                write!(f, "stale message from {msg_view}, current {current_view}")
            }
            Self::MissingBlock(h) => write!(f, "missing block {h}"),
            Self::NetworkError(s) => write!(f, "network error: {s}"),
            Self::EpochMismatch { expected, got } => {
                write!(f, "epoch mismatch: expected {expected}, got {got}")
            }
            Self::Equivocation(proof) => {
                write!(
                    f,
                    "equivocation by {} in view {}",
                    proof.validator, proof.view
                )
            }
        }
    }
}
