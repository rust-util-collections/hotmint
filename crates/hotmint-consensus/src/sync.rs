//! Block sync: allows a node that is behind to catch up by requesting
//! missing blocks from peers and replaying the commit lifecycle.

use ruc::*;

use crate::application::Application;
use crate::commit;
use crate::store::BlockStore;
use hotmint_types::context::BlockContext;
use hotmint_types::epoch::Epoch;
use hotmint_types::sync::{MAX_SYNC_BATCH, SyncRequest, SyncResponse};
use hotmint_types::{Block, Height};
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
    request_tx: &mpsc::UnboundedSender<SyncRequest>,
    response_rx: &mut mpsc::UnboundedReceiver<SyncResponse>,
) -> Result<()> {
    // First, get status from peer
    request_tx
        .send(SyncRequest::GetStatus)
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
        replay_blocks(&blocks, store, app, current_epoch, last_committed_height)?;

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
    blocks: &[Block],
    store: &mut dyn BlockStore,
    app: &dyn Application,
    current_epoch: &mut Epoch,
    last_committed_height: &mut Height,
) -> Result<()> {
    for (i, block) in blocks.iter().enumerate() {
        // Validate chain continuity
        if i > 0 && block.parent_hash != blocks[i - 1].hash {
            return Err(eg!(
                "chain discontinuity at height {}: expected parent {}, got {}",
                block.height.as_u64(),
                blocks[i - 1].hash,
                block.parent_hash
            ));
        }

        // Skip already-committed blocks
        if block.height <= *last_committed_height {
            continue;
        }

        // Store the block
        store.put_block(block.clone());

        // Run application lifecycle
        let ctx = BlockContext {
            height: block.height,
            view: block.view,
            proposer: block.proposer,
            epoch: current_epoch.number,
            validator_set: &current_epoch.validator_set,
        };

        let txs = commit::decode_payload(&block.payload);
        let response = app
            .execute_block(&txs, &ctx)
            .c(d!("execute_block failed during sync"))?;

        app.on_commit(block, &ctx)
            .c(d!("on_commit failed during sync"))?;

        // Handle epoch transitions
        if !response.validator_updates.is_empty() {
            let new_vs = current_epoch
                .validator_set
                .apply_updates(&response.validator_updates);
            *current_epoch = Epoch::new(current_epoch.number.next(), block.view, new_vs);
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
    use hotmint_types::{BlockHash, ValidatorId, ViewNumber};

    fn make_block(height: u64, parent: BlockHash) -> Block {
        let hash = BlockHash([height as u8; 32]);
        Block {
            height: Height(height),
            parent_hash: parent,
            view: ViewNumber(height),
            proposer: ValidatorId(0),
            payload: vec![],
            hash,
        }
    }

    #[test]
    fn test_replay_blocks_valid_chain() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let vs = hotmint_types::ValidatorSet::new(vec![hotmint_types::ValidatorInfo {
            id: ValidatorId(0),
            public_key: hotmint_types::PublicKey(vec![0]),
            power: 1,
        }]);
        let mut epoch = Epoch::genesis(vs);
        let mut height = Height::GENESIS;

        let b1 = make_block(1, BlockHash::GENESIS);
        let b2 = make_block(2, b1.hash);
        let b3 = make_block(3, b2.hash);

        replay_blocks(&[b1, b2, b3], &mut store, &app, &mut epoch, &mut height).unwrap();
        assert_eq!(height, Height(3));
        assert!(store.get_block_by_height(Height(1)).is_some());
        assert!(store.get_block_by_height(Height(3)).is_some());
    }

    #[test]
    fn test_replay_blocks_broken_chain() {
        let mut store = MemoryBlockStore::new();
        let app = NoopApplication;
        let vs = hotmint_types::ValidatorSet::new(vec![hotmint_types::ValidatorInfo {
            id: ValidatorId(0),
            public_key: hotmint_types::PublicKey(vec![0]),
            power: 1,
        }]);
        let mut epoch = Epoch::genesis(vs);
        let mut height = Height::GENESIS;

        let b1 = make_block(1, BlockHash::GENESIS);
        let b3 = make_block(3, BlockHash([99u8; 32])); // wrong parent

        assert!(replay_blocks(&[b1, b3], &mut store, &app, &mut epoch, &mut height).is_err());
    }
}
