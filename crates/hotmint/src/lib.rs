//! # Hotmint
//!
//! A BFT consensus framework combining Tendermint's engineering ergonomics
//! with HotStuff-2's two-chain commit protocol.
//!
//! Hotmint is designed as a library crate (like `tendermint-core`) that
//! developers embed to build their own consensus-driven applications.
//!
//! # Crate Layout
//!
//! - [`types`] — Core data types: `Block`, `Vote`, `QC`, `ValidatorSet`, etc.
//! - [`crypto`] — Cryptographic primitives: Ed25519 signing, Blake3 hashing
//! - [`consensus`] — The HotStuff-2 state machine and engine
//! - [`storage`] — Persistent block/state storage (vsdb)
//! - [`network`] — P2P networking via litep2p
//! - [`mempool`] — Transaction buffering and deduplication
//! - [`abci`] — IPC proxy for out-of-process applications (Unix socket)
//! - [`api`] — JSON-RPC API for external interaction
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use hotmint::prelude::*;
//! use hotmint::consensus::{ConsensusEngine, ConsensusState};
//! use hotmint::consensus::application::Application;
//!
//! // Implement your application logic
//! struct MyApp;
//! impl Application for MyApp {
//!     fn on_commit(&self, block: &hotmint::types::Block, _ctx: &hotmint::types::BlockContext) -> ruc::Result<()> {
//!         println!("committed block at height {}", block.height);
//!         Ok(())
//!     }
//! }
//! ```

pub mod config;

/// Core data types: Block, Vote, QC, ValidatorSet, ConsensusMessage, etc.
pub use hotmint_types as types;

/// Cryptographic primitives: Ed25519 signing/verification, Blake3 hashing,
/// aggregate signatures.
pub use hotmint_crypto as crypto;

/// The HotStuff-2 consensus state machine, engine, pacemaker, and traits
/// (`Application`, `BlockStore`, `NetworkSink`).
pub use hotmint_consensus as consensus;

/// Persistent storage backends (vsdb).
pub use hotmint_storage as storage;

/// P2P networking via litep2p (notification + request-response protocols).
pub use hotmint_network as network;

/// Transaction mempool with FIFO ordering and deduplication.
pub use hotmint_mempool as mempool;

/// IPC proxy layer for running applications as separate processes
/// (Unix domain socket, length-prefixed CBOR).
pub use hotmint_abci as abci;

/// JSON-RPC API for external interaction (status, transaction submission).
pub use hotmint_api as api;

/// Staking infrastructure: validator lifecycle, delegation, slashing, rewards.
pub use hotmint_staking as staking;

/// Prelude: commonly used types re-exported for convenience.
pub mod prelude {
    pub use hotmint_types::{
        Block, BlockContext, BlockHash, ConsensusMessage, DoubleCertificate, EndBlockResponse,
        Epoch, EpochNumber, EquivocationProof, Height, QuorumCertificate, Signer,
        TimeoutCertificate, ValidatorId, ValidatorInfo, ValidatorSet, ValidatorUpdate, Verifier,
        ViewNumber, Vote, VoteType,
    };
}
