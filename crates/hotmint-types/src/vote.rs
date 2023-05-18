use serde::{Deserialize, Serialize};

use crate::block::BlockHash;
use crate::crypto::Signature;
use crate::validator::ValidatorId;
use crate::view::ViewNumber;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VoteType {
    /// First-phase vote (step 3)
    Vote,
    /// Second-phase vote (step 5)
    Vote2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub block_hash: BlockHash,
    pub view: ViewNumber,
    pub validator: ValidatorId,
    pub signature: Signature,
    pub vote_type: VoteType,
}

impl Vote {
    /// Canonical bytes for signing: view || block_hash || vote_type
    pub fn signing_bytes(view: ViewNumber, block_hash: &BlockHash, vote_type: VoteType) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32 + 8 + 1);
        buf.extend_from_slice(&view.as_u64().to_le_bytes());
        buf.extend_from_slice(&block_hash.0);
        buf.push(vote_type as u8);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signing_bytes_deterministic() {
        let hash = BlockHash([42u8; 32]);
        let a = Vote::signing_bytes(ViewNumber(5), &hash, VoteType::Vote);
        let b = Vote::signing_bytes(ViewNumber(5), &hash, VoteType::Vote);
        assert_eq!(a, b);
    }

    #[test]
    fn test_signing_bytes_differ_by_type() {
        let hash = BlockHash([1u8; 32]);
        let a = Vote::signing_bytes(ViewNumber(1), &hash, VoteType::Vote);
        let b = Vote::signing_bytes(ViewNumber(1), &hash, VoteType::Vote2);
        assert_ne!(a, b);
    }

    #[test]
    fn test_signing_bytes_differ_by_view() {
        let hash = BlockHash([1u8; 32]);
        let a = Vote::signing_bytes(ViewNumber(1), &hash, VoteType::Vote);
        let b = Vote::signing_bytes(ViewNumber(2), &hash, VoteType::Vote);
        assert_ne!(a, b);
    }
}
