use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::time::Duration;

use hotmint_types::crypto::{AggregateSignature, Signature};
use hotmint_types::{
    ConsensusMessage, QuorumCertificate, TimeoutCertificate, ValidatorId, ValidatorSet, ViewNumber,
};
use tokio::time::{Instant, Sleep};
use tracing::debug;

const BASE_TIMEOUT_MS: u64 = 2000;
const MAX_TIMEOUT_MS: u64 = 30000;
const BACKOFF_MULTIPLIER: f64 = 1.5;

/// Configurable pacemaker parameters.
#[derive(Debug, Clone)]
pub struct PacemakerConfig {
    pub base_timeout_ms: u64,
    pub max_timeout_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for PacemakerConfig {
    fn default() -> Self {
        Self {
            base_timeout_ms: BASE_TIMEOUT_MS,
            max_timeout_ms: MAX_TIMEOUT_MS,
            backoff_multiplier: BACKOFF_MULTIPLIER,
        }
    }
}

/// Full pacemaker with exponential backoff and TC relay
pub struct Pacemaker {
    config: PacemakerConfig,
    pub view_deadline: Instant,
    base_timeout: Duration,
    current_timeout: Duration,
    /// Number of consecutive timeouts without progress
    consecutive_timeouts: u32,
    /// Collected wishes: target_view -> (validator_id, highest_qc, signature)
    wishes: HashMap<ViewNumber, Vec<(ValidatorId, Option<QuorumCertificate>, Signature)>>,
    /// TCs we have already relayed (avoid re-broadcasting)
    relayed_tcs: HashMap<ViewNumber, bool>,
}

impl Default for Pacemaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Pacemaker {
    pub fn new() -> Self {
        Self::with_config(PacemakerConfig::default())
    }

    pub fn with_config(config: PacemakerConfig) -> Self {
        let base_timeout = Duration::from_millis(config.base_timeout_ms);
        Self {
            config,
            view_deadline: Instant::now() + base_timeout,
            base_timeout,
            current_timeout: base_timeout,
            consecutive_timeouts: 0,
            wishes: HashMap::new(),
            relayed_tcs: HashMap::new(),
        }
    }

    /// Reset timer with current backoff-adjusted timeout
    pub fn reset_timer(&mut self) {
        self.view_deadline = Instant::now() + self.current_timeout;
    }

    /// Reset timer after successful view completion (resets backoff)
    pub fn reset_on_progress(&mut self) {
        self.consecutive_timeouts = 0;
        self.current_timeout = self.base_timeout;
        self.view_deadline = Instant::now() + self.current_timeout;
    }

    /// Increase timeout with exponential backoff after a timeout event
    pub fn on_timeout(&mut self) {
        self.consecutive_timeouts += 1;
        let multiplier = self
            .config
            .backoff_multiplier
            .powi(self.consecutive_timeouts as i32);
        let new_ms = (self.base_timeout.as_millis() as f64 * multiplier) as u64;
        self.current_timeout = Duration::from_millis(new_ms.min(self.config.max_timeout_ms));
        debug!(
            consecutive = self.consecutive_timeouts,
            timeout_ms = self.current_timeout.as_millis() as u64,
            "pacemaker timeout backoff"
        );
        self.view_deadline = Instant::now() + self.current_timeout;
    }

    pub fn sleep_until_deadline(&self) -> Sleep {
        tokio::time::sleep_until(self.view_deadline)
    }

    pub fn current_timeout(&self) -> Duration {
        self.current_timeout
    }

    pub fn consecutive_timeouts(&self) -> u32 {
        self.consecutive_timeouts
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
        let msg_bytes = wish_signing_bytes(target_view, highest_qc.as_ref());
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

    /// Check if we should relay a received TC (returns true if not yet relayed)
    pub fn should_relay_tc(&mut self, tc: &TimeoutCertificate) -> bool {
        match self.relayed_tcs.entry(tc.view) {
            Entry::Vacant(e) => {
                e.insert(true);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    pub fn clear_view(&mut self, view: ViewNumber) {
        self.wishes.remove(&view);
        // Keep relayed_tcs — pruned lazily
    }

    /// Prune old relay tracking data
    pub fn prune_before(&mut self, min_view: ViewNumber) {
        self.wishes.retain(|v, _| *v >= min_view);
        self.relayed_tcs.retain(|v, _| *v >= min_view);
    }
}

pub(crate) fn wish_signing_bytes(
    target_view: ViewNumber,
    highest_qc: Option<&QuorumCertificate>,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(42);
    buf.push(b'W');
    buf.extend_from_slice(&target_view.as_u64().to_le_bytes());
    // Bind the highest_qc to prevent replay with a different QC.
    // Canonical encoding: 0x00 = None, 0x01 + view_le + hash = Some.
    match highest_qc {
        None => buf.push(0x00),
        Some(qc) => {
            buf.push(0x01);
            buf.extend_from_slice(&qc.view.as_u64().to_le_bytes());
            buf.extend_from_slice(&qc.block_hash.0);
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_backoff() {
        let mut pm = Pacemaker::new();
        assert_eq!(pm.consecutive_timeouts(), 0);
        assert_eq!(pm.current_timeout().as_millis(), 2000);

        pm.on_timeout();
        assert_eq!(pm.consecutive_timeouts(), 1);
        assert_eq!(pm.current_timeout().as_millis(), 3000); // 2000 * 1.5

        pm.on_timeout();
        assert_eq!(pm.consecutive_timeouts(), 2);
        assert_eq!(pm.current_timeout().as_millis(), 4500); // 2000 * 1.5^2

        pm.on_timeout();
        assert_eq!(pm.consecutive_timeouts(), 3);
        assert_eq!(pm.current_timeout().as_millis(), 6750); // 2000 * 1.5^3
    }

    #[test]
    fn test_backoff_caps_at_max() {
        let mut pm = Pacemaker::new();
        for _ in 0..20 {
            pm.on_timeout();
        }
        assert!(pm.current_timeout().as_millis() <= MAX_TIMEOUT_MS as u128);
    }

    #[test]
    fn test_reset_on_progress() {
        let mut pm = Pacemaker::new();
        pm.on_timeout();
        pm.on_timeout();
        assert!(pm.current_timeout().as_millis() > 2000);

        pm.reset_on_progress();
        assert_eq!(pm.consecutive_timeouts(), 0);
        assert_eq!(pm.current_timeout().as_millis(), 2000);
    }

    #[test]
    fn test_tc_relay_dedup() {
        let mut pm = Pacemaker::new();
        let tc = TimeoutCertificate {
            view: ViewNumber(5),
            aggregate_signature: AggregateSignature::new(4),
            highest_qcs: vec![],
        };
        assert!(pm.should_relay_tc(&tc));
        assert!(!pm.should_relay_tc(&tc)); // second time: already relayed
    }
}
