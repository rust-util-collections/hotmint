//! Block sync: allows a node that is behind to catch up by requesting
//! missing blocks from peers and replaying the commit lifecycle.

use ruc::*;

use crate::application::Application;
use crate::commit;
use crate::store::BlockStore;
use hotmint_types::context::BlockContext;
use hotmint_types::epoch::Epoch;
use hotmint_types::sync::{MAX_SYNC_BATCH, SyncRequest, SyncResponse};
use hotmint_types::{Block, BlockHash, Height, ViewNumber};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tracing::info;

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

/// Run block sync: request missing blocks from peers and replay them.
///
/// This should be called **before** the consensus engine starts.
/// Returns the updated (height, epoch) after syncing.
pub async fn sync_to_tip(
    store: &mut dyn BlockStore,
    app: &dyn Application,
    current_epoch: &mut Epoch,
    last_committed_height: &mut Height,
    last_app_hash: &mut BlockHash,
    request_tx: &mpsc::Sender<SyncRequest>,
    response_rx: &mut mpsc::Receiver<SyncResponse>,
) -> Result<()> {
    // First, get status from peer
    request_tx
        .send(SyncRequest::GetStatus)
        .await
        .map_err(|_| eg!("sync channel closed"))?;

    let peer_status = match timeout(SYNC_TIMEOUT, response_rx.recv()).await {
        Ok(Some(SyncResponse::Status {
            last_committed_height: peer_height,
            ..
        })) => peer_height,
        Ok(Some(SyncResponse::Error(e))) => return Err(eg!("peer error: {}", e)),
        Ok(Some(SyncResponse::Blocks(_))) => return Err(eg!("unexpected blocks response")),
        Ok(None) => return Err(eg!("sync channel closed")),
        Err(_) => {
            info!("sync status request timed out, starting from current state");
            return Ok(());
        }
    };

    if peer_status <= *last_committed_height {
        info!(
            our_height = last_committed_height.as_u64(),
            peer_height = peer_status.as_u64(),
            "already caught up"
        );
        return Ok(());
    }

    info!(
        our_height = last_committed_height.as_u64(),
        peer_height = peer_status.as_u64(),
        "starting block sync"
    );

    // Batch sync loop
    loop {
        let from = Height(last_committed_height.as_u64() + 1);
        let to = Height(std::cmp::min(
            from.as_u64() + MAX_SYNC_BATCH - 1,
            peer_status.as_u64(),
        ));

        request_tx
            .send(SyncRequest::GetBlocks {
                from_height: from,
                to_height: to,
            })
            .await
            .map_err(|_| eg!("sync channel closed"))?;

        let blocks = match timeout(SYNC_TIMEOUT, response_rx.recv()).await {
            Ok(Some(SyncResponse::Blocks(blocks))) => blocks,
            Ok(Some(SyncResponse::Error(e))) => return Err(eg!("peer error: {}", e)),
            Ok(Some(SyncResponse::Status { .. })) => return Err(eg!("unexpected status response")),
            Ok(None) => return Err(eg!("sync channel closed")),
            Err(_) => return Err(eg!("sync request timed out")),
        };

        if blocks.is_empty() {
            break;
        }

        // Validate chain continuity and replay
        replay_blocks(
            &blocks,
            store,
            app,
            current_epoch,
            last_committed_height,
            last_app_hash,
        )?;

        info!(
            synced_to = last_committed_height.as_u64(),
            target = peer_status.as_u64(),
            "sync progress"
        );

        if *last_committed_height >= peer_status {
            break;
        }
    }

    info!(
        height = last_committed_height.as_u64(),
        epoch = %current_epoch.number,
        "block sync complete"
    );
    Ok(())
}

