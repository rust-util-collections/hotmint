use ruc::*;

use hotmint_crypto::aggregate::{aggregate_votes, has_quorum};
use hotmint_types::validator::ValidatorSet;
use hotmint_types::view::ViewNumber;
use hotmint_types::vote::{Vote, VoteType};
use hotmint_types::{BlockHash, QuorumCertificate};
use std::collections::HashMap;

/// Collects votes and forms QCs when quorum is reached
pub struct VoteCollector {
    /// (view, block_hash, vote_type) -> votes
    votes: HashMap<(ViewNumber, BlockHash, VoteType), Vec<Vote>>,
}

impl Default for VoteCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl VoteCollector {
    pub fn new() -> Self {
        Self {
            votes: HashMap::new(),
        }
    }

    /// Add a vote and check if quorum is reached. Returns QC if formed.
    pub fn add_vote(&mut self, vs: &ValidatorSet, vote: Vote) -> Result<Option<QuorumCertificate>> {
        let key = (vote.view, vote.block_hash, vote.vote_type);
        let votes = self.votes.entry(key).or_default();

        // Dedup
        if votes.iter().any(|v| v.validator == vote.validator) {
            return Ok(None);
        }

        votes.push(vote);

        let agg = aggregate_votes(vs, votes).c(d!())?;
        if has_quorum(vs, &agg) {
            let qc = QuorumCertificate {
                block_hash: key.1,
                view: key.0,
                aggregate_signature: agg,
            };
            Ok(Some(qc))
        } else {
            Ok(None)
        }
    }

    pub fn clear_view(&mut self, view: ViewNumber) {
        self.votes.retain(|k, _| k.0 != view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotmint_crypto::Ed25519Signer;
    use hotmint_types::Signer;
    use hotmint_types::validator::{ValidatorId, ValidatorInfo};

    fn make_test_env() -> (ValidatorSet, Vec<Ed25519Signer>) {
        let signers: Vec<Ed25519Signer> = (0..4)
            .map(|i| Ed25519Signer::generate(ValidatorId(i)))
            .collect();
        let infos: Vec<ValidatorInfo> = signers
            .iter()
            .map(|s| ValidatorInfo {
                id: s.validator_id(),
                public_key: s.public_key(),
                power: 1,
            })
            .collect();
        (ValidatorSet::new(infos), signers)
    }

    fn make_vote(
        signer: &Ed25519Signer,
        view: ViewNumber,
        hash: BlockHash,
        vote_type: VoteType,
    ) -> Vote {
        let bytes = Vote::signing_bytes(view, &hash, vote_type);
        Vote {
            block_hash: hash,
            view,
            validator: signer.validator_id(),
            signature: signer.sign(&bytes),
            vote_type,
        }
    }

    #[test]
    fn test_qc_formed_at_quorum() {
        let (vs, signers) = make_test_env();
        let mut vc = VoteCollector::new();
        let hash = BlockHash([1u8; 32]);
        let view = ViewNumber(1);

        // 2 votes: no quorum yet
        let v0 = make_vote(&signers[0], view, hash, VoteType::Vote);
        assert!(vc.add_vote(&vs, v0).unwrap().is_none());

        let v1 = make_vote(&signers[1], view, hash, VoteType::Vote);
        assert!(vc.add_vote(&vs, v1).unwrap().is_none());

        // 3rd vote: quorum reached (3 out of 4)
        let v2 = make_vote(&signers[2], view, hash, VoteType::Vote);
        let qc = vc.add_vote(&vs, v2).unwrap();
        assert!(qc.is_some());
        let qc = qc.unwrap();
        assert_eq!(qc.block_hash, hash);
        assert_eq!(qc.view, view);
    }

    #[test]
    fn test_duplicate_vote_ignored() {
        let (vs, signers) = make_test_env();
        let mut vc = VoteCollector::new();
        let hash = BlockHash([2u8; 32]);
        let view = ViewNumber(1);

        let v0 = make_vote(&signers[0], view, hash, VoteType::Vote);
        assert!(vc.add_vote(&vs, v0.clone()).unwrap().is_none());

        // Same validator again
        let v0_dup = make_vote(&signers[0], view, hash, VoteType::Vote);
        assert!(vc.add_vote(&vs, v0_dup).unwrap().is_none());
    }

    #[test]
    fn test_different_vote_types_separate() {
        let (vs, signers) = make_test_env();
        let mut vc = VoteCollector::new();
        let hash = BlockHash([3u8; 32]);
        let view = ViewNumber(1);

        // 2 Vote + 1 Vote2 should NOT form a QC
        let v0 = make_vote(&signers[0], view, hash, VoteType::Vote);
        let v1 = make_vote(&signers[1], view, hash, VoteType::Vote);
        let v2 = make_vote(&signers[2], view, hash, VoteType::Vote2);

        assert!(vc.add_vote(&vs, v0).unwrap().is_none());
        assert!(vc.add_vote(&vs, v1).unwrap().is_none());
        assert!(vc.add_vote(&vs, v2).unwrap().is_none());
    }

    #[test]
    fn test_clear_view() {
        let (vs, signers) = make_test_env();
        let mut vc = VoteCollector::new();
        let hash = BlockHash([4u8; 32]);

        let v0 = make_vote(&signers[0], ViewNumber(1), hash, VoteType::Vote);
        let v1 = make_vote(&signers[1], ViewNumber(1), hash, VoteType::Vote);
        vc.add_vote(&vs, v0).unwrap();
        vc.add_vote(&vs, v1).unwrap();

        vc.clear_view(ViewNumber(1));

        // After clearing, adding 1 more vote shouldn't form QC
        let v2 = make_vote(&signers[2], ViewNumber(1), hash, VoteType::Vote);
        assert!(vc.add_vote(&vs, v2).unwrap().is_none());
    }
}
