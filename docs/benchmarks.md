# Benchmarks

## How to Run

```bash
# Consensus layer benchmark (minimal application, local)
make bench-consensus

# EVM execution benchmark (revm transfers, local)
make bench-evm

# IPC benchmark (Unix domain socket overhead, local)
make bench-ipc

# All three benchmarks
make bench-all
```

## Real-World Multi-Machine Cluster

4 validators across heterogeneous machines connected over WAN:

| Node | OS | Arch | Network | Deployment Mode |
|------|----|------|---------|-----------------|
| V0 | macOS 15.4 | ARM64 | WiFi (NAT/DMZ) | Rust embedded (single-process) |
| V1 | Linux (Gentoo, EPYC 48-core) | x86_64 | Wired | Go ABCI (dual-process) |
| V2 | FreeBSD 14.4 | amd64 | Wired | Rust ABCI (dual-process) |
| V3 | FreeBSD 14.4 | amd64 | Wired | Rust embedded (single-process) |

### All Embedded (Single-Process)

| Metric | Value |
|--------|-------|
| Blocks committed (60s) | ~130 |
| **Throughput** | **~2.1 blocks/sec** |
| Block time | ~475ms |

### Mixed Deployment Modes

3 different deployment modes (Rust embedded + Go ABCI + Rust ABCI) in the same cluster:

| Metric | Value |
|--------|-------|
| Blocks committed (90s) | ~128 |
| **Throughput** | **~1.4 blocks/sec** |
| Block time | ~710ms |

All nodes committed identical block hashes. The WiFi node kept pace with wired nodes — the 3 wired nodes form a sufficient quorum (3/4 ≥ 2/3).

## Local Benchmarks (In-Process, No Network Overhead)

> **Environment**: Apple Silicon (M-series), 4 validators, `--release` profile. These measure raw consensus/execution performance without network latency.

### Consensus Layer (1KB payload/block)

| Config | Timeout | Blocks/sec | ms/block |
|:-------|:--------|:-----------|:---------|
| Fast | 500ms | ~1,860 | ~0.5 |
| Normal | 2,000ms | ~1,870 | ~0.5 |
| Conservative | 5,000ms | ~1,840 | ~0.5 |

### EVM Execution (10 ETH transfers/block, via revm)

| Config | Timeout | Blocks/sec | TX/sec | ms/block |
|:-------|:--------|:-----------|:-------|:---------|
| Fast | 500ms | ~1,510 | ~60,500 | ~0.7 |
| Normal | 2,000ms | ~1,580 | ~63,400 | ~0.6 |
| Conservative | 5,000ms | ~1,620 | ~64,700 | ~0.6 |

EVM execution adds ~15-20% overhead. At 10 transfers per block, the system achieves ~60,000+ EVM transactions per second across 4 validators.

### IPC (Unix Domain Socket, Protobuf Framing)

| Config | Timeout | Blocks/sec | ms/block | IPC overhead |
|:-------|:--------|:-----------|:---------|:-------------|
| Fast | 500ms | ~1,530 | ~0.7 | ~17% |
| Normal | 2,000ms | ~1,540 | ~0.6 | ~18% |
| Conservative | 5,000ms | ~1,540 | ~0.6 | ~16% |

The Unix socket IPC layer adds ~17% overhead compared to in-process direct calls. This is the cost of protobuf serialization + Unix socket round-trip per Application method call.

## What the Timeout Means

The `base_timeout_ms` parameter controls how long a validator waits before triggering a view change (timeout → Wish → TC → new leader):

- **Fast (500ms)**: Aggressive recovery from leader failures, but increases spurious timeouts in high-latency networks
- **Normal (2,000ms)**: Default — balances recovery speed with stability
- **Conservative (5,000ms)**: For high-latency or unreliable networks

In production with real P2P networking, the consensus round-trip time is dominated by network latency (typically 50-200ms per round over WAN, <1ms on LAN).
