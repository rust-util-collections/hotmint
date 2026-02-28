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

    /// Bootstrap: enter genesis view and start the event loop
    pub async fn run(mut self) {
        self.enter_genesis_view();

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
    fn verify_message(&self, msg: &ConsensusMessage) -> bool {
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
                // Verify the TC's aggregate signature over wish signing bytes
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
                        // Fast-forward via double cert
                        let store = self.store.read().unwrap();
                        match try_commit(
                            dc,
                            store.as_ref(),
                            self.app.as_ref(),
                            &mut self.state.last_committed_height,
                            &self.state.current_epoch,
                        ) {
                            Ok(result) => {
                                if result.pending_epoch.is_some() {
                                    self.pending_epoch = result.pending_epoch;
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "try_commit failed during fast-forward");
                            }
                        }
                        drop(store);
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
                        let mut store = self.store.write().unwrap();
                        store.put_block(block);
                    }
                    return Ok(());
                }

                let mut store = self.store.write().unwrap();
                let maybe_pending = view_protocol::on_proposal(
                    &mut self.state,
                    block,
                    justify,
                    double_cert,
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
                    let needed = self.state.validator_set.quorum_threshold() as usize - 1;
                    if self.status_senders.len() >= needed {
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
        {
            let store = self.store.read().unwrap();
            match try_commit(
                &dc,
                store.as_ref(),
                self.app.as_ref(),
                &mut self.state.last_committed_height,
                &self.state.current_epoch,
            ) {
                Ok(result) => {
                    if result.pending_epoch.is_some() {
                        self.pending_epoch = result.pending_epoch;
                    }
                }
                Err(e) => {
                    warn!(error = %e, "try_commit failed in double cert handler");
                }
            }
        }

        self.state.highest_double_cert = Some(dc.clone());

        // Advance to next view — as new leader, include DC in proposal
        self.advance_view(ViewEntryTrigger::DoubleCert(dc));
    }

    fn handle_timeout(&mut self) {
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

        // If we're the leader, we may need to propose
        if self.state.is_leader() && self.state.step == ViewStep::WaitingForStatus {
            // In simplified version, leader proposes immediately
            self.try_propose();
        }
    }
}
