use ruc::*;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::application::Application;
use crate::commit::try_commit;
use crate::leader;
use crate::network::NetworkSink;
use crate::pacemaker::{Pacemaker, PacemakerConfig};
use crate::state::{ConsensusState, ViewStep};
use crate::store::BlockStore;
use crate::view_protocol::{self, ViewEntryTrigger};
use crate::vote_collector::VoteCollector;

use hotmint_types::epoch::Epoch;
use hotmint_types::vote::VoteType;
use hotmint_types::*;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Shared block store type used by the engine, RPC, and sync responder.
pub type SharedBlockStore = Arc<RwLock<Box<dyn BlockStore>>>;

/// Trait for persisting critical consensus state across restarts.
pub trait StatePersistence: Send {
    fn save_current_view(&mut self, view: ViewNumber);
    fn save_locked_qc(&mut self, qc: &QuorumCertificate);
    fn save_highest_qc(&mut self, qc: &QuorumCertificate);
    fn save_last_committed_height(&mut self, height: Height);
    fn save_current_epoch(&mut self, epoch: &Epoch);
    fn save_last_app_hash(&mut self, hash: BlockHash);
    fn flush(&self);
}

pub struct ConsensusEngine {
    state: ConsensusState,
    store: SharedBlockStore,
    network: Box<dyn NetworkSink>,
    app: Box<dyn Application>,
    signer: Box<dyn Signer>,
    verifier: Box<dyn Verifier>,
    vote_collector: VoteCollector,
    pacemaker: Pacemaker,
    pacemaker_config: PacemakerConfig,
    msg_rx: mpsc::Receiver<(Option<ValidatorId>, ConsensusMessage)>,
    /// Collected unique status cert senders (for leader, per view)
    status_senders: HashSet<ValidatorId>,
    /// The QC formed in this view's first voting round (used to build DoubleCert)
    current_view_qc: Option<QuorumCertificate>,
    /// Pending epoch transition (set by try_commit, applied in advance_view_to)
    pending_epoch: Option<Epoch>,
    /// Optional state persistence (for crash recovery).
    persistence: Option<Box<dyn StatePersistence>>,
}

/// Configuration for ConsensusEngine.
pub struct EngineConfig {
    pub verifier: Box<dyn Verifier>,
    pub pacemaker: Option<PacemakerConfig>,
    pub persistence: Option<Box<dyn StatePersistence>>,
}

impl EngineConfig {
    /// Create an `EngineConfig` with the given verifier and defaults
    /// (no custom pacemaker, no persistence).
    pub fn new(verifier: Box<dyn Verifier>) -> Self {
        Self {
            verifier,
            pacemaker: None,
            persistence: None,
        }
    }

    /// Set a custom pacemaker configuration.
    pub fn with_pacemaker(mut self, pacemaker: PacemakerConfig) -> Self {
        self.pacemaker = Some(pacemaker);
        self
    }

    /// Set a state persistence backend.
    pub fn with_persistence(mut self, persistence: Box<dyn StatePersistence>) -> Self {
        self.persistence = Some(persistence);
        self
    }
}

/// Builder for constructing a `ConsensusEngine` with a fluent API.
///
/// # Example
/// ```rust,ignore
/// let engine = ConsensusEngineBuilder::new()
///     .state(state)
///     .store(store)
///     .network(network)
///     .app(app)
///     .signer(signer)
///     .messages(msg_rx)
///     .verifier(verifier)
///     .build()
///     .expect("all required fields must be set");
/// ```
pub struct ConsensusEngineBuilder {
    state: Option<ConsensusState>,
    store: Option<SharedBlockStore>,
    network: Option<Box<dyn NetworkSink>>,
    app: Option<Box<dyn Application>>,
    signer: Option<Box<dyn Signer>>,
    msg_rx: Option<mpsc::Receiver<(Option<ValidatorId>, ConsensusMessage)>>,
    verifier: Option<Box<dyn Verifier>>,
    pacemaker: Option<PacemakerConfig>,
    persistence: Option<Box<dyn StatePersistence>>,
}

impl ConsensusEngineBuilder {
    pub fn new() -> Self {
        Self {
            state: None,
            store: None,
            network: None,
            app: None,
            signer: None,
            msg_rx: None,
            verifier: None,
            pacemaker: None,
            persistence: None,
        }
    }

    pub fn state(mut self, state: ConsensusState) -> Self {
        self.state = Some(state);
        self
    }

    pub fn store(mut self, store: SharedBlockStore) -> Self {
        self.store = Some(store);
        self
    }

    pub fn network(mut self, network: Box<dyn NetworkSink>) -> Self {
        self.network = Some(network);
        self
    }

    pub fn app(mut self, app: Box<dyn Application>) -> Self {
        self.app = Some(app);
        self
    }

    pub fn signer(mut self, signer: Box<dyn Signer>) -> Self {
        self.signer = Some(signer);
        self
    }

    pub fn messages(mut self, msg_rx: mpsc::Receiver<(Option<ValidatorId>, ConsensusMessage)>) -> Self {
        self.msg_rx = Some(msg_rx);
        self
    }

    pub fn verifier(mut self, verifier: Box<dyn Verifier>) -> Self {
        self.verifier = Some(verifier);
        self
    }

    pub fn pacemaker(mut self, config: PacemakerConfig) -> Self {
        self.pacemaker = Some(config);
        self
    }

    pub fn persistence(mut self, persistence: Box<dyn StatePersistence>) -> Self {
        self.persistence = Some(persistence);
        self
    }

