use ruc::*;

use crate::application::Application;
use crate::commit::try_commit;
use crate::leader;
use crate::network::NetworkSink;
use crate::state::{ConsensusState, ViewRole, ViewStep};
use crate::store::BlockStore;
use hotmint_crypto::hash::compute_block_hash;
use hotmint_types::context::BlockContext;
use hotmint_types::epoch::Epoch;
use hotmint_types::vote::VoteType;
use hotmint_types::*;
use tracing::{debug, info, warn};

/// Trigger that causes us to enter a new view
pub enum ViewEntryTrigger {
    DoubleCert(DoubleCertificate),
    TimeoutCert(TimeoutCertificate),
    Genesis,
}

/// Execute step (1): Enter view
pub fn enter_view(
    state: &mut ConsensusState,
    view: ViewNumber,
    trigger: ViewEntryTrigger,
    network: &dyn NetworkSink,
    signer: &dyn Signer,
) {
    state.current_view = view;
    state.step = ViewStep::Entered;

    let am_leader = leader::is_leader(&state.validator_set, view, state.validator_id);
    state.role = if am_leader {
        ViewRole::Leader
    } else {
        ViewRole::Replica
    };

    info!(
        validator = %state.validator_id,
        view = %view,
        role = ?state.role,
        epoch = %state.current_epoch.number,
        "entering view"
    );

    match trigger {
        ViewEntryTrigger::Genesis => {
            if am_leader {
                // Genesis leader enters WaitingForStatus; engine calls try_propose() directly
                state.step = ViewStep::WaitingForStatus;
            } else {
                state.step = ViewStep::WaitingForProposal;
            }
        }
        ViewEntryTrigger::DoubleCert(dc) => {
            state.update_highest_qc(&dc.outer_qc);
            state.highest_double_cert = Some(dc);
            if am_leader {
                state.step = ViewStep::WaitingForStatus;
            } else {
                // Send status to new leader
                let leader_id = state
                    .validator_set
                    .leader_for_view(view)
                    .expect("empty validator set")
                    .id;
                let msg_bytes = status_signing_bytes(&state.chain_id_hash, view, &state.locked_qc);
                let sig = signer.sign(&msg_bytes);
                network.send_to(
                    leader_id,
                    ConsensusMessage::StatusCert {
                        locked_qc: state.locked_qc.clone(),
                        validator: state.validator_id,
                        signature: sig,
                    },
                );
                state.step = ViewStep::WaitingForProposal;
            }
        }
        ViewEntryTrigger::TimeoutCert(tc) => {
            if let Some(hqc) = tc.highest_qc() {
                state.update_highest_qc(hqc);
            }
            if am_leader {
                state.step = ViewStep::WaitingForStatus;
            } else {
                let leader_id = state
                    .validator_set
                    .leader_for_view(view)
                    .expect("empty validator set")
                    .id;
                let msg_bytes = status_signing_bytes(&state.chain_id_hash, view, &state.locked_qc);
                let sig = signer.sign(&msg_bytes);
                network.send_to(
                    leader_id,
                    ConsensusMessage::StatusCert {
                        locked_qc: state.locked_qc.clone(),
                        validator: state.validator_id,
                        signature: sig,
                    },
                );
                state.step = ViewStep::WaitingForProposal;
            }
        }
    }
}

/// Execute step (2): Leader proposes
pub fn propose(
    state: &mut ConsensusState,
    store: &mut dyn BlockStore,
    network: &dyn NetworkSink,
    app: &dyn Application,
    signer: &dyn Signer,
) -> Result<Block> {
    let justify = state
        .highest_qc
        .clone()
        .c(d!("no QC to justify proposal"))?;

    let parent_hash = justify.block_hash;
    let parent = store
        .get_block(&parent_hash)
        .c(d!("parent block not found"))?;
    let height = parent.height.next();

    let ctx = BlockContext {
        height,
        view: state.current_view,
        proposer: state.validator_id,
        epoch: state.current_epoch.number,
        epoch_start_view: state.current_epoch.start_view,
        validator_set: &state.validator_set,
    };

    let payload = app.create_payload(&ctx);

    let mut block = Block {
        height,
        parent_hash,
        view: state.current_view,
        proposer: state.validator_id,
        payload,
        app_hash: state.last_app_hash,
        hash: BlockHash::GENESIS, // placeholder
    };
    block.hash = compute_block_hash(&block);

    store.put_block(block.clone());

    let msg_bytes = proposal_signing_bytes(&state.chain_id_hash, &block, &justify);
    let signature = signer.sign(&msg_bytes);

    info!(
        validator = %state.validator_id,
        view = %state.current_view,
        height = height.as_u64(),
        hash = %block.hash,
        "proposing block"
    );

    network.broadcast(ConsensusMessage::Propose {
        block: Box::new(block.clone()),
        justify: Box::new(justify),
        double_cert: state.highest_double_cert.clone().map(Box::new),
        signature,
    });

    state.step = ViewStep::CollectingVotes;
    Ok(block)
}

