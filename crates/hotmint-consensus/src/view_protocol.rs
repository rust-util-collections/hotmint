use ruc::*;

use crate::application::Application;
use crate::commit::try_commit;
use crate::leader;
use crate::network::NetworkSink;
use crate::state::{ConsensusState, ViewRole, ViewStep};
use crate::store::BlockStore;
use hotmint_crypto::hash::hash_block;
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
                state.step = ViewStep::WaitingForStatus;
                // In genesis, leader can propose immediately (no status to wait for)
                state.step = ViewStep::Proposed; // will be set properly by propose()
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
                let leader_id = state.validator_set.leader_for_view(view).id;
                let msg_bytes = status_signing_bytes(view, &state.locked_qc);
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
                let leader_id = state.validator_set.leader_for_view(view).id;
                let msg_bytes = status_signing_bytes(view, &state.locked_qc);
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
        validator_set: &state.validator_set,
    };

    let payload = app.create_payload(&ctx);

    let mut block = Block {
        height,
        parent_hash,
        view: state.current_view,
        proposer: state.validator_id,
        payload,
        hash: BlockHash::GENESIS, // placeholder
    };
    block.hash = hash_block(&block);

    store.put_block(block.clone());

    let msg_bytes = proposal_signing_bytes(&block, &justify);
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

/// Execute step (3): Replica receives proposal, validates, votes.
/// Returns `Option<Epoch>` if fast-forward commit triggered an epoch change.
#[allow(clippy::too_many_arguments)]
pub fn on_proposal(
    state: &mut ConsensusState,
    block: Block,
    justify: QuorumCertificate,
    double_cert: Option<DoubleCertificate>,
    store: &mut dyn BlockStore,
    network: &dyn NetworkSink,
    app: &dyn Application,
    signer: &dyn Signer,
) -> Result<Option<Epoch>> {
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

    let ctx = BlockContext {
        height: block.height,
        view: block.view,
        proposer: block.proposer,
        epoch: state.current_epoch.number,
        validator_set: &state.validator_set,
    };

    if !app.validate_block(&block, &ctx) {
        return Err(eg!("application rejected block"));
    }

    // Store the block
    store.put_block(block.clone());

    // Update highest QC
    state.update_highest_qc(&justify);

    // Try commit if double cert present (fast-forward)
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
                pending_epoch = result.pending_epoch;
            }
            Err(e) => {
                warn!(error = %e, "try_commit failed during fast-forward in on_proposal");
            }
        }
    }

    // Vote (first phase) → send to current leader
    let vote_bytes = Vote::signing_bytes(state.current_view, &block.hash, VoteType::Vote);
    let signature = signer.sign(&vote_bytes);
    let vote = Vote {
        block_hash: block.hash,
        view: state.current_view,
        validator: state.validator_id,
        signature,
        vote_type: VoteType::Vote,
    };

    let leader_id = state.validator_set.leader_for_view(state.current_view).id;
    info!(
        validator = %state.validator_id,
        view = %state.current_view,
        hash = %block.hash,
        "voting for block"
    );
    network.send_to(leader_id, ConsensusMessage::VoteMsg(vote));

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

    let msg_bytes = prepare_signing_bytes(&qc);
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

    // Vote2 → send to next leader
    let vote_bytes = Vote::signing_bytes(state.current_view, &qc.block_hash, VoteType::Vote2);
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

    state.step = ViewStep::SentVote2;
}

// --- Signing helpers ---

pub(crate) fn status_signing_bytes(
    view: ViewNumber,
    locked_qc: &Option<QuorumCertificate>,
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(b'S');
    buf.extend_from_slice(&view.as_u64().to_le_bytes());
    if let Some(qc) = locked_qc {
        buf.extend_from_slice(&qc.block_hash.0);
        buf.extend_from_slice(&qc.view.as_u64().to_le_bytes());
    }
    buf
}

pub(crate) fn proposal_signing_bytes(block: &Block, justify: &QuorumCertificate) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(b'P');
    buf.extend_from_slice(&block.hash.0);
    buf.extend_from_slice(&justify.block_hash.0);
    buf.extend_from_slice(&justify.view.as_u64().to_le_bytes());
    buf
}

pub(crate) fn prepare_signing_bytes(qc: &QuorumCertificate) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(b'Q');
    buf.extend_from_slice(&qc.block_hash.0);
    buf.extend_from_slice(&qc.view.as_u64().to_le_bytes());
    buf
}