    pub fn build(self) -> ruc::Result<ConsensusEngine> {
        let state = self.state.ok_or_else(|| ruc::eg!("state is required"))?;
        let store = self.store.ok_or_else(|| ruc::eg!("store is required"))?;
        let network = self
            .network
            .ok_or_else(|| ruc::eg!("network is required"))?;
        let app = self.app.ok_or_else(|| ruc::eg!("app is required"))?;
        let signer = self.signer.ok_or_else(|| ruc::eg!("signer is required"))?;
        let msg_rx = self
            .msg_rx
            .ok_or_else(|| ruc::eg!("messages (msg_rx) is required"))?;
        let verifier = self
            .verifier
            .ok_or_else(|| ruc::eg!("verifier is required"))?;

        let config = EngineConfig {
            verifier,
            pacemaker: self.pacemaker,
            persistence: self.persistence,
        };

        Ok(ConsensusEngine::new(
            state, store, network, app, signer, msg_rx, config,
        ))
    }
}

impl Default for ConsensusEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsensusEngine {
    pub fn new(
        state: ConsensusState,
        store: SharedBlockStore,
        network: Box<dyn NetworkSink>,
        app: Box<dyn Application>,
        signer: Box<dyn Signer>,
        msg_rx: mpsc::Receiver<(Option<ValidatorId>, ConsensusMessage)>,
        config: EngineConfig,
    ) -> Self {
        let pc = config.pacemaker.unwrap_or_default();
        Self {
            state,
            store,
            network,
            app,
            signer,
            verifier: config.verifier,
            vote_collector: VoteCollector::new(),
            pacemaker: Pacemaker::with_config(pc.clone()),
            pacemaker_config: pc,
            msg_rx,
            status_senders: HashSet::new(),
            current_view_qc: None,
            pending_epoch: None,
            persistence: config.persistence,
        }
    }

    /// Bootstrap and start the event loop.
    /// If persisted state was restored (current_view > 1), skip genesis bootstrap.
    pub async fn run(mut self) {
        if self.state.current_view.as_u64() <= 1 {
            self.enter_genesis_view();
        } else {
            info!(
                validator = %self.state.validator_id,
                view = %self.state.current_view,
                height = %self.state.last_committed_height,
                "resuming from persisted state"
            );
            self.pacemaker.reset_timer();
        }

        loop {
            let deadline = self.pacemaker.sleep_until_deadline();
            tokio::pin!(deadline);

            tokio::select! {
                Some((sender, msg)) = self.msg_rx.recv() => {
                    if let Err(e) = self.handle_message(sender, msg) {
                        warn!(validator = %self.state.validator_id, error = %e, "error handling message");
                    }
                }
                _ = &mut deadline => {
                    self.handle_timeout();
                }
            }
        }
    }

    fn enter_genesis_view(&mut self) {
        // Create a synthetic genesis QC so the first leader can propose
        let genesis_qc = QuorumCertificate {
            block_hash: BlockHash::GENESIS,
            view: ViewNumber::GENESIS,
            aggregate_signature: AggregateSignature::new(
                self.state.validator_set.validator_count(),
            ),
        };
        self.state.highest_qc = Some(genesis_qc);

        let view = ViewNumber(1);
        view_protocol::enter_view(
            &mut self.state,
            view,
            ViewEntryTrigger::Genesis,
            self.network.as_ref(),
            self.signer.as_ref(),
        );
        self.pacemaker.reset_timer();

        // If leader of genesis view, propose immediately
        if self.state.is_leader() {
            self.state.step = ViewStep::WaitingForStatus;
            // In genesis, skip status wait — propose directly
            self.try_propose();
        }
    }

    fn try_propose(&mut self) {
        let mut store = self.store.write().unwrap();
        match view_protocol::propose(
            &mut self.state,
            store.as_mut(),
            self.network.as_ref(),
            self.app.as_ref(),
            self.signer.as_ref(),
        ) {
            Ok(block) => {
                drop(store);
                // Leader votes for its own block
                self.leader_self_vote(block.hash);
            }
            Err(e) => {
                warn!(
                    validator = %self.state.validator_id,
                    error = %e,
                    "failed to propose"
                );
            }
        }
    }

    fn leader_self_vote(&mut self, block_hash: BlockHash) {
        let vote_bytes = Vote::signing_bytes(self.state.current_view, &block_hash, VoteType::Vote);
        let signature = self.signer.sign(&vote_bytes);
        let vote = Vote {
            block_hash,
            view: self.state.current_view,
            validator: self.state.validator_id,
            signature,
            vote_type: VoteType::Vote,
        };
        match self
            .vote_collector
            .add_vote(&self.state.validator_set, vote)
        {
            Ok(result) => {
                self.handle_equivocation(&result);
                if let Some(qc) = result.qc {
                    self.on_qc_formed(qc);
                }
            }
            Err(e) => warn!(error = %e, "failed to add self vote"),
        }
    }
}