/// Incoming proposal data from the leader.
pub struct ProposalData {
    pub block: Block,
    pub justify: QuorumCertificate,
    pub double_cert: Option<DoubleCertificate>,
}

/// Execute step (3): Replica receives proposal, validates, votes.
/// Returns `Option<Epoch>` if fast-forward commit triggered an epoch change.
pub fn on_proposal(
    state: &mut ConsensusState,
    proposal: ProposalData,
    store: &mut dyn BlockStore,
    network: &dyn NetworkSink,
    app: &dyn Application,
    signer: &dyn Signer,
) -> Result<Option<Epoch>> {
    let ProposalData {
        block,
        justify,
        double_cert,
    } = proposal;
    if state.step != ViewStep::WaitingForProposal {
        debug!(
            validator = %state.validator_id,
            step = ?state.step,
            "ignoring proposal, not waiting"
        );
        return Ok(None);
    }

    // Safety check: justify.rank() >= locked_qc.rank()
    if let Some(ref locked) = state.locked_qc
        && justify.rank() < locked.rank()
    {
        warn!(
            validator = %state.validator_id,
            justify_view = %justify.view,
            locked_view = %locked.view,
            "rejecting proposal: justify rank < locked rank"
        );
        return Err(eg!("proposal justify rank below locked QC rank"));
    }

    // Verify proposer is the rightful leader for this view
    let expected_leader = state
        .validator_set
        .leader_for_view(block.view)
        .ok_or_else(|| eg!("empty validator set"))?
        .id;
    if block.proposer != expected_leader {
        return Err(eg!(
            "block proposer {} is not leader {} for view {}",
            block.proposer,
            expected_leader,
            block.view
        ));
    }

    // Verify block hash integrity
    let expected_hash = hotmint_crypto::compute_block_hash(&block);
    if block.hash != expected_hash {
        return Err(eg!(
            "block hash mismatch: declared {} != computed {}",
            block.hash,
            expected_hash
        ));
    }

    let ctx = BlockContext {
        height: block.height,
        view: block.view,
        proposer: block.proposer,
        epoch: state.current_epoch.number,
        epoch_start_view: state.current_epoch.start_view,
        validator_set: &state.validator_set,
    };

    if !app.validate_block(&block, &ctx) {
        return Err(eg!("application rejected block"));
    }

    // Store the block
    store.put_block(block.clone());

    // Update highest QC
    state.update_highest_qc(&justify);

    // Fast-forward commit via double cert (if present).
    // IMPORTANT: this MUST happen BEFORE the app_hash check below.
    // block.app_hash = state-after-parent, and the DC in this proposal commits
    // the parent block, producing that exact state root.  Processing the DC
    // first keeps state.last_app_hash in sync so the check below is consistent
    // for both the leader (who committed the parent independently) and replicas
    // (who commit the parent only via this DC).
    let mut pending_epoch = None;
    if let Some(ref dc) = double_cert {
        match try_commit(
            dc,
            store,
            app,
            &mut state.last_committed_height,
            &state.current_epoch,
        ) {
            Ok(result) => {
                if !result.committed_blocks.is_empty() {
                    state.last_app_hash = result.last_app_hash;
                }
                pending_epoch = result.pending_epoch;
            }
            Err(e) => {
                return Err(eg!("try_commit failed during fast-forward: {}", e));
            }
        }
    }

    // Verify app_hash matches local state AFTER fast-forward commit.
    // Skip when the application does not track state roots (e.g. fullnode
    // running NoopApplication against a chain produced by a real ABCI app).
    if app.tracks_app_hash() && block.app_hash != state.last_app_hash {
        return Err(eg!(
            "app_hash mismatch: block {} != local {}",
            block.app_hash,
            state.last_app_hash
        ));
    }

    // Vote (first phase) → send to current leader (only if we have voting power)
    if state.validator_set.power_of(state.validator_id) > 0 {
        let vote_bytes = Vote::signing_bytes(
            &state.chain_id_hash,
            state.current_view,
            &block.hash,
            VoteType::Vote,
        );
        let signature = signer.sign(&vote_bytes);
        let vote = Vote {
            block_hash: block.hash,
            view: state.current_view,
            validator: state.validator_id,
            signature,
            vote_type: VoteType::Vote,
        };

        let leader_id = state
            .validator_set
            .leader_for_view(state.current_view)
            .expect("empty validator set")
            .id;
        info!(
            validator = %state.validator_id,
            view = %state.current_view,
            hash = %block.hash,
            "voting for block"
        );
        network.send_to(leader_id, ConsensusMessage::VoteMsg(vote));
    }

    state.step = ViewStep::Voted;
    Ok(pending_epoch)
}

