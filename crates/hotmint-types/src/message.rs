use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::certificate::{DoubleCertificate, QuorumCertificate, TimeoutCertificate};
use crate::crypto::Signature;
use crate::validator::ValidatorId;
use crate::view::ViewNumber;
use crate::vote::Vote;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsensusMessage {
    /// Leader broadcasts proposal
    Propose {
        block: Box<Block>,
        justify: Box<QuorumCertificate>,
        double_cert: Option<Box<DoubleCertificate>>,
        signature: Signature,
    },

    /// First-phase vote → current leader
    VoteMsg(Vote),

    /// Leader broadcasts QC after collecting 2f+1 votes
    Prepare {
        certificate: QuorumCertificate,
        signature: Signature,
    },

    /// Second-phase vote → next leader
    Vote2Msg(Vote),

    /// Timeout wish: validator wants to advance to target_view
    Wish {
        target_view: ViewNumber,
        validator: ValidatorId,
        highest_qc: Option<QuorumCertificate>,
        signature: Signature,
    },

    /// Timeout certificate broadcast
    TimeoutCert(TimeoutCertificate),

    /// Status message: replica sends locked_qc to new leader
    StatusCert {
        locked_qc: Option<QuorumCertificate>,
        validator: ValidatorId,
        signature: Signature,
    },
}
