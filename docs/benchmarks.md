# Benchmarks

> **Environment**: Apple Silicon (M-series), 4 validators, in-process channels (no network overhead). Measured with `--release` profile. Results are for reference only — real-world performance depends on network latency, hardware, and application complexity.

## How to Run

```bash
# Consensus layer benchmark (minimal application)
make bench-consensus

# EVM execution benchmark (revm transfers)
make bench-evm

# Both benchmarks
make bench-all
```

## Consensus Layer (minimal application, 1KB payload/block)

Each block carries a fixed 1KB payload. The application layer does no processing — this measures pure consensus throughput (proposal, voting, QC formation, commit).

| Config | Timeout | Blocks/sec | ms/block | Total blocks (10s) |
|:-------|:--------|:-----------|:---------|:-------------------|
| Fast | 500ms | ~1,860 | ~0.5 | ~18,600 |
| Normal | 2,000ms | ~1,870 | ~0.5 | ~18,700 |
| Conservative | 5,000ms | ~1,840 | ~0.5 | ~18,400 |

**Observation**: With in-process channels, the consensus timeout has negligible impact on steady-state throughput — blocks commit much faster than any timeout. The timeout only matters during view changes (leader failure or network delays).

## EVM Execution (10 ETH transfers/block, via revm)

Each block executes 10 ETH transfer transactions through revm's EVM engine. Each transaction is a real EVM state transition: balance debit/credit, nonce increment, gas accounting.

| Config | Timeout | Blocks/sec | TX/sec | ms/block | Total TX (10s) |
|:-------|:--------|:-----------|:-------|:---------|:---------------|
| Fast | 500ms | ~1,510 | ~60,500 | ~0.7 | ~605,000 |
| Normal | 2,000ms | ~1,580 | ~63,400 | ~0.6 | ~634,000 |
| Conservative | 5,000ms | ~1,620 | ~64,700 | ~0.6 | ~647,000 |

**Observation**: EVM execution adds ~15-20% overhead compared to pure consensus. At 10 transfers per block, the system achieves ~60,000+ EVM transactions per second across 4 validators. In a real deployment with network latency, throughput would be lower but the EVM execution overhead itself is minimal.

## What the Timeout Means

The `base_timeout_ms` parameter controls how long a validator waits before triggering a view change (timeout → Wish → TC → new leader). In local benchmarks with zero network latency:

- **Fast (500ms)**: Aggressive recovery from leader failures, but increases spurious timeouts in high-latency networks
- **Normal (2,000ms)**: Default — balances recovery speed with stability
- **Conservative (5,000ms)**: For high-latency or unreliable networks

In production with real P2P networking, the consensus round-trip time will be dominated by network latency (typically 50-200ms per round) rather than the timeout value.