/// Verify the per-sender individual signature on a consensus message before relaying.
///
/// Only checks signatures that can be attributed to a single known validator using the
/// provided key map.  Aggregate certificates (TimeoutCert, justify/prepare QCs) are
/// intentionally not fully re-verified here — the receiving engine always does the full
/// check.  Messages whose signing bytes depend on receiver state (StatusCert needs
/// `current_view`) are also allowed through; the engine will reject them if invalid.
///
/// `ordered_validators` is the validator list in round-robin order (same order used by
/// `ValidatorSet::leader_for_view`).  Pass an empty slice to skip the leader-for-view
/// check (e.g., in tests where the set is not available).
///
/// Returns `false` when:
/// - The claimed sender is not in `validator_keys` (unknown/non-validator peer), OR
/// - The individual signature is cryptographically invalid, OR
/// - For Prepare: the sender is not the leader for the certificate's view.
pub fn verify_relay_sender(
    sender: ValidatorId,
    msg: &ConsensusMessage,
    validator_keys: &HashMap<ValidatorId, hotmint_types::crypto::PublicKey>,
    ordered_validators: &[ValidatorId],
) -> bool {
    use hotmint_crypto::Ed25519Verifier;
    use hotmint_types::vote::Vote;
    use hotmint_types::Verifier;
    let verifier = Ed25519Verifier;
    match msg {
        ConsensusMessage::Propose {
            block,
            justify,
            signature,
            ..
        } => {
            let Some(pk) = validator_keys.get(&block.proposer) else {
                return false;
            };
            let bytes = crate::view_protocol::proposal_signing_bytes(block, justify);
            Verifier::verify(&verifier, pk, &bytes, signature)
        }
        ConsensusMessage::VoteMsg(vote) | ConsensusMessage::Vote2Msg(vote) => {
            let Some(pk) = validator_keys.get(&vote.validator) else {
                return false;
            };
            let bytes = Vote::signing_bytes(vote.view, &vote.block_hash, vote.vote_type);
            Verifier::verify(&verifier, pk, &bytes, &vote.signature)
        }
        ConsensusMessage::Prepare {
            certificate,
            signature,
        } => {
            // Prepare is broadcast by the current leader. Verify that the relay
            // sender is the leader for this view, then check the signature.
            if !ordered_validators.is_empty() {
                let n = ordered_validators.len();
                let expected_leader =
                    ordered_validators[certificate.view.as_u64() as usize % n];
                if sender != expected_leader {
                    return false;
                }
            }
            let Some(pk) = validator_keys.get(&sender) else {
                return false;
            };
            let bytes = crate::view_protocol::prepare_signing_bytes(certificate);
            Verifier::verify(&verifier, pk, &bytes, signature)
        }
        ConsensusMessage::Wish {
            target_view,
            validator,
            highest_qc,
            signature,
        } => {
            let Some(pk) = validator_keys.get(validator) else {
                return false;
            };
            let bytes = crate::pacemaker::wish_signing_bytes(*target_view, highest_qc.as_ref());
            Verifier::verify(&verifier, pk, &bytes, signature)
        }
        // TimeoutCert: aggregate signature — engine verifies with full ValidatorSet.
        // StatusCert: signing bytes need current_view — engine verifies.
        ConsensusMessage::TimeoutCert(_) | ConsensusMessage::StatusCert { .. } => true,
    }
}

impl ConsensusEngine {
    /// Verify the cryptographic signature on an inbound consensus message.
    /// Returns false (and logs a warning) if verification fails.
    /// Messages from past views are skipped (they'll be dropped by handle_message anyway).
    fn verify_message(&self, msg: &ConsensusMessage) -> bool {
        // Skip verification for non-Propose past-view messages — these may have
        // been signed by a previous epoch's validator set. They'll be dropped by
        // view checks. Propose messages are always verified because they may still
        // be stored (for chain continuity in fast-forward).
        let msg_view = match msg {
            ConsensusMessage::Propose { .. } => None, // always verify proposals
            ConsensusMessage::VoteMsg(v) | ConsensusMessage::Vote2Msg(v) => Some(v.view),
            ConsensusMessage::Prepare { certificate, .. } => Some(certificate.view),
            ConsensusMessage::Wish { target_view, .. } => Some(*target_view),
            ConsensusMessage::TimeoutCert(tc) => Some(ViewNumber(tc.view.as_u64() + 1)),
            ConsensusMessage::StatusCert { .. } => None,
        };
        if let Some(v) = msg_view
            && v < self.state.current_view
        {
            return true; // will be dropped by handler
        }

        let vs = &self.state.validator_set;
        match msg {
            ConsensusMessage::Propose {
                block,
                justify,
                signature,
                ..
            } => {
                let proposer = vs.get(block.proposer);
                let Some(vi) = proposer else {
                    warn!(proposer = %block.proposer, "propose from unknown validator");
                    return false;
                };
                let bytes = view_protocol::proposal_signing_bytes(block, justify);
                if !self.verifier.verify(&vi.public_key, &bytes, signature) {
                    warn!(proposer = %block.proposer, "invalid proposal signature");
                    return false;
                }
                // Verify justify QC aggregate signature (skip genesis QC which has no signers)
                if justify.aggregate_signature.count() > 0 {
                    let qc_bytes =
                        Vote::signing_bytes(justify.view, &justify.block_hash, VoteType::Vote);
                    if !self
                        .verifier
                        .verify_aggregate(vs, &qc_bytes, &justify.aggregate_signature)
                    {
                        warn!(proposer = %block.proposer, "invalid justify QC aggregate signature");
                        return false;
                    }
                    if !hotmint_crypto::has_quorum(vs, &justify.aggregate_signature) {
                        warn!(proposer = %block.proposer, "justify QC below quorum threshold");
                        return false;
                    }
                }
                true
            }
            ConsensusMessage::VoteMsg(vote) | ConsensusMessage::Vote2Msg(vote) => {
                let Some(vi) = vs.get(vote.validator) else {
                    warn!(validator = %vote.validator, "vote from unknown validator");
                    return false;
                };
                let bytes = Vote::signing_bytes(vote.view, &vote.block_hash, vote.vote_type);
                if !self
                    .verifier
                    .verify(&vi.public_key, &bytes, &vote.signature)
                {
                    warn!(validator = %vote.validator, "invalid vote signature");
                    return false;
                }
                true
            }
            ConsensusMessage::Prepare {
                certificate,
                signature,
            } => {
                // Verify the leader's signature on the prepare message
                let leader = vs.leader_for_view(certificate.view);
                let bytes = view_protocol::prepare_signing_bytes(certificate);
                if !self.verifier.verify(&leader.public_key, &bytes, signature) {
                    warn!(view = %certificate.view, "invalid prepare signature");
                    return false;
                }
                // Also verify the QC's aggregate signature and quorum
                let qc_bytes =
                    Vote::signing_bytes(certificate.view, &certificate.block_hash, VoteType::Vote);
                if !self
                    .verifier
                    .verify_aggregate(vs, &qc_bytes, &certificate.aggregate_signature)
                {
                    warn!(view = %certificate.view, "invalid QC aggregate signature");
                    return false;
                }
                if !hotmint_crypto::has_quorum(vs, &certificate.aggregate_signature) {
                    warn!(view = %certificate.view, "Prepare QC below quorum threshold");
                    return false;
                }
                true
            }
            ConsensusMessage::Wish {
                target_view,
                validator,
                highest_qc,
                signature,
            } => {
                let Some(vi) = vs.get(*validator) else {
                    warn!(validator = %validator, "wish from unknown validator");
                    return false;
                };
                // Signing bytes bind both target_view and highest_qc to prevent replay.
                let bytes = crate::pacemaker::wish_signing_bytes(*target_view, highest_qc.as_ref());
                if !self.verifier.verify(&vi.public_key, &bytes, signature) {
                    warn!(validator = %validator, "invalid wish signature");
                    return false;
                }
                true
            }
            ConsensusMessage::TimeoutCert(tc) => {
                // The TC's aggregate signature is a collection of individual Ed25519 signatures,
                // each signed over wish_signing_bytes(target_view, signer_highest_qc).
                // Because each validator may have a different highest_qc, we verify per-signer
                // using tc.highest_qcs[i] (indexed by validator slot).
                // This also enforces quorum: we sum voting power of verified signers.
                let target_view = ViewNumber(tc.view.as_u64() + 1);
                let n = vs.validator_count();
                if tc.aggregate_signature.signers.len() != n {
                    warn!(view = %tc.view, "TC signers bitfield length mismatch");
                    return false;
                }
                let mut sig_idx = 0usize;
                let mut power = 0u64;
                for (i, &signed) in tc.aggregate_signature.signers.iter().enumerate() {
                    if !signed {
                        continue;
                    }
                    let Some(vi) = vs.validators().get(i) else {
                        warn!(view = %tc.view, validator_idx = i, "TC signer index out of validator set");
                        return false;
                    };
                    let hqc = tc.highest_qcs.get(i).and_then(|h| h.as_ref());
                    let bytes = crate::pacemaker::wish_signing_bytes(target_view, hqc);
                    if sig_idx >= tc.aggregate_signature.signatures.len() {
                        warn!(view = %tc.view, "TC aggregate_signature has fewer sigs than signers");
                        return false;
                    }
                    if !self
                        .verifier
                        .verify(&vi.public_key, &bytes, &tc.aggregate_signature.signatures[sig_idx])
                    {
                        warn!(view = %tc.view, validator = %vi.id, "TC signer signature invalid");
                        return false;
                    }
                    power += vs.power_of(vi.id);
                    sig_idx += 1;
                }
                if sig_idx != tc.aggregate_signature.signatures.len() {
                    warn!(view = %tc.view, "TC has extra signatures beyond bitfield");
                    return false;
                }
                if power < vs.quorum_threshold() {
                    warn!(view = %tc.view, power, threshold = vs.quorum_threshold(), "TC insufficient quorum");
                    return false;
                }
                true
            }
            ConsensusMessage::StatusCert {
                locked_qc,
                validator,
                signature,
            } => {
                let Some(vi) = vs.get(*validator) else {
                    warn!(validator = %validator, "status from unknown validator");
                    return false;
                };
                let bytes = view_protocol::status_signing_bytes(self.state.current_view, locked_qc);
                if !self.verifier.verify(&vi.public_key, &bytes, signature) {
                    warn!(validator = %validator, "invalid status signature");
                    return false;
                }
                true
            }
        }
    }

