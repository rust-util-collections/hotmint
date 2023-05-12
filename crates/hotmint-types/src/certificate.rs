use serde::{Deserialize, Serialize};

use crate::block::BlockHash;
use crate::crypto::AggregateSignature;
use crate::view::ViewNumber;

/// C_v(B_k): quorum certificate — 2f+1 validators signed the block hash in view v
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumCertificate {
    pub block_hash: BlockHash,
    pub view: ViewNumber,
    pub aggregate_signature: AggregateSignature,
}

impl QuorumCertificate {
    /// Rank of a QC is its view number (used for comparison in safety rules)
    pub fn rank(&self) -> ViewNumber {
        self.view
    }
}

/// C_v(C_v(B_k)): double certificate — QC of QC, triggers commit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoubleCertificate {
    pub inner_qc: QuorumCertificate,
    pub outer_qc: QuorumCertificate,
}

/// TC_v: timeout certificate — 2f+1 validators want to leave view v
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutCertificate {
    pub view: ViewNumber,
    pub aggregate_signature: AggregateSignature,
    /// Each signer's highest known QC
    pub highest_qcs: Vec<Option<QuorumCertificate>>,
}

impl TimeoutCertificate {
    /// The highest QC carried in the TC
    pub fn highest_qc(&self) -> Option<&QuorumCertificate> {
        self.highest_qcs
            .iter()
            .filter_map(|qc| qc.as_ref())
            .max_by_key(|qc| qc.view)
    }
}
