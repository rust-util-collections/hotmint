# Metrics

Hotmint exposes Prometheus metrics via the `prometheus-client` crate for monitoring consensus health and performance.

## Setup

```rust
use prometheus_client::registry::Registry;
use hotmint::consensus::metrics::ConsensusMetrics;

let mut registry = Registry::default();
let metrics = ConsensusMetrics::new(&mut registry);
```

All metrics are registered under the `hotmint_` prefix.

## Available Metrics

### Counters

| Metric | Type | Description |
|:-------|:-----|:------------|
| `hotmint_blocks_committed` | Counter | Total number of blocks committed |
| `hotmint_blocks_proposed` | Counter | Total number of blocks proposed (leader only) |
| `hotmint_votes_sent` | Counter | Total number of votes sent (phase-1 and phase-2) |
| `hotmint_qcs_formed` | Counter | Total number of Quorum Certificates formed |
| `hotmint_double_certs_formed` | Counter | Total number of Double Certificates formed (triggers commit) |
| `hotmint_view_timeouts` | Counter | Total number of view timeouts |
| `hotmint_tcs_formed` | Counter | Total number of Timeout Certificates formed |

### Gauges

| Metric | Type | Description |
|:-------|:-----|:------------|
| `hotmint_current_view` | Gauge | Current view number |
| `hotmint_current_height` | Gauge | Latest committed block height |
| `hotmint_consecutive_timeouts` | Gauge | Number of consecutive timeouts (0 = healthy) |

### Histograms

| Metric | Type | Description |
|:-------|:-----|:------------|
| `hotmint_view_duration_seconds` | Histogram | Time spent in each view (seconds) |

## Interpretation

### Healthy Consensus

- `blocks_committed` increases steadily
- `consecutive_timeouts` stays at 0
- `view_duration_seconds` is low and consistent (close to network RTT + processing time)
- `qcs_formed` and `double_certs_formed` increase at roughly the same rate as `blocks_committed`

### Degraded Consensus

- `view_timeouts` increasing: views are timing out (possible leader failure or network partition)
- `consecutive_timeouts` > 0: multiple consecutive timeouts indicate sustained issues
- `view_duration_seconds` increasing: consensus is slowing down

### Stalled Consensus

- `blocks_committed` stops increasing
- `consecutive_timeouts` grows unbounded
- `tcs_formed` may or may not increase (depends on whether timeout certificates can still form)

## Exposing Metrics

`prometheus-client` provides the registry — you choose how to expose it. A common approach is an HTTP endpoint:

```rust
use prometheus_client::encoding::text::encode;

async fn metrics_handler(registry: &Registry) -> String {
    let mut buf = String::new();
    encode(&mut buf, registry).unwrap();
    buf
}
```

### Example: HTTP Metrics Endpoint with Hyper

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use prometheus_client::registry::Registry;
use hotmint::consensus::metrics::ConsensusMetrics;

let registry = Arc::new(RwLock::new(Registry::default()));
let metrics = ConsensusMetrics::new(&mut registry.write().await);

// expose on /metrics endpoint using your preferred HTTP framework
// e.g., with axum:
//
// async fn metrics_endpoint(State(registry): State<Arc<RwLock<Registry>>>) -> String {
//     let mut buf = String::new();
//     let reg = registry.read().await;
//     prometheus_client::encoding::text::encode(&mut buf, &reg).unwrap();
//     buf
// }
```

## Grafana Dashboard

Key panels to include in a Grafana dashboard:

- **Block Rate**: `rate(hotmint_blocks_committed[5m])`
- **Current View**: `hotmint_current_view`
- **Current Height**: `hotmint_current_height`
- **Timeout Rate**: `rate(hotmint_view_timeouts[5m])`
- **Consecutive Timeouts**: `hotmint_consecutive_timeouts`
- **View Duration P99**: `histogram_quantile(0.99, rate(hotmint_view_duration_seconds_bucket[5m]))`
- **QC Formation Rate**: `rate(hotmint_qcs_formed[5m])`

Alert on:
- `hotmint_consecutive_timeouts > 3` — consensus may be stalled
- `rate(hotmint_blocks_committed[5m]) == 0` — no blocks committed in 5 minutes
- `hotmint_view_duration_seconds{quantile="0.99"} > 10` — views taking too long
