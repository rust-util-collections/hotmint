use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

/// Consensus metrics exposed via Prometheus
pub struct ConsensusMetrics {
    pub blocks_committed: Counter,
    pub blocks_proposed: Counter,
    pub votes_sent: Counter,
    pub qcs_formed: Counter,
    pub double_certs_formed: Counter,
    pub view_timeouts: Counter,
    pub tcs_formed: Counter,
    pub current_view: Gauge,
    pub current_height: Gauge,
    pub consecutive_timeouts: Gauge,
    pub view_duration_seconds: Histogram,
    pub epoch_transitions: Counter,
    pub equivocations_detected: Counter,
    pub current_epoch: Gauge,
}

impl ConsensusMetrics {
    pub fn new(registry: &mut Registry) -> Self {
        let sub = registry.sub_registry_with_prefix("hotmint");

        let blocks_committed = Counter::default();
        sub.register(
            "blocks_committed",
            "Total blocks committed",
            blocks_committed.clone(),
        );

        let blocks_proposed = Counter::default();
        sub.register(
            "blocks_proposed",
            "Total blocks proposed",
            blocks_proposed.clone(),
        );

        let votes_sent = Counter::default();
        sub.register("votes_sent", "Total votes sent", votes_sent.clone());

        let qcs_formed = Counter::default();
        sub.register("qcs_formed", "Total QCs formed", qcs_formed.clone());

        let double_certs_formed = Counter::default();
        sub.register(
            "double_certs_formed",
            "Total double certificates formed",
            double_certs_formed.clone(),
        );

        let view_timeouts = Counter::default();
        sub.register(
            "view_timeouts",
            "Total view timeouts",
            view_timeouts.clone(),
        );

        let tcs_formed = Counter::default();
        sub.register(
            "tcs_formed",
            "Total timeout certificates formed",
            tcs_formed.clone(),
        );

        let current_view = Gauge::default();
        sub.register("current_view", "Current view number", current_view.clone());

        let current_height = Gauge::default();
        sub.register(
            "current_height",
            "Last committed block height",
            current_height.clone(),
        );

        let consecutive_timeouts = Gauge::default();
        sub.register(
            "consecutive_timeouts",
            "Consecutive view timeouts",
            consecutive_timeouts.clone(),
        );

        let view_duration_seconds = Histogram::new(exponential_buckets(0.1, 2.0, 10));
        sub.register(
            "view_duration_seconds",
            "Time spent in each view",
            view_duration_seconds.clone(),
        );

        let epoch_transitions = Counter::default();
        sub.register(
            "epoch_transitions",
            "Total epoch transitions",
            epoch_transitions.clone(),
        );

        let equivocations_detected = Counter::default();
        sub.register(
            "equivocations_detected",
            "Total equivocations detected",
            equivocations_detected.clone(),
        );

        let current_epoch = Gauge::default();
        sub.register(
            "current_epoch",
            "Current epoch number",
            current_epoch.clone(),
        );

        Self {
            blocks_committed,
            blocks_proposed,
            votes_sent,
            qcs_formed,
            double_certs_formed,
            view_timeouts,
            tcs_formed,
            current_view,
            current_height,
            consecutive_timeouts,
            view_duration_seconds,
            epoch_transitions,
            equivocations_detected,
            current_epoch,
        }
    }
}
