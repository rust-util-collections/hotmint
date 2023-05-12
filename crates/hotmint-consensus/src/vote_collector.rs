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
