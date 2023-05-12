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
    for (i, signed) in agg.signers.iter().enumerate() {
        if *signed {
            power += vs.validators[i].power;
        }
    }
    power >= vs.quorum_threshold()
}
