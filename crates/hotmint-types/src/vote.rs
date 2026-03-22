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
    /// Canonical bytes for signing: domain_tag || chain_id_hash || view || block_hash || vote_type
    ///
    /// The domain tag and chain_id_hash prevent cross-chain and cross-message-type
    /// signature replay attacks.
    pub fn signing_bytes(
        chain_id_hash: &[u8; 32],
        view: ViewNumber,
        block_hash: &BlockHash,
        vote_type: VoteType,
    ) -> Vec<u8> {
        let tag = b"HOTMINT_VOTE_V1\0";
        let mut buf = Vec::with_capacity(tag.len() + 32 + 8 + 32 + 1);
        buf.extend_from_slice(tag);
        buf.extend_from_slice(chain_id_hash);
        buf.extend_from_slice(&view.as_u64().to_le_bytes());
        buf.extend_from_slice(&block_hash.0);
        buf.push(vote_type as u8);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CHAIN: [u8; 32] = [0u8; 32];

    #[test]
    fn test_signing_bytes_deterministic() {
        let hash = BlockHash([42u8; 32]);
        let a = Vote::signing_bytes(&TEST_CHAIN, ViewNumber(5), &hash, VoteType::Vote);
        let b = Vote::signing_bytes(&TEST_CHAIN, ViewNumber(5), &hash, VoteType::Vote);
        assert_eq!(a, b);
    }

    #[test]
    fn test_signing_bytes_differ_by_type() {
        let hash = BlockHash([1u8; 32]);
        let a = Vote::signing_bytes(&TEST_CHAIN, ViewNumber(1), &hash, VoteType::Vote);
        let b = Vote::signing_bytes(&TEST_CHAIN, ViewNumber(1), &hash, VoteType::Vote2);
        assert_ne!(a, b);
    }

    #[test]
    fn test_signing_bytes_differ_by_view() {
        let hash = BlockHash([1u8; 32]);
        let a = Vote::signing_bytes(&TEST_CHAIN, ViewNumber(1), &hash, VoteType::Vote);
        let b = Vote::signing_bytes(&TEST_CHAIN, ViewNumber(2), &hash, VoteType::Vote);
        assert_ne!(a, b);
    }

    #[test]
    fn test_signing_bytes_differ_by_chain() {
        let hash = BlockHash([1u8; 32]);
        let chain_a = [1u8; 32];
        let chain_b = [2u8; 32];
        let a = Vote::signing_bytes(&chain_a, ViewNumber(1), &hash, VoteType::Vote);
        let b = Vote::signing_bytes(&chain_b, ViewNumber(1), &hash, VoteType::Vote);
        assert_ne!(a, b);
    }
}
