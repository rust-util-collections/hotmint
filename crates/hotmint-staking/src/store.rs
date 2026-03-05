use std::collections::HashMap;

use hotmint_types::validator::ValidatorId;

use crate::types::{StakeEntry, UnbondingEntry, ValidatorState};

/// Abstract storage backend for staking state.
///
/// Implement this trait to plug in any persistence layer (in-memory, vsdb,
/// RocksDB, etc.). The staking manager operates entirely through this
/// interface and never assumes a specific storage backend.
pub trait StakingStore {
    fn get_validator(&self, id: ValidatorId) -> Option<ValidatorState>;
    fn set_validator(&mut self, id: ValidatorId, state: ValidatorState);
    fn remove_validator(&mut self, id: ValidatorId);
    fn all_validator_ids(&self) -> Vec<ValidatorId>;

    fn get_stake(&self, staker: &[u8], validator: ValidatorId) -> Option<StakeEntry>;
    fn set_stake(&mut self, staker: &[u8], validator: ValidatorId, entry: StakeEntry);
    fn remove_stake(&mut self, staker: &[u8], validator: ValidatorId);
    /// Return all (staker_address, entry) pairs for a given validator.
    fn stakers_of(&self, validator: ValidatorId) -> Vec<(Vec<u8>, StakeEntry)>;

    // ── Unbonding queue ─────────────────────────────────────────────

    /// Append an entry to the unbonding queue.
    fn push_unbonding(&mut self, entry: UnbondingEntry);

    /// Remove and return all entries whose `completion_height <= current_height`.
    fn drain_mature_unbondings(&mut self, current_height: u64) -> Vec<UnbondingEntry>;

    /// Return all pending unbonding entries (for slashing).
    fn all_unbondings(&self) -> Vec<UnbondingEntry>;

    /// Replace the entire unbonding queue (used after slashing adjustments).
    fn replace_unbondings(&mut self, entries: Vec<UnbondingEntry>);
}

/// In-memory staking store for testing and demos.
#[derive(Default)]
pub struct InMemoryStakingStore {
    validators: HashMap<ValidatorId, ValidatorState>,
    /// Key: (staker_address, validator_id)
    stakes: HashMap<(Vec<u8>, ValidatorId), StakeEntry>,
    unbondings: Vec<UnbondingEntry>,
}

impl InMemoryStakingStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StakingStore for InMemoryStakingStore {
    fn get_validator(&self, id: ValidatorId) -> Option<ValidatorState> {
        self.validators.get(&id).cloned()
    }

    fn set_validator(&mut self, id: ValidatorId, state: ValidatorState) {
        self.validators.insert(id, state);
    }

    fn remove_validator(&mut self, id: ValidatorId) {
        self.validators.remove(&id);
    }

    fn all_validator_ids(&self) -> Vec<ValidatorId> {
        self.validators.keys().copied().collect()
    }

    fn get_stake(&self, staker: &[u8], validator: ValidatorId) -> Option<StakeEntry> {
        self.stakes.get(&(staker.to_vec(), validator)).cloned()
    }

    fn set_stake(&mut self, staker: &[u8], validator: ValidatorId, entry: StakeEntry) {
        self.stakes.insert((staker.to_vec(), validator), entry);
    }

    fn remove_stake(&mut self, staker: &[u8], validator: ValidatorId) {
        self.stakes.remove(&(staker.to_vec(), validator));
    }

    fn stakers_of(&self, validator: ValidatorId) -> Vec<(Vec<u8>, StakeEntry)> {
        self.stakes
            .iter()
            .filter(|((_, vid), _)| *vid == validator)
            .map(|((addr, _), entry)| (addr.clone(), entry.clone()))
            .collect()
    }

    fn push_unbonding(&mut self, entry: UnbondingEntry) {
        self.unbondings.push(entry);
    }

    fn drain_mature_unbondings(&mut self, current_height: u64) -> Vec<UnbondingEntry> {
        let (mature, pending): (Vec<_>, Vec<_>) = self
            .unbondings
            .drain(..)
            .partition(|e| e.completion_height <= current_height);
        self.unbondings = pending;
        mature
    }

    fn all_unbondings(&self) -> Vec<UnbondingEntry> {
        self.unbondings.clone()
    }

    fn replace_unbondings(&mut self, entries: Vec<UnbondingEntry>) {
        self.unbondings = entries;
    }
}