/// Execute step (4): Leader collected 2f+1 votes → form QC → broadcast prepare
pub fn on_votes_collected(
    state: &mut ConsensusState,
    qc: QuorumCertificate,
    network: &dyn NetworkSink,
    signer: &dyn Signer,
) {
    info!(
        validator = %state.validator_id,
        view = %state.current_view,
        hash = %qc.block_hash,
        "QC formed, broadcasting prepare"
    );

    state.update_highest_qc(&qc);

    let msg_bytes = prepare_signing_bytes(&state.chain_id_hash, &qc);
    let signature = signer.sign(&msg_bytes);

    network.broadcast(ConsensusMessage::Prepare {
        certificate: qc,
        signature,
    });

    state.step = ViewStep::Prepared;
}

/// Execute step (5): Replica receives prepare → update lock → send vote2 to next leader
pub fn on_prepare(
    state: &mut ConsensusState,
    qc: QuorumCertificate,
    network: &dyn NetworkSink,
    signer: &dyn Signer,
) {
    // Update lock to this QC
    state.update_locked_qc(&qc);
    state.update_highest_qc(&qc);

    // Vote2 → send to next leader (only if we have voting power)
    if state.validator_set.power_of(state.validator_id) > 0 {
        let vote_bytes = Vote::signing_bytes(
            &state.chain_id_hash,
            state.current_view,
            &qc.block_hash,
            VoteType::Vote2,
        );
        let signature = signer.sign(&vote_bytes);
        let vote = Vote {
            block_hash: qc.block_hash,
            view: state.current_view,
            validator: state.validator_id,
            signature,
            vote_type: VoteType::Vote2,
        };

        let next_leader_id = leader::next_leader(&state.validator_set, state.current_view);
        info!(
            validator = %state.validator_id,
            view = %state.current_view,
            hash = %qc.block_hash,
            "sending vote2 to next leader {}",
            next_leader_id
        );
        network.send_to(next_leader_id, ConsensusMessage::Vote2Msg(vote));
    }

    state.step = ViewStep::SentVote2;
}

// --- Signing helpers ---

pub(crate) fn status_signing_bytes(
    chain_id_hash: &[u8; 32],
    view: ViewNumber,
    locked_qc: &Option<QuorumCertificate>,
) -> Vec<u8> {
    let tag = b"HOTMINT_STATUS_V1\0";
    let mut buf = Vec::with_capacity(tag.len() + 32 + 8 + 40);
    buf.extend_from_slice(tag);
    buf.extend_from_slice(chain_id_hash);
    buf.extend_from_slice(&view.as_u64().to_le_bytes());
    if let Some(qc) = locked_qc {
        buf.extend_from_slice(&qc.block_hash.0);
        buf.extend_from_slice(&qc.view.as_u64().to_le_bytes());
    }
    buf
}

pub(crate) fn proposal_signing_bytes(
    chain_id_hash: &[u8; 32],
    block: &Block,
    justify: &QuorumCertificate,
) -> Vec<u8> {
    let tag = b"HOTMINT_PROPOSAL_V1\0";
    let mut buf = Vec::with_capacity(tag.len() + 32 + 32 + 32 + 8);
    buf.extend_from_slice(tag);
    buf.extend_from_slice(chain_id_hash);
    buf.extend_from_slice(&block.hash.0);
    buf.extend_from_slice(&justify.block_hash.0);
    buf.extend_from_slice(&justify.view.as_u64().to_le_bytes());
    buf
}

pub(crate) fn prepare_signing_bytes(chain_id_hash: &[u8; 32], qc: &QuorumCertificate) -> Vec<u8> {
    let tag = b"HOTMINT_PREPARE_V1\0";
    let mut buf = Vec::with_capacity(tag.len() + 32 + 32 + 8);
    buf.extend_from_slice(tag);
    buf.extend_from_slice(chain_id_hash);
    buf.extend_from_slice(&qc.block_hash.0);
    buf.extend_from_slice(&qc.view.as_u64().to_le_bytes());
    buf
}