/// Replay a batch of blocks: store them and run the application lifecycle.
/// Validates chain continuity (parent_hash linkage).
pub fn replay_blocks(
    blocks: &[(Block, Option<hotmint_types::QuorumCertificate>)],
    store: &mut dyn BlockStore,
    app: &dyn Application,
    current_epoch: &mut Epoch,
    last_committed_height: &mut Height,
    last_app_hash: &mut BlockHash,
) -> Result<()> {
    for (i, (block, qc)) in blocks.iter().enumerate() {
        // Validate chain continuity
        if i > 0 && block.parent_hash != blocks[i - 1].0.hash {
            return Err(eg!(
                "chain discontinuity at height {}: expected parent {}, got {}",
                block.height.as_u64(),
                blocks[i - 1].0.hash,
                block.parent_hash
            ));
        }

        // Verify commit QC if present (non-genesis blocks should have one)
        if let Some(cert) = qc {
            if cert.block_hash != block.hash {
                return Err(eg!(
                    "sync QC block_hash mismatch at height {}: QC={} block={}",
                    block.height.as_u64(),
                    cert.block_hash,
                    block.hash
                ));
            }
            // Verify QC aggregate signature
            let verifier = hotmint_crypto::Ed25519Verifier;
            let qc_bytes = hotmint_types::vote::Vote::signing_bytes(
                cert.view,
                &cert.block_hash,
                hotmint_types::vote::VoteType::Vote,
            );
            if !hotmint_types::Verifier::verify_aggregate(
                &verifier,
                &current_epoch.validator_set,
                &qc_bytes,
                &cert.aggregate_signature,
            ) {
                return Err(eg!(
                    "sync QC signature verification failed at height {}",
                    block.height.as_u64()
                ));
            }
        } else if block.height.as_u64() > 1 {
            // Non-genesis blocks MUST have a commit QC — without one, the block
            // has not been certified by a 2/3 quorum and must be rejected.
            return Err(eg!(
                "sync block at height {} missing commit QC — refusing unverified block",
                block.height.as_u64()
            ));
        }

        // Skip already-committed blocks
        if block.height <= *last_committed_height {
            continue;
        }

        // Verify block hash integrity (includes app_hash in computation)
        let expected_hash = hotmint_crypto::compute_block_hash(block);
        if block.hash != expected_hash {
            return Err(eg!(
                "sync block hash mismatch at height {}: declared {} != computed {}",
                block.height.as_u64(),
                block.hash,
                expected_hash
            ));
        }

        // Verify app_hash matches local application state.
        // Skip when the application does not track state roots (e.g. NoopApplication),
        // so that fullnodes without an ABCI backend can sync from peers running real
        // applications that produce non-zero app_hash values.
        if app.tracks_app_hash() && block.app_hash != *last_app_hash {
            return Err(eg!(
                "sync block app_hash mismatch at height {}: block {} != local {}",
                block.height.as_u64(),
                block.app_hash,
                last_app_hash
            ));
        }

        // Store the block
        store.put_block(block.clone());

        // Run application lifecycle
        let ctx = BlockContext {
            height: block.height,
            view: block.view,
            proposer: block.proposer,
            epoch: current_epoch.number,
            epoch_start_view: current_epoch.start_view,
            validator_set: &current_epoch.validator_set,
        };

        if !app.validate_block(block, &ctx) {
            return Err(eg!(
                "app rejected synced block at height {}",
                block.height.as_u64()
            ));
        }

        let txs = commit::decode_payload(&block.payload);
        let response = app
            .execute_block(&txs, &ctx)
            .c(d!("execute_block failed during sync"))?;

        app.on_commit(block, &ctx)
            .c(d!("on_commit failed during sync"))?;

        *last_app_hash = if app.tracks_app_hash() {
            response.app_hash
        } else {
            // App does not compute state roots: carry the chain's authoritative
            // value forward so that the continuity check stays coherent.
            block.app_hash
        };

        // Handle epoch transitions
        if !response.validator_updates.is_empty() {
            let new_vs = current_epoch
                .validator_set
                .apply_updates(&response.validator_updates);
            // Epoch starts 2 views after the committing block (same as commit.rs)
            let epoch_start = ViewNumber(block.view.as_u64() + 2);
            *current_epoch = Epoch::new(current_epoch.number.next(), epoch_start, new_vs);
        }

        *last_committed_height = block.height;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::NoopApplication;
    use crate::store::MemoryBlockStore;
    use hotmint_types::{BlockHash, QuorumCertificate, ValidatorId, ViewNumber};

    fn make_qc(block: &Block, signer: &hotmint_crypto::Ed25519Signer) -> QuorumCertificate {
        let vote_bytes = hotmint_types::vote::Vote::signing_bytes(
            block.view,
            &block.hash,
            hotmint_types::vote::VoteType::Vote,
        );
        let sig = hotmint_types::Signer::sign(signer, &vote_bytes);
        let mut agg = hotmint_types::AggregateSignature::new(1);
        agg.add(0, sig).unwrap();
        QuorumCertificate {
            block_hash: block.hash,
            view: block.view,
            aggregate_signature: agg,
        }
    }

    fn make_block(height: u64, parent: BlockHash) -> Block {
        let mut block = Block {
            height: Height(height),
            parent_hash: parent,
            view: ViewNumber(height),
            proposer: ValidatorId(0),
            payload: vec![],
            app_hash: BlockHash::GENESIS,
            hash: BlockHash::GENESIS, // placeholder
        };
        block.hash = hotmint_crypto::compute_block_hash(&block);
        block
    }

    #[test]
    fn test_replay_blocks_valid_chain() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let signer = hotmint_crypto::Ed25519Signer::generate(ValidatorId(0));
        let vs = hotmint_types::ValidatorSet::new(vec![hotmint_types::ValidatorInfo {
            id: ValidatorId(0),
            public_key: hotmint_types::Signer::public_key(&signer),
            power: 1,
        }]);
        let mut epoch = Epoch::genesis(vs);
        let mut height = Height::GENESIS;

        let b1 = make_block(1, BlockHash::GENESIS);
        let b2 = make_block(2, b1.hash);
        let b3 = make_block(3, b2.hash);

        let qc1 = make_qc(&b1, &signer);
        let qc2 = make_qc(&b2, &signer);
        let qc3 = make_qc(&b3, &signer);

        let blocks: Vec<_> = vec![(b1, Some(qc1)), (b2, Some(qc2)), (b3, Some(qc3))];
        let mut app_hash = BlockHash::GENESIS;
        replay_blocks(
            &blocks,
            &mut store,
            &app,
            &mut epoch,
            &mut height,
            &mut app_hash,
        )
        .unwrap();
        assert_eq!(height, Height(3));
        assert!(store.get_block_by_height(Height(1)).is_some());
        assert!(store.get_block_by_height(Height(3)).is_some());
    }

    #[test]
    fn test_replay_blocks_rejects_missing_qc() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let signer = hotmint_crypto::Ed25519Signer::generate(ValidatorId(0));
        let vs = hotmint_types::ValidatorSet::new(vec![hotmint_types::ValidatorInfo {
            id: ValidatorId(0),
            public_key: hotmint_types::Signer::public_key(&signer),
            power: 1,
        }]);
        let mut epoch = Epoch::genesis(vs);
        let mut height = Height::GENESIS;

        let b1 = make_block(1, BlockHash::GENESIS);
        let qc1 = make_qc(&b1, &signer);
        let b2 = make_block(2, b1.hash);
        // Non-genesis block without QC should be rejected
        let blocks: Vec<_> = vec![(b1, Some(qc1)), (b2, None)];
        let mut app_hash = BlockHash::GENESIS;
        assert!(
            replay_blocks(
                &blocks,
                &mut store,
                &app,
                &mut epoch,
                &mut height,
                &mut app_hash
            )
            .is_err()
        );
    }

    #[test]
    fn test_replay_blocks_broken_chain() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let signer = hotmint_crypto::Ed25519Signer::generate(ValidatorId(0));
        let vs = hotmint_types::ValidatorSet::new(vec![hotmint_types::ValidatorInfo {
            id: ValidatorId(0),
            public_key: hotmint_types::Signer::public_key(&signer),
            power: 1,
        }]);
        let mut epoch = Epoch::genesis(vs);
        let mut height = Height::GENESIS;

        let b1 = make_block(1, BlockHash::GENESIS);
        let b3 = make_block(3, BlockHash([99u8; 32])); // wrong parent

        let qc1 = make_qc(&b1, &signer);
        let qc3 = make_qc(&b3, &signer);
        let blocks: Vec<_> = vec![(b1, Some(qc1)), (b3, Some(qc3))];
        let mut app_hash = BlockHash::GENESIS;
        assert!(
            replay_blocks(
                &blocks,
                &mut store,
                &app,
                &mut epoch,
                &mut height,
                &mut app_hash
            )
            .is_err()
        );
    }
}
