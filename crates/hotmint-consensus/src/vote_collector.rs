use ruc::*;

use hotmint_crypto::aggregate::{aggregate_votes, has_quorum};
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator::ValidatorSet;
use hotmint_types::view::ViewNumber;
use hotmint_types::vote::{Vote, VoteType};
use hotmint_types::{BlockHash, QuorumCertificate};
use std::collections::HashMap;

/// Result of adding a vote to the collector
pub struct VoteResult {
    /// QC formed if quorum was reached
    pub qc: Option<QuorumCertificate>,
    /// Equivocation detected (same validator, same view+type, different block)
    pub equivocation: Option<EquivocationProof>,
}

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

    /// Add a vote, detect equivocation, and check if quorum is reached.
    pub fn add_vote(&mut self, vs: &ValidatorSet, vote: Vote) -> Result<VoteResult> {
        // Detect equivocation: same (view, vote_type) but different block_hash
        let mut equivocation = None;
        for ((v, bh, vt), existing_votes) in &self.votes {
            if *v == vote.view
                && *vt == vote.vote_type
                && *bh != vote.block_hash
                && let Some(existing) = existing_votes
                    .iter()
                    .find(|ev| ev.validator == vote.validator)
            {
                equivocation = Some(EquivocationProof {
                    validator: vote.validator,
                    view: vote.view,
                    vote_type: vote.vote_type,
                    block_hash_a: existing.block_hash,
                    signature_a: existing.signature.clone(),
                    block_hash_b: vote.block_hash,
                    signature_b: vote.signature.clone(),
                });
                break;
            }
        }

        // Standard dedup + quorum check
        let key = (vote.view, vote.block_hash, vote.vote_type);
        let votes = self.votes.entry(key).or_default();

        if votes.iter().any(|v| v.validator == vote.validator) {
            return Ok(VoteResult {
                qc: None,
                equivocation,
            });
        }

        votes.push(vote);

        let agg = aggregate_votes(vs, votes).c(d!())?;
        let qc = if has_quorum(vs, &agg) {
            Some(QuorumCertificate {
                block_hash: key.1,
                view: key.0,
                aggregate_signature: agg,
            })
        } else {
            None
        };

        Ok(VoteResult { qc, equivocation })
    }

    pub fn clear_view(&mut self, view: ViewNumber) {
        self.votes.retain(|k, _| k.0 != view);
    }

    /// Remove all votes for views before `min_view` to prevent unbounded growth.
    pub fn prune_before(&mut self, min_view: ViewNumber) {
        self.votes.retain(|k, _| k.0 >= min_view);
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

        let v0 = make_vote(&signers[0], view, hash, VoteType::Vote);
        assert!(vc.add_vote(&vs, v0).unwrap().qc.is_none());

        let v1 = make_vote(&signers[1], view, hash, VoteType::Vote);
        assert!(vc.add_vote(&vs, v1).unwrap().qc.is_none());

        let v2 = make_vote(&signers[2], view, hash, VoteType::Vote);
        let result = vc.add_vote(&vs, v2).unwrap();
        assert!(result.qc.is_some());
        let qc = result.qc.unwrap();
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
        assert!(vc.add_vote(&vs, v0).unwrap().qc.is_none());

        let v0_dup = make_vote(&signers[0], view, hash, VoteType::Vote);
        let result = vc.add_vote(&vs, v0_dup).unwrap();
        assert!(result.qc.is_none());
        assert!(result.equivocation.is_none());
    }

    #[test]
    fn test_different_vote_types_separate() {
        let (vs, signers) = make_test_env();
        let mut vc = VoteCollector::new();
        let hash = BlockHash([3u8; 32]);
        let view = ViewNumber(1);

        let v0 = make_vote(&signers[0], view, hash, VoteType::Vote);
        let v1 = make_vote(&signers[1], view, hash, VoteType::Vote);
        let v2 = make_vote(&signers[2], view, hash, VoteType::Vote2);

        assert!(vc.add_vote(&vs, v0).unwrap().qc.is_none());
        assert!(vc.add_vote(&vs, v1).unwrap().qc.is_none());
        assert!(vc.add_vote(&vs, v2).unwrap().qc.is_none());
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

        let v2 = make_vote(&signers[2], ViewNumber(1), hash, VoteType::Vote);
        assert!(vc.add_vote(&vs, v2).unwrap().qc.is_none());
    }

    #[test]
    fn test_equivocation_detection() {
        let (vs, signers) = make_test_env();
        let mut vc = VoteCollector::new();
        let hash_a = BlockHash([10u8; 32]);
        let hash_b = BlockHash([20u8; 32]);
        let view = ViewNumber(1);

        // Validator 0 votes for hash_a
        let v0a = make_vote(&signers[0], view, hash_a, VoteType::Vote);
        let result = vc.add_vote(&vs, v0a).unwrap();
        assert!(result.equivocation.is_none());

        // Validator 0 votes for hash_b (different block, same view) — equivocation!
        let v0b = make_vote(&signers[0], view, hash_b, VoteType::Vote);
        let result = vc.add_vote(&vs, v0b).unwrap();
        assert!(result.equivocation.is_some());
        let proof = result.equivocation.unwrap();
        assert_eq!(proof.validator, ValidatorId(0));
        assert_eq!(proof.view, view);
        assert_eq!(proof.block_hash_a, hash_a);
        assert_eq!(proof.block_hash_b, hash_b);
    }

    #[test]
    fn test_no_false_equivocation() {
        let (vs, signers) = make_test_env();
        let mut vc = VoteCollector::new();
        let hash = BlockHash([10u8; 32]);
        let view = ViewNumber(1);

        // Same validator, same hash, same view — duplicate, not equivocation
        let v0 = make_vote(&signers[0], view, hash, VoteType::Vote);
        vc.add_vote(&vs, v0).unwrap();

        let v0_dup = make_vote(&signers[0], view, hash, VoteType::Vote);
        let result = vc.add_vote(&vs, v0_dup).unwrap();
        assert!(result.equivocation.is_none());
        assert!(result.qc.is_none());
    }
}
