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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_qc(view: u64, hash: [u8; 32]) -> QuorumCertificate {
        QuorumCertificate {
            block_hash: BlockHash(hash),
            view: ViewNumber(view),
            aggregate_signature: AggregateSignature::new(4),
        }
    }

    #[test]
    fn test_qc_rank() {
        let qc = make_qc(5, [1u8; 32]);
        assert_eq!(qc.rank(), ViewNumber(5));
    }

    #[test]
    fn test_tc_highest_qc_some() {
        let qc1 = make_qc(3, [1u8; 32]);
        let qc2 = make_qc(7, [2u8; 32]);
        let tc = TimeoutCertificate {
            view: ViewNumber(8),
            aggregate_signature: AggregateSignature::new(4),
            highest_qcs: vec![Some(qc1), None, Some(qc2), None],
        };
        let highest = tc.highest_qc().unwrap();
        assert_eq!(highest.view, ViewNumber(7));
    }

    #[test]
    fn test_tc_highest_qc_all_none() {
        let tc = TimeoutCertificate {
            view: ViewNumber(5),
            aggregate_signature: AggregateSignature::new(4),
            highest_qcs: vec![None, None, None, None],
        };
        assert!(tc.highest_qc().is_none());
    }

    #[test]
    fn test_tc_highest_qc_empty() {
        let tc = TimeoutCertificate {
            view: ViewNumber(5),
            aggregate_signature: AggregateSignature::new(4),
            highest_qcs: vec![],
        };
        assert!(tc.highest_qc().is_none());
    }
}
