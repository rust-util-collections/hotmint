use ruc::*;

use std::collections::HashSet;
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
    msg_rx: mpsc::Receiver<(ValidatorId, ConsensusMessage)>,
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
    msg_rx: Option<mpsc::Receiver<(ValidatorId, ConsensusMessage)>>,
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

    pub fn messages(mut self, msg_rx: mpsc::Receiver<(ValidatorId, ConsensusMessage)>) -> Self {
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
        msg_rx: mpsc::Receiver<(ValidatorId, ConsensusMessage)>,
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
                // Also verify the QC's aggregate signature
                let qc_bytes =
                    Vote::signing_bytes(certificate.view, &certificate.block_hash, VoteType::Vote);
                if !self
                    .verifier
                    .verify_aggregate(vs, &qc_bytes, &certificate.aggregate_signature)
                {
                    warn!(view = %certificate.view, "invalid QC aggregate signature");
                    return false;
                }
                true
            }
            ConsensusMessage::Wish {
                target_view,
                validator,
                signature,
                ..
            } => {
                let Some(vi) = vs.get(*validator) else {
                    warn!(validator = %validator, "wish from unknown validator");
                    return false;
                };
                let bytes = crate::pacemaker::wish_signing_bytes(*target_view);
                if !self.verifier.verify(&vi.public_key, &bytes, signature) {
                    warn!(validator = %validator, "invalid wish signature");
                    return false;
                }
                true
            }
            ConsensusMessage::TimeoutCert(tc) => {
                // TC aggregate signature: individual wishes bind highest_qc,
                // but aggregate verification can't split per-validator.
                // Verify with None (base signing bytes) — individual wish
                // verification at add_wish provides the full binding.
                let bytes = crate::pacemaker::wish_signing_bytes(ViewNumber(tc.view.as_u64() + 1));
                if !self
                    .verifier
                    .verify_aggregate(vs, &bytes, &tc.aggregate_signature)
                {
                    warn!(view = %tc.view, "invalid TC aggregate signature");
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

    fn handle_message(&mut self, _sender: ValidatorId, msg: ConsensusMessage) -> Result<()> {
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
                        // Validate DoubleCert comprehensively:
                        // 1. Inner and outer QC must reference same block
                        if dc.inner_qc.block_hash != dc.outer_qc.block_hash {
                            warn!("double cert inner/outer block_hash mismatch");
                            return Ok(());
                        }
                        // 2. Both QCs must have quorum-level signer count
                        let quorum = self.state.validator_set.quorum_threshold() as usize;
                        if dc.inner_qc.aggregate_signature.count() < quorum {
                            warn!("double cert inner QC insufficient signers");
                            return Ok(());
                        }
                        if dc.outer_qc.aggregate_signature.count() < quorum {
                            warn!("double cert outer QC insufficient signers");
                            return Ok(());
                        }
                        // 3. Verify inner QC aggregate signature (Vote1)
                        let inner_bytes = Vote::signing_bytes(
                            dc.inner_qc.view,
                            &dc.inner_qc.block_hash,
                            VoteType::Vote,
                        );
                        if !self.verifier.verify_aggregate(
                            &self.state.validator_set,
                            &inner_bytes,
                            &dc.inner_qc.aggregate_signature,
                        ) {
                            warn!("double cert inner QC signature invalid");
                            return Ok(());
                        }
                        // 4. Verify outer QC aggregate signature (Vote2)
                        let outer_bytes = Vote::signing_bytes(
                            dc.outer_qc.view,
                            &dc.outer_qc.block_hash,
                            VoteType::Vote2,
                        );
                        if !self.verifier.verify_aggregate(
                            &self.state.validator_set,
                            &outer_bytes,
                            &dc.outer_qc.aggregate_signature,
                        ) {
                            warn!("double cert outer QC signature invalid");
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
                // Validate carried highest_qc (C4 mitigation)
                if let Some(ref qc) = highest_qc
                    && qc.aggregate_signature.count() > 0
                {
                    let qc_bytes = Vote::signing_bytes(qc.view, &qc.block_hash, VoteType::Vote);
                    if !self.verifier.verify_aggregate(
                        &self.state.validator_set,
                        &qc_bytes,
                        &qc.aggregate_signature,
                    ) {
                        warn!(validator = %validator, "wish carries invalid highest_qc");
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
                        s.put_commit_qc(block.height, result.commit_qc.clone());
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