    fn handle_message(&mut self, _sender: Option<ValidatorId>, msg: ConsensusMessage) -> Result<()> {
        if !self.verify_message(&msg) {
            return Ok(());
        }

        match msg {
            ConsensusMessage::Propose {
                block,
                justify,
                double_cert,
                signature: _,
            } => {
                let block = *block;
                let justify = *justify;
                let double_cert = double_cert.map(|dc| *dc);

                // If proposal is from a future view, advance to it first
                if block.view > self.state.current_view {
                    if let Some(ref dc) = double_cert {
                        if !self.validate_double_cert(dc) {
                            return Ok(());
                        }

                        // Fast-forward via double cert
                        self.apply_commit(dc, "fast-forward");
                        self.state.highest_double_cert = Some(dc.clone());
                        self.advance_view_to(block.view, ViewEntryTrigger::DoubleCert(dc.clone()));
                    } else {
                        return Ok(());
                    }
                } else if block.view < self.state.current_view {
                    // Still store blocks from past views if we haven't committed
                    // that height yet. This handles the case where fast-forward
                    // advanced our view but we missed storing the block from the
                    // earlier proposal. Without this, chain commits that walk
                    // the parent chain would fail with "block not found".
                    if block.height > self.state.last_committed_height {
                        // Verify block hash before storing past-view blocks
                        let expected = hotmint_crypto::compute_block_hash(&block);
                        if block.hash == expected {
                            let mut store = self.store.write().unwrap();
                            store.put_block(block);
                        }
                    }
                    return Ok(());
                }

                let mut store = self.store.write().unwrap();

                // R-25: verify any DoubleCert in the same-view proposal path.
                // The future-view path already calls validate_double_cert; the same-view path
                // passes the DC straight to on_proposal → try_commit without verification.
                // A Byzantine leader could inject a forged DC to trigger incorrect commits.
                if let Some(ref dc) = double_cert {
                    if !self.validate_double_cert(dc) {
                        return Ok(());
                    }
                }

                // R-28: persist justify QC as commit evidence for the block it certifies.
                // When blocks are committed via the 2-chain rule (possibly multiple blocks at
                // once), the innermost block gets its own commit QC, but ancestor blocks only
                // get the chain-rule commit and have no stored QC.  Storing the justify QC here
                // ensures that sync responders can later serve those ancestor blocks with proof.
                if justify.aggregate_signature.count() > 0 {
                    if let Some(justified_block) = store.get_block(&justify.block_hash) {
                        if store.get_commit_qc(justified_block.height).is_none() {
                            store.put_commit_qc(justified_block.height, justify.clone());
                        }
                    }
                }

                let maybe_pending = view_protocol::on_proposal(
                    &mut self.state,
                    view_protocol::ProposalData {
                        block,
                        justify,
                        double_cert,
                    },
                    store.as_mut(),
                    self.network.as_ref(),
                    self.app.as_ref(),
                    self.signer.as_ref(),
                )
                .c(d!())?;
                drop(store);

                if let Some(epoch) = maybe_pending {
                    self.pending_epoch = Some(epoch);
                }
            }

            ConsensusMessage::VoteMsg(vote) => {
                if vote.view != self.state.current_view {
                    return Ok(());
                }
                if !self.state.is_leader() {
                    return Ok(());
                }
                if vote.vote_type != VoteType::Vote {
                    return Ok(());
                }

                let result = self
                    .vote_collector
                    .add_vote(&self.state.validator_set, vote)
                    .c(d!())?;
                self.handle_equivocation(&result);
                if let Some(qc) = result.qc {
                    self.on_qc_formed(qc);
                }
            }

            ConsensusMessage::Prepare {
                certificate,
                signature: _,
            } => {
                if certificate.view < self.state.current_view {
                    return Ok(());
                }
                if certificate.view == self.state.current_view {
                    // Validate the Prepare's block app_hash if we have the block in
                    // store. Prevents locking onto a block whose app_hash diverges from
                    // our local state. When the block is absent (node caught up via TC),
                    // we defer to the QC's 2f+1 signatures for safety.
                    if self.app.tracks_app_hash() {
                        let store = self.store.read().unwrap();
                        if let Some(block) = store.get_block(&certificate.block_hash) {
                            if block.app_hash != self.state.last_app_hash {
                                warn!(
                                    block_app_hash = %block.app_hash,
                                    local_app_hash = %self.state.last_app_hash,
                                    "prepare block app_hash mismatch, ignoring"
                                );
                                return Ok(());
                            }
                        }
                    }
                    view_protocol::on_prepare(
                        &mut self.state,
                        certificate,
                        self.network.as_ref(),
                        self.signer.as_ref(),
                    );
                }
            }

            ConsensusMessage::Vote2Msg(vote) => {
                if vote.vote_type != VoteType::Vote2 {
                    return Ok(());
                }
                if vote.view != self.state.current_view {
                    return Ok(());
                }
                let result = self
                    .vote_collector
                    .add_vote(&self.state.validator_set, vote)
                    .c(d!())?;
                self.handle_equivocation(&result);
                if let Some(outer_qc) = result.qc {
                    self.on_double_cert_formed(outer_qc);
                }
            }

            ConsensusMessage::Wish {
                target_view,
                validator,
                highest_qc,
                signature,
            } => {
                // Validate carried highest_qc (C4 mitigation).
                // Both signature authenticity and 2f+1 quorum weight must pass.
                if let Some(ref qc) = highest_qc
                    && qc.aggregate_signature.count() > 0
                {
                    let qc_bytes = Vote::signing_bytes(qc.view, &qc.block_hash, VoteType::Vote);
                    if !self.verifier.verify_aggregate(
                        &self.state.validator_set,
                        &qc_bytes,
                        &qc.aggregate_signature,
                    ) {
                        warn!(validator = %validator, "wish carries invalid highest_qc signature");
                        return Ok(());
                    }
                    if !hotmint_crypto::has_quorum(&self.state.validator_set, &qc.aggregate_signature) {
                        warn!(validator = %validator, "wish carries highest_qc without quorum");
                        return Ok(());
                    }
                }

                if let Some(tc) = self.pacemaker.add_wish(
                    &self.state.validator_set,
                    target_view,
                    validator,
                    highest_qc,
                    signature,
                ) {
                    info!(
                        validator = %self.state.validator_id,
                        view = %tc.view,
                        "TC formed, advancing view"
                    );
                    self.network
                        .broadcast(ConsensusMessage::TimeoutCert(tc.clone()));
                    self.advance_view(ViewEntryTrigger::TimeoutCert(tc));
                }
            }

            ConsensusMessage::TimeoutCert(tc) => {
                if self.pacemaker.should_relay_tc(&tc) {
                    self.network
                        .broadcast(ConsensusMessage::TimeoutCert(tc.clone()));
                }
                let new_view = ViewNumber(tc.view.as_u64() + 1);
                if new_view > self.state.current_view {
                    self.advance_view(ViewEntryTrigger::TimeoutCert(tc));
                }
            }

            ConsensusMessage::StatusCert {
                locked_qc,
                validator,
                signature: _,
            } => {
                if self.state.is_leader() && self.state.step == ViewStep::WaitingForStatus {
                    if let Some(ref qc) = locked_qc {
                        self.state.update_highest_qc(qc);
                    }
                    self.status_senders.insert(validator);
                    let status_power: u64 = self
                        .status_senders
                        .iter()
                        .map(|v| self.state.validator_set.power_of(*v))
                        .sum();
                    // Leader's own power counts toward quorum
                    let total_power =
                        status_power + self.state.validator_set.power_of(self.state.validator_id);
                    if total_power >= self.state.validator_set.quorum_threshold() {
                        self.try_propose();
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_equivocation(&self, result: &crate::vote_collector::VoteResult) {
        if let Some(ref proof) = result.equivocation {
            warn!(
                validator = %proof.validator,
                view = %proof.view,
                "equivocation detected!"
            );
            if let Err(e) = self.app.on_evidence(proof) {
                warn!(error = %e, "on_evidence callback failed");
            }
        }
    }

    fn on_qc_formed(&mut self, qc: QuorumCertificate) {
        // Save the QC so we can reliably pair it when forming a DoubleCert
        self.current_view_qc = Some(qc.clone());

        view_protocol::on_votes_collected(
            &mut self.state,
            qc.clone(),
            self.network.as_ref(),
            self.signer.as_ref(),
        );

        // Leader also does vote2 for its own prepare (self-vote for step 5)
        let vote_bytes =
            Vote::signing_bytes(self.state.current_view, &qc.block_hash, VoteType::Vote2);
        let signature = self.signer.sign(&vote_bytes);
        let vote = Vote {
            block_hash: qc.block_hash,
            view: self.state.current_view,
            validator: self.state.validator_id,
            signature,
            vote_type: VoteType::Vote2,
        };

        // Lock on this QC
        self.state.update_locked_qc(&qc);

        let next_leader_id =
            leader::next_leader(&self.state.validator_set, self.state.current_view);
        if next_leader_id == self.state.validator_id {
            // We are the next leader, collect vote2 locally
            match self
                .vote_collector
                .add_vote(&self.state.validator_set, vote)
            {
                Ok(result) => {
                    self.handle_equivocation(&result);
                    if let Some(outer_qc) = result.qc {
                        self.on_double_cert_formed(outer_qc);
                    }
                }
                Err(e) => warn!(error = %e, "failed to add self vote2"),
            }
        } else {
            self.network
                .send_to(next_leader_id, ConsensusMessage::Vote2Msg(vote));
        }
    }

    fn on_double_cert_formed(&mut self, outer_qc: QuorumCertificate) {
        // Use the QC we explicitly saved from this view's first voting round
        let inner_qc = match self.current_view_qc.take() {
            Some(qc) if qc.block_hash == outer_qc.block_hash => qc,
            _ => {
                // Fallback to locked_qc or highest_qc
                match &self.state.locked_qc {
                    Some(qc) if qc.block_hash == outer_qc.block_hash => qc.clone(),
                    _ => match &self.state.highest_qc {
                        Some(qc) if qc.block_hash == outer_qc.block_hash => qc.clone(),
                        _ => {
                            warn!(
                                validator = %self.state.validator_id,
                                "double cert formed but can't find matching inner QC"
                            );
                            return;
                        }
                    },
                }
            }
        };

        let dc = DoubleCertificate { inner_qc, outer_qc };

        info!(
            validator = %self.state.validator_id,
            view = %self.state.current_view,
            hash = %dc.inner_qc.block_hash,
            "double certificate formed, committing"
        );

        // Commit
        self.apply_commit(&dc, "double-cert");

        self.state.highest_double_cert = Some(dc.clone());

        // Advance to next view — as new leader, include DC in proposal
        self.advance_view(ViewEntryTrigger::DoubleCert(dc));
    }

    fn handle_timeout(&mut self) {
        // Skip wish building/signing entirely when we have no voting power (fullnodes).
        // build_wish involves a cryptographic signing operation that serves no purpose
        // when the wish will never be broadcast or counted toward a TC.
        let has_power = self.state.validator_set.power_of(self.state.validator_id) > 0;
        if !has_power {
            self.pacemaker.on_timeout();
            return;
        }

        info!(
            validator = %self.state.validator_id,
            view = %self.state.current_view,
            "view timeout, sending wish"
        );

        let wish = self.pacemaker.build_wish(
            self.state.current_view,
            self.state.validator_id,
            self.state.highest_qc.clone(),
            self.signer.as_ref(),
        );

        self.network.broadcast(wish.clone());

        // Also process our own wish
        if let ConsensusMessage::Wish {
            target_view,
            validator,
            highest_qc,
            signature,
        } = wish
            && let Some(tc) = self.pacemaker.add_wish(
                &self.state.validator_set,
                target_view,
                validator,
                highest_qc,
                signature,
            )
        {
            self.network
                .broadcast(ConsensusMessage::TimeoutCert(tc.clone()));
            self.advance_view(ViewEntryTrigger::TimeoutCert(tc));
            return;
        }

        // Exponential backoff on repeated timeouts
        self.pacemaker.on_timeout();
    }

    /// Apply the result of a successful try_commit: update app_hash, pending epoch,
    /// store commit QCs, and flush. Called from both normal and fast-forward commit paths.
    fn apply_commit(&mut self, dc: &DoubleCertificate, context: &str) {
        let store = self.store.read().unwrap();
        match try_commit(
            dc,
            store.as_ref(),
            self.app.as_ref(),
            &mut self.state.last_committed_height,
            &self.state.current_epoch,
        ) {
            Ok(result) => {
                if !result.committed_blocks.is_empty() {
                    self.state.last_app_hash = result.last_app_hash;
                }
                if result.pending_epoch.is_some() {
                    self.pending_epoch = result.pending_epoch;
                }
                drop(store);
                {
                    let mut s = self.store.write().unwrap();
                    for block in &result.committed_blocks {
                        // R-28: only write the commit QC for the block it actually certifies.
                        // Ancestor blocks committed via the chain rule may get their QC stored
                        // by the justify-QC persistence in handle_message (Propose path).
                        // Writing the wrong QC (mismatched block_hash) here would cause sync
                        // verification failures for those ancestor blocks.
                        if result.commit_qc.block_hash == block.hash {
                            s.put_commit_qc(block.height, result.commit_qc.clone());
                        }
                    }
                    s.flush();
                }
            }
            Err(e) => {
                warn!(error = %e, "try_commit failed during {context}");
                drop(store);
            }
        }
    }

    /// Cryptographically validate a DoubleCertificate:
    /// 1. inner and outer QC must reference the same block hash
    /// 2. inner QC aggregate signature (Vote1) must be valid and reach quorum
    /// 3. outer QC aggregate signature (Vote2) must be valid and reach quorum
    ///
    /// Note on quorum and epoch transitions: DCs are always formed in the same epoch as
    /// the block they commit (vote_collector enforces quorum at formation time).  When
    /// a DC is received by a node that has already transitioned to a new epoch, the
    /// validator set may differ.  We enforce quorum against the current validator set
    /// as the best available reference; a legitimate DC from a prior epoch should still
    /// satisfy quorum against the new set unless the set shrank significantly.
    fn validate_double_cert(&self, dc: &DoubleCertificate) -> bool {
        if dc.inner_qc.block_hash != dc.outer_qc.block_hash {
            warn!("double cert inner/outer block_hash mismatch");
            return false;
        }
        let vs = &self.state.validator_set;
        let inner_bytes = Vote::signing_bytes(
            dc.inner_qc.view,
            &dc.inner_qc.block_hash,
            VoteType::Vote,
        );
        if !self.verifier.verify_aggregate(vs, &inner_bytes, &dc.inner_qc.aggregate_signature) {
            warn!("double cert inner QC signature invalid");
            return false;
        }
        if !hotmint_crypto::has_quorum(vs, &dc.inner_qc.aggregate_signature) {
            warn!("double cert inner QC below quorum threshold");
            return false;
        }
        let outer_bytes = Vote::signing_bytes(
            dc.outer_qc.view,
            &dc.outer_qc.block_hash,
            VoteType::Vote2,
        );
        if !self.verifier.verify_aggregate(vs, &outer_bytes, &dc.outer_qc.aggregate_signature) {
            warn!("double cert outer QC signature invalid");
            return false;
        }
        if !hotmint_crypto::has_quorum(vs, &dc.outer_qc.aggregate_signature) {
            warn!("double cert outer QC below quorum threshold");
            return false;
        }
        true
    }

    fn persist_state(&mut self) {
        if let Some(p) = self.persistence.as_mut() {
            p.save_current_view(self.state.current_view);
            if let Some(ref qc) = self.state.locked_qc {
                p.save_locked_qc(qc);
            }
            if let Some(ref qc) = self.state.highest_qc {
                p.save_highest_qc(qc);
            }
            p.save_last_committed_height(self.state.last_committed_height);
            p.save_current_epoch(&self.state.current_epoch);
            p.save_last_app_hash(self.state.last_app_hash);
            p.flush();
        }
    }

    fn advance_view(&mut self, trigger: ViewEntryTrigger) {
        let new_view = match &trigger {
            ViewEntryTrigger::DoubleCert(_) => self.state.current_view.next(),
            ViewEntryTrigger::TimeoutCert(tc) => ViewNumber(tc.view.as_u64() + 1),
            ViewEntryTrigger::Genesis => ViewNumber(1),
        };
        self.advance_view_to(new_view, trigger);
    }

    fn advance_view_to(&mut self, new_view: ViewNumber, trigger: ViewEntryTrigger) {
        if new_view <= self.state.current_view {
            return;
        }

        // Reset backoff on successful progress (DoubleCert path)
        let is_progress = matches!(&trigger, ViewEntryTrigger::DoubleCert(_));

        self.vote_collector.clear_view(self.state.current_view);
        self.vote_collector.prune_before(self.state.current_view);
        self.pacemaker.clear_view(self.state.current_view);
        self.status_senders.clear();
        self.current_view_qc = None;

        // Epoch transition: apply pending validator set change when we reach the
        // epoch's start_view. The start_view is set deterministically (commit_view + 2)
        // so all honest nodes apply the transition at the same view.
        if let Some(ref epoch) = self.pending_epoch
            && new_view >= epoch.start_view
        {
            let new_epoch = self.pending_epoch.take().unwrap();
            info!(
                validator = %self.state.validator_id,
                old_epoch = %self.state.current_epoch.number,
                new_epoch = %new_epoch.number,
                start_view = %new_epoch.start_view,
                validators = new_epoch.validator_set.validator_count(),
                "epoch transition"
            );
            self.state.validator_set = new_epoch.validator_set.clone();
            self.state.current_epoch = new_epoch;
            // Notify network layer of the new validator set
            self.network.on_epoch_change(&self.state.validator_set);
            // Full clear: old votes/wishes are from the previous epoch's validator set
            self.vote_collector = VoteCollector::new();
            self.pacemaker = Pacemaker::with_config(self.pacemaker_config.clone());
        }

        view_protocol::enter_view(
            &mut self.state,
            new_view,
            trigger,
            self.network.as_ref(),
            self.signer.as_ref(),
        );

        if is_progress {
            self.pacemaker.reset_on_progress();
        } else {
            self.pacemaker.reset_timer();
        }

        self.persist_state();

        // If we're the leader, propose immediately.
        // Note: in a full implementation, the leader would collect StatusCerts
        // before proposing (status_senders quorum gate). Currently the immediate
        // propose path is required for liveness across epoch transitions where
        // cross-epoch verification complexity can stall status collection.
        if self.state.is_leader() && self.state.step == ViewStep::WaitingForStatus {
            self.try_propose();
        }
    }
}

// ---------------------------------------------------------------------------
// Regression tests for sub-quorum certificate injection (R-29, R-32)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, RwLock};

    use hotmint_crypto::{Ed25519Signer, Ed25519Verifier};
    use hotmint_types::certificate::QuorumCertificate;
    use hotmint_types::crypto::AggregateSignature;
    use hotmint_types::validator::{ValidatorId, ValidatorInfo};
    use hotmint_types::vote::{Vote, VoteType};
    use hotmint_types::Signer as SignerTrait;
    use tokio::sync::mpsc;

    use crate::application::NoopApplication;
    use crate::network::NetworkSink;
    use crate::state::ConsensusState;
    use crate::store::MemoryBlockStore;

    // Minimal no-op network for unit tests — messages are silently discarded.
    struct DevNullNetwork;
    impl NetworkSink for DevNullNetwork {
        fn broadcast(&self, _: ConsensusMessage) {}
        fn send_to(&self, _: ValidatorId, _: ConsensusMessage) {}
    }

    fn make_validator_set_4() -> (ValidatorSet, Vec<Ed25519Signer>) {
        let signers: Vec<Ed25519Signer> =
            (0..4).map(|i| Ed25519Signer::generate(ValidatorId(i))).collect();
        let infos: Vec<ValidatorInfo> = signers
            .iter()
            .map(|s| ValidatorInfo { id: s.validator_id(), public_key: s.public_key(), power: 1 })
            .collect();
        (ValidatorSet::new(infos), signers)
    }

    fn make_test_engine(
        vid: ValidatorId,
        vs: ValidatorSet,
        signer: Ed25519Signer,
    ) -> (ConsensusEngine, mpsc::Sender<(Option<ValidatorId>, ConsensusMessage)>) {
        let (tx, rx) = mpsc::channel(64);
        let store = Arc::new(RwLock::new(
            Box::new(MemoryBlockStore::new()) as Box<dyn crate::store::BlockStore>
        ));
        let state = ConsensusState::new(vid, vs);
        let engine = ConsensusEngine::new(
            state,
            store,
            Box::new(DevNullNetwork),
            Box::new(NoopApplication),
            Box::new(signer),
            rx,
            EngineConfig {
                verifier: Box::new(Ed25519Verifier),
                pacemaker: None,
                persistence: None,
            },
        );
        (engine, tx)
    }

    // R-29 regression: a Propose message whose justify QC is signed by fewer than
    // 2f+1 validators must be rejected by verify_message().
    #[test]
    fn r29_propose_sub_quorum_justify_rejected_by_verify_message() {
        let (vs, signers) = make_validator_set_4();
        // Use a fresh signer for the engine; verify_message only needs the engine's
        // validator set and verifier, not its own signing key.
        let engine_signer = Ed25519Signer::generate(ValidatorId(0));
        let (engine, _tx) = make_test_engine(ValidatorId(0), vs.clone(), engine_signer);

        // Build a justify QC signed by exactly 1 of 4 validators — below 2f+1 = 3.
        let hash = BlockHash::GENESIS;
        let qc_view = ViewNumber::GENESIS;
        let vote_bytes = Vote::signing_bytes(qc_view, &hash, VoteType::Vote);
        let mut agg = AggregateSignature::new(4);
        agg.add(1, SignerTrait::sign(&signers[1], &vote_bytes)).unwrap();
        let sub_quorum_qc = QuorumCertificate { block_hash: hash, view: qc_view, aggregate_signature: agg };

        // Construct a proposal from V1 carrying this sub-quorum justify.
        let mut block = Block::genesis();
        block.height = Height(1);
        block.view = ViewNumber(1);
        block.proposer = ValidatorId(1);
        block.hash = block.compute_hash();
        let proposal_bytes = crate::view_protocol::proposal_signing_bytes(&block, &sub_quorum_qc);
        let signature = SignerTrait::sign(&signers[1], &proposal_bytes);

        let msg = ConsensusMessage::Propose {
            block: Box::new(block),
            justify: Box::new(sub_quorum_qc),
            double_cert: None,
            signature,
        };

        assert!(
            !engine.verify_message(&msg),
            "R-29 regression: Propose with sub-quorum justify QC must be rejected by verify_message"
        );
    }

    // R-29 regression: a Propose message with a full quorum justify QC (3/4) must pass.
    #[test]
    fn r29_propose_full_quorum_justify_accepted_by_verify_message() {
        let (vs, signers) = make_validator_set_4();
        let engine_signer = Ed25519Signer::generate(ValidatorId(0));
        let (engine, _tx) = make_test_engine(ValidatorId(0), vs.clone(), engine_signer);

        let hash = BlockHash::GENESIS;
        let qc_view = ViewNumber::GENESIS;
        let vote_bytes = Vote::signing_bytes(qc_view, &hash, VoteType::Vote);
        // 3 of 4 signers — meets 2f+1 threshold.
        let mut agg = AggregateSignature::new(4);
        for (i, signer) in signers.iter().take(3).enumerate() {
            agg.add(i, SignerTrait::sign(signer, &vote_bytes)).unwrap();
        }
        let full_quorum_qc = QuorumCertificate { block_hash: hash, view: qc_view, aggregate_signature: agg };

        let mut block = Block::genesis();
        block.height = Height(1);
        block.view = ViewNumber(1);
        block.proposer = ValidatorId(1);
        block.hash = block.compute_hash();
        let proposal_bytes = crate::view_protocol::proposal_signing_bytes(&block, &full_quorum_qc);
        let signature = SignerTrait::sign(&signers[1], &proposal_bytes);

        let msg = ConsensusMessage::Propose {
            block: Box::new(block),
            justify: Box::new(full_quorum_qc),
            double_cert: None,
            signature,
        };

        assert!(
            engine.verify_message(&msg),
            "R-29: Propose with full quorum justify QC must pass verify_message"
        );
    }

    // R-32 regression: a Wish carrying a sub-quorum highest_qc must cause
    // verify_highest_qc_in_wish to treat the QC as invalid and return false,
    // which causes handle_message to discard the Wish without forwarding it
    // to the pacemaker.
    //
    // We verify the sub-component: has_quorum returns false for a 1-of-4 aggregate,
    // ensuring the guard in handle_message fires.
    #[test]
    fn r32_sub_quorum_highest_qc_fails_has_quorum() {
        let (vs, signers) = make_validator_set_4();

        let hash = BlockHash([1u8; 32]);
        let qc_view = ViewNumber(1);
        let vote_bytes = Vote::signing_bytes(qc_view, &hash, VoteType::Vote);

        // Build a QC with only 1 signer — sub-quorum.
        let mut agg = AggregateSignature::new(4);
        agg.add(0, SignerTrait::sign(&signers[0], &vote_bytes)).unwrap();

        assert!(
            !hotmint_crypto::has_quorum(&vs, &agg),
            "R-32 regression: 1-of-4 signed QC must not satisfy has_quorum"
        );

        // Build a QC with 3 signers — full quorum.
        let mut agg_full = AggregateSignature::new(4);
        for (i, signer) in signers.iter().take(3).enumerate() {
            agg_full.add(i, SignerTrait::sign(signer, &vote_bytes)).unwrap();
        }
        assert!(
            hotmint_crypto::has_quorum(&vs, &agg_full),
            "R-32: 3-of-4 signed QC must satisfy has_quorum"
        );
    }
}
