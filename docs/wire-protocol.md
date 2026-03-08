# Hotmint Wire Protocol Reference

This document defines the wire-level encoding standards that **all** hotmint
node implementations must follow, regardless of programming language or P2P
transport library.

## 1. Codec Framing (Consensus & Sync Messages)

Every consensus message and sync message on the wire uses a 1-byte tag prefix:

```
[0x00][raw CBOR payload]     — uncompressed
[0x01][zstd-compressed CBOR] — zstd level 3
```

### Encoding Rules

| Condition | Action |
|-----------|--------|
| CBOR payload ≤ 256 bytes | Prefix with `0x00`, send raw |
| CBOR payload > 256 bytes | Compress with zstd (level 3), prefix with `0x01` |

### Decoding Rules

1. Read the first byte (tag).
2. If `0x00`: the remainder is raw CBOR — decode directly.
3. If `0x01`: the remainder is zstd-compressed — decompress first, then CBOR-decode.
4. Any other tag: reject the message.

### Rationale

- Small messages (Vote, Prepare, Wish ≈ 100–200 bytes) are sent uncompressed —
  the zstd framing overhead would exceed the savings.
- Large messages (Propose with full Block, SyncResponse with multiple blocks)
  benefit significantly from compression.
- The tag byte makes the format self-describing — any implementation can detect
  whether decompression is needed without out-of-band negotiation.
- Compression is part of the **hotmint protocol**, not the P2P transport layer,
  ensuring interoperability across different P2P libraries.

### Scope

| Protocol | Uses codec framing |
|----------|--------------------|
| `/hotmint/consensus/notif/1` | Yes |
| `/hotmint/consensus/reqresp/1` | Yes |
| `/hotmint/sync/1` | Yes |
| `/hotmint/pex/1` | No (raw CBOR, small peer-exchange messages) |

## 2. CBOR Serialization

All structured data is serialized using [CBOR (RFC 8949)](https://www.rfc-editor.org/rfc/rfc8949)
via serde's default externally-tagged enum representation.

### Enum Encoding

Rust enums are encoded as CBOR maps with a single string key:

```cbor
{"Propose": {"block": ..., "justify": ..., "double_cert": ..., "signature": ...}}
{"VoteMsg": {...}}
```

### Newtype Wrappers

Types like `Height(u64)`, `ViewNumber(u64)`, `ValidatorId(u64)` are
transparent — they serialize as the inner value directly.

### Fixed-Size Arrays

`BlockHash([u8; 32])` is serialized as a CBOR array of 32 integers (not a
CBOR byte string). This is a consequence of serde_cbor_2's handling of
fixed-size arrays.

`Vec<u8>` fields (e.g., `payload`, `Signature.0`, `PublicKey.0`) are
serialized as CBOR byte strings.

## 3. ABCI IPC Protocol

The ABCI (Application Binary Consensus Interface) uses a **separate**
serialization format for cross-language interoperability:

- **Transport**: Unix domain socket
- **Framing**: 4-byte little-endian `u32` length prefix + protobuf payload
- **Serialization**: Protocol Buffers (see `proto/abci.proto`)
- **Max frame size**: 64 MB

The ABCI protocol is defined in `proto/abci.proto` and is the canonical
schema for Go (and other language) SDK implementations.

### Request/Response Flow

```
Engine (Rust) → [4-byte LE len][protobuf Request]  → Application (Go/Rust)
Engine (Rust) ← [4-byte LE len][protobuf Response] ← Application (Go/Rust)
```

## 4. Block Hash Computation

```
block_hash = Blake3(height_le64 || parent_hash_32 || view_le64 || proposer_le64 || app_hash_32 || payload)
```

All fields are serialized in little-endian byte order. The `hash` field
itself is excluded from the computation to avoid circularity.

`app_hash` is the application state root after executing the **parent** block
(delayed inclusion, following the CometBFT model).

## 5. Real-World Benchmark (Multi-Machine Cluster)

4-node cluster across heterogeneous machines and OS:

| Node | OS | Arch | Network | Deployment Mode |
|------|----|------|---------|-----------------|
| V0 | macOS 15.4 | ARM64 | WiFi (NAT/DMZ) | Rust embedded (single-process) |
| V1 | Linux (Gentoo, EPYC 48-core) | x86_64 | Wired LAN | Go ABCI (dual-process) |
| V2 | FreeBSD 14.4 | amd64 | Wired LAN | Rust ABCI (dual-process) |
| V3 | FreeBSD 14.4 | amd64 | Wired LAN | Rust embedded (single-process) |

### Scenario A: All Rust Embedded (Single-Process)

All 4 nodes using `cluster-node` (embedded NoopApplication):

- **~130 blocks in 60 seconds → ~2.1 blocks/sec**
- All nodes committed identical block hashes
- WiFi node (V0) kept pace with wired nodes
- Leader rotation (V0→V1→V2→V3) worked correctly

### Scenario B: Mixed Deployment Modes

3 different deployment modes in the same cluster:

- **~128 blocks in 90 seconds → ~1.4 blocks/sec** (full cluster)
- WiFi node joined late and synced to ~2.0 blocks/sec once caught up
- Go ABCI, Rust ABCI, and Rust embedded modes fully interoperable
- Block hashes consistent across all nodes and all deployment modes

### Key Findings

- The HotStuff-2 two-chain commit rule tolerates the WiFi node's higher
  latency — the 3 wired nodes form a quorum (3/4 ≥ 2/3) independently
- Cross-OS interop (macOS/Linux/FreeBSD) works without issues
- ABCI dual-process modes (Go and Rust) produce the same consensus
  results as the embedded single-process mode
- Network latency across WAN is the dominant factor in block time;
  on a LAN cluster, expect significantly higher throughput

## 6. Version History

| Version | Changes |
|---------|---------|
| 0.6.0 | Added `app_hash` to Block and EndBlockResponse; wire codec with zstd compression; ABCI switched from CBOR to protobuf |
| 0.5.0 | Initial release with CBOR-only encoding |
