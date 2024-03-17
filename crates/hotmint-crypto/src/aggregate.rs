use ruc::*;

use hotmint_types::crypto::AggregateSignature;
use hotmint_types::validator::ValidatorSet;
use hotmint_types::vote::Vote;

/// Collect individual signatures into an AggregateSignature
pub fn aggregate_votes(vs: &ValidatorSet, votes: &[Vote]) -> Result<AggregateSignature> {
    let mut agg = AggregateSignature::new(vs.validator_count());
    for vote in votes {
        let idx = vs
            .index_of(vote.validator)
            .c(d!("unknown validator in vote"))?;
        agg.add(idx, vote.signature.clone()).c(d!())?;
    }
    Ok(agg)
}

/// Check if an aggregate has reached quorum
pub fn has_quorum(vs: &ValidatorSet, agg: &AggregateSignature) -> bool {
    let mut power = 0u64;
    let validators = vs.validators();
    for (i, signed) in agg.signers.iter().enumerate() {
        if *signed && let Some(vi) = validators.get(i) {
            power += vi.power;
        }
    }
    power >= vs.quorum_threshold()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ed25519Signer;
    use hotmint_types::validator::{ValidatorId, ValidatorInfo};
    use hotmint_types::view::ViewNumber;
    use hotmint_types::vote::VoteType;
    use hotmint_types::{BlockHash, Signer};

    fn make_env() -> (ValidatorSet, Vec<Ed25519Signer>) {
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

    #[test]
    fn test_aggregate_votes_basic() {
        let (vs, signers) = make_env();
        let hash = BlockHash([1u8; 32]);
        let view = ViewNumber(1);
        let votes: Vec<Vote> = signers
            .iter()
            .take(3)
            .map(|s| {
                let bytes = Vote::signing_bytes(view, &hash, VoteType::Vote);
                Vote {
                    block_hash: hash,
                    view,
                    validator: s.validator_id(),
                    signature: s.sign(&bytes),
                    vote_type: VoteType::Vote,
                }
            })
            .collect();

        let agg = aggregate_votes(&vs, &votes).unwrap();
        assert_eq!(agg.count(), 3);
        assert!(has_quorum(&vs, &agg));
    }

    #[test]
    fn test_no_quorum_with_too_few_votes() {
        let (vs, signers) = make_env();
        let hash = BlockHash([2u8; 32]);
        let view = ViewNumber(1);
        let votes: Vec<Vote> = signers
            .iter()
            .take(2)
            .map(|s| {
                let bytes = Vote::signing_bytes(view, &hash, VoteType::Vote);
                Vote {
                    block_hash: hash,
                    view,
                    validator: s.validator_id(),
                    signature: s.sign(&bytes),
                    vote_type: VoteType::Vote,
                }
            })
            .collect();

        let agg = aggregate_votes(&vs, &votes).unwrap();
        assert_eq!(agg.count(), 2);
        assert!(!has_quorum(&vs, &agg));
    }

    #[test]
    fn test_aggregate_unknown_validator() {
        let (vs, _) = make_env();
        let unknown_signer = Ed25519Signer::generate(ValidatorId(99));
        let hash = BlockHash([3u8; 32]);
        let bytes = Vote::signing_bytes(ViewNumber(1), &hash, VoteType::Vote);
        let vote = Vote {
            block_hash: hash,
            view: ViewNumber(1),
            validator: ValidatorId(99),
            signature: unknown_signer.sign(&bytes),
            vote_type: VoteType::Vote,
        };
        assert!(aggregate_votes(&vs, &[vote]).is_err());
    }
}
