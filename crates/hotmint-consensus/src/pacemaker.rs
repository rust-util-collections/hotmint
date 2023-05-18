use std::collections::HashMap;
use std::time::Duration;

use hotmint_types::crypto::{AggregateSignature, Signature};
use hotmint_types::{
    ConsensusMessage, QuorumCertificate, TimeoutCertificate, ValidatorId, ValidatorSet, ViewNumber,
};
use tokio::time::{Instant, Sleep};

const BASE_TIMEOUT_MS: u64 = 2000;

/// Simplified pacemaker (Phase 1): per-view timer, wish collection, TC formation
pub struct Pacemaker {
    pub view_deadline: Instant,
    base_timeout: Duration,
    /// Collected wishes: target_view -> (validator_id, highest_qc, signature)
    wishes: HashMap<ViewNumber, Vec<(ValidatorId, Option<QuorumCertificate>, Signature)>>,
}

impl Default for Pacemaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Pacemaker {
    pub fn new() -> Self {
        let base_timeout = Duration::from_millis(BASE_TIMEOUT_MS);
        Self {
            view_deadline: Instant::now() + base_timeout,
            base_timeout,
            wishes: HashMap::new(),
        }
    }

    pub fn reset_timer(&mut self) {
        self.view_deadline = Instant::now() + self.base_timeout;
    }

    pub fn sleep_until_deadline(&self) -> Sleep {
        tokio::time::sleep_until(self.view_deadline)
    }

    /// Build the Wish message for timeout
    pub fn build_wish(
        &self,
        current_view: ViewNumber,
        validator_id: ValidatorId,
        highest_qc: Option<QuorumCertificate>,
        signer: &dyn hotmint_types::Signer,
    ) -> ConsensusMessage {
        let target_view = current_view.next();
        let msg_bytes = wish_signing_bytes(target_view);
        let signature = signer.sign(&msg_bytes);
        ConsensusMessage::Wish {
            target_view,
            validator: validator_id,
            highest_qc,
            signature,
        }
    }

    /// Add a wish and check if we have 2f+1 to form a TC
    pub fn add_wish(
        &mut self,
        vs: &ValidatorSet,
        target_view: ViewNumber,
        validator: ValidatorId,
        highest_qc: Option<QuorumCertificate>,
        signature: Signature,
    ) -> Option<TimeoutCertificate> {
        let wishes = self.wishes.entry(target_view).or_default();

        // Dedup
        if wishes.iter().any(|(id, _, _)| *id == validator) {
            return None;
        }

        wishes.push((validator, highest_qc, signature));

        // Check quorum
        let mut power = 0u64;
        for (vid, _, _) in wishes.iter() {
            power += vs.power_of(*vid);
        }

        if power >= vs.quorum_threshold() {
            let mut agg = AggregateSignature::new(vs.validator_count());
            let mut highest_qcs = vec![None; vs.validator_count()];

            for (vid, hqc, sig) in wishes.iter() {
                if let Some(idx) = vs.index_of(*vid) {
                    let _ = agg.add(idx, sig.clone());
                    highest_qcs[idx] = hqc.clone();
                }
            }

            Some(TimeoutCertificate {
                view: ViewNumber(target_view.as_u64().saturating_sub(1)),
                aggregate_signature: agg,
                highest_qcs,
            })
        } else {
            None
        }
    }

    pub fn clear_view(&mut self, view: ViewNumber) {
        self.wishes.remove(&view);
    }
}

fn wish_signing_bytes(target_view: ViewNumber) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9);
    buf.push(b'W');
    buf.extend_from_slice(&target_view.as_u64().to_le_bytes());
    buf
}
