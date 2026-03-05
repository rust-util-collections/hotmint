use ruc::*;

use hotmint_types::crypto::PublicKey;
use hotmint_types::validator::{ValidatorId, ValidatorSet};
use hotmint_types::validator_update::ValidatorUpdate;

use crate::rewards;
use crate::store::StakingStore;
use crate::types::{
    SlashReason, SlashResult, StakeEntry, StakingConfig, UnbondingEntry, ValidatorState,
};

/// Central staking manager that operates on any [`StakingStore`] backend.
///
/// Use this inside your [`Application::execute_block`] to process staking
/// transactions, distribute rewards, apply slashing, and compute validator
/// set updates for epoch transitions.
pub struct StakingManager<S: StakingStore> {
    store: S,
    config: StakingConfig,
}

impl<S: StakingStore> StakingManager<S> {
    pub fn new(store: S, config: StakingConfig) -> Self {
        Self { store, config }
    }

    pub fn config(&self) -> &StakingConfig {
        &self.config
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    // ── Validator registration ─────────────────────────────────────

    /// Register a new validator with an initial self-stake.
    pub fn register_validator(
        &mut self,
        id: ValidatorId,
        pubkey: PublicKey,
        self_stake: u64,
    ) -> Result<()> {
        if self.store.get_validator(id).is_some() {
            return Err(eg!("validator {} already registered", id));
        }
        if self_stake < self.config.min_self_stake {
            return Err(eg!(
                "self-stake {} below minimum {}",
                self_stake,
                self.config.min_self_stake
            ));
        }

        let state = ValidatorState {
            id,
            public_key: pubkey,
            self_stake,
            delegated_stake: 0,
            score: self.config.initial_score,
            jailed: false,
            jail_until_height: 0,
        };
        self.store.set_validator(id, state);
        Ok(())
    }

    /// Remove a validator from the staking system entirely.
    /// All delegated stakes are returned (caller handles balance credits).
    pub fn unregister_validator(&mut self, id: ValidatorId) -> Result<u64> {
        let state = self
            .store
            .get_validator(id)
            .ok_or_else(|| eg!("validator {} not found", id))?;
        if state.jailed {
            return Err(eg!("cannot unregister validator {} while jailed", id));
        }
        let total = state.total_stake();
        // Remove all delegation entries
        let stakers = self.store.stakers_of(id);
        for (addr, _) in stakers {
            self.store.remove_stake(&addr, id);
        }
        self.store.remove_validator(id);
        Ok(total)
    }

    // ── Delegation ─────────────────────────────────────────────────

    /// Delegate `amount` from `staker` to `validator`.
    pub fn delegate(&mut self, staker: &[u8], validator: ValidatorId, amount: u64) -> Result<()> {
        if amount == 0 {
            return Err(eg!("cannot delegate zero amount"));
        }
        let mut vs = self
            .store
            .get_validator(validator)
            .ok_or_else(|| eg!("validator {} not found", validator))?;

        vs.delegated_stake = vs
            .delegated_stake
            .checked_add(amount)
            .ok_or_else(|| eg!("delegated stake overflow for validator {}", validator))?;
        self.store.set_validator(validator, vs);

        let mut entry = self
            .store
            .get_stake(staker, validator)
            .unwrap_or(StakeEntry { amount: 0 });
        entry.amount = entry
            .amount
            .checked_add(amount)
            .ok_or_else(|| eg!("stake entry overflow"))?;
        self.store.set_stake(staker, validator, entry);
        Ok(())
    }

    /// Undelegate `amount` from `staker`'s delegation to `validator`.
    ///
    /// Voting power is reduced immediately. If `unbonding_period > 0`, the
    /// tokens are locked in an unbonding queue and released only after
    /// [`process_unbonding`] is called at or after `current_height + unbonding_period`.
    pub fn undelegate(
        &mut self,
        staker: &[u8],
        validator: ValidatorId,
        amount: u64,
        current_height: u64,
    ) -> Result<()> {
        if amount == 0 {
            return Err(eg!("cannot undelegate zero amount"));
        }
        let mut vs = self
            .store
            .get_validator(validator)
            .ok_or_else(|| eg!("validator {} not found", validator))?;
        let mut entry = self
            .store
            .get_stake(staker, validator)
            .ok_or_else(|| eg!("no stake from staker to validator {}", validator))?;

        if entry.amount < amount {
            return Err(eg!(
                "insufficient delegation: have {}, requested {}",
                entry.amount,
                amount
            ));
        }

        entry.amount -= amount;
        vs.delegated_stake = vs.delegated_stake.saturating_sub(amount);

        if entry.amount == 0 {
            self.store.remove_stake(staker, validator);
        } else {
            self.store.set_stake(staker, validator, entry);
        }
        self.store.set_validator(validator, vs);

        // Queue unbonding entry
        let completion_height = current_height.saturating_add(self.config.unbonding_period);
        self.store.push_unbonding(UnbondingEntry {
            staker: staker.to_vec(),
            validator,
            amount,
            completion_height,
        });

        Ok(())
    }

    /// Process mature unbondings whose lock period has elapsed.
    ///
    /// Returns the completed entries so the application can credit the
    /// released tokens to the stakers' balances.
    pub fn process_unbonding(&mut self, current_height: u64) -> Vec<UnbondingEntry> {
        self.store.drain_mature_unbondings(current_height)
    }

    // ── Slashing ───────────────────────────────────────────────────

    /// Slash a validator for misbehavior.
    ///
    /// Reduces self-stake and delegated stakes proportionally, jails the
    /// validator, and returns the total slashed amount.
    pub fn slash(
        &mut self,
        id: ValidatorId,
        reason: SlashReason,
        current_height: u64,
    ) -> Result<SlashResult> {
        let mut vs = self
            .store
            .get_validator(id)
            .ok_or_else(|| eg!("validator {} not found", id))?;

        let rate = match reason {
            SlashReason::DoubleSign => self.config.slash_rate_double_sign,
            SlashReason::Downtime => self.config.slash_rate_downtime,
        };

        let self_slash = (vs.self_stake as u128 * rate as u128 / 10_000) as u64;
        let del_slash = (vs.delegated_stake as u128 * rate as u128 / 10_000) as u64;

        vs.self_stake = vs.self_stake.saturating_sub(self_slash);
        vs.delegated_stake = vs.delegated_stake.saturating_sub(del_slash);
        vs.jailed = true;
        vs.jail_until_height = current_height.saturating_add(self.config.jail_duration);
        vs.score = vs.score.saturating_sub(self.config.max_score / 10);

        // Proportionally reduce each staker's delegation.
        // The last staker absorbs any rounding remainder so that
        // sum(staker.amount) == vs.delegated_stake after slashing.
        if del_slash > 0 {
            let stakers = self.store.stakers_of(id);
            let total_del: u64 = stakers.iter().map(|(_, e)| e.amount).sum();
            if total_del > 0 {
                let count = stakers.len();
                let mut slashed_so_far = 0u64;
                for (i, (addr, mut entry)) in stakers.into_iter().enumerate() {
                    let staker_slash = if i == count - 1 {
                        // Last staker absorbs remainder
                        del_slash.saturating_sub(slashed_so_far)
                    } else {
                        (entry.amount as u128 * del_slash as u128 / total_del as u128) as u64
                    };
                    slashed_so_far = slashed_so_far.saturating_add(staker_slash);
                    entry.amount = entry.amount.saturating_sub(staker_slash);
                    if entry.amount == 0 {
                        self.store.remove_stake(&addr, id);
                    } else {
                        self.store.set_stake(&addr, id, entry);
                    }
                }
            }
        }

        self.store.set_validator(id, vs);

        // Slash pending unbondings for this validator at the same rate
        let unbondings = self.store.all_unbondings();
        let mut unbonding_slashed = 0u64;
        let updated: Vec<UnbondingEntry> = unbondings
            .into_iter()
            .map(|mut e| {
                if e.validator == id && e.amount > 0 {
                    let ub_slash = (e.amount as u128 * rate as u128 / 10_000) as u64;
                    e.amount = e.amount.saturating_sub(ub_slash);
                    unbonding_slashed = unbonding_slashed.saturating_add(ub_slash);
                }
                e
            })
            .filter(|e| e.amount > 0)
            .collect();
        self.store.replace_unbondings(updated);

        Ok(SlashResult {
            self_slashed: self_slash,
            delegated_slashed: del_slash.saturating_add(unbonding_slashed),
            jailed: true,
        })
    }

    /// Unjail a validator if the jail period has passed.
    pub fn unjail(&mut self, id: ValidatorId, current_height: u64) -> Result<()> {
        let mut vs = self
            .store
            .get_validator(id)
            .ok_or_else(|| eg!("validator {} not found", id))?;
        if !vs.jailed {
            return Err(eg!("validator {} is not jailed", id));
        }
        if current_height < vs.jail_until_height {
            return Err(eg!(
                "validator {} jailed until height {}, current {}",
                id,
                vs.jail_until_height,
                current_height
            ));
        }
        vs.jailed = false;
        vs.jail_until_height = 0;
        self.store.set_validator(id, vs);
        Ok(())
    }

    // ── Reputation score ───────────────────────────────────────────

    /// Increase a validator's reputation score.
    pub fn increment_score(&mut self, id: ValidatorId, delta: u32) {
        if let Some(mut vs) = self.store.get_validator(id) {
            vs.score = vs.score.saturating_add(delta).min(self.config.max_score);
            self.store.set_validator(id, vs);
        }
    }

    /// Decrease a validator's reputation score.
    pub fn decrement_score(&mut self, id: ValidatorId, delta: u32) {
        if let Some(mut vs) = self.store.get_validator(id) {
            vs.score = vs.score.saturating_sub(delta);
            self.store.set_validator(id, vs);
        }
    }

    // ── Rewards ────────────────────────────────────────────────────

    /// Add the configured block reward to the proposer's self-stake.
    /// Returns the reward amount.
    pub fn reward_proposer(&mut self, proposer: ValidatorId) -> Result<u64> {
        let reward = rewards::proposer_reward(&self.config);
        if reward == 0 {
            return Ok(0);
        }
        let mut vs = self
            .store
            .get_validator(proposer)
            .ok_or_else(|| eg!("proposer {} not found", proposer))?;
        vs.self_stake = vs
            .self_stake
            .checked_add(reward)
            .ok_or_else(|| eg!("stake overflow on reward"))?;
        self.store.set_validator(proposer, vs);
        Ok(reward)
    }

    // ── Queries ────────────────────────────────────────────────────

    pub fn get_validator(&self, id: ValidatorId) -> Option<ValidatorState> {
        self.store.get_validator(id)
    }

    pub fn voting_power(&self, id: ValidatorId) -> u64 {
        self.store
            .get_validator(id)
            .map(|vs| vs.voting_power())
            .unwrap_or(0)
    }

    pub fn total_staked(&self) -> u64 {
        self.store
            .all_validator_ids()
            .into_iter()
            .filter_map(|id| self.store.get_validator(id))
            .map(|vs| vs.total_stake())
            .fold(0u64, u64::saturating_add)
    }

    /// Return the top `max_validators` validators sorted by voting power (descending).
    /// Jailed validators are excluded.
    pub fn formal_validator_list(&self) -> Vec<ValidatorState> {
        let mut active: Vec<ValidatorState> = self
            .store
            .all_validator_ids()
            .into_iter()
            .filter_map(|id| self.store.get_validator(id))
            .filter(|vs| !vs.jailed && vs.self_stake >= self.config.min_self_stake)
            .collect();
        active.sort_by_key(|v| std::cmp::Reverse(v.voting_power()));
        active.truncate(self.config.max_validators);
        active
    }

    // ── Epoch integration ──────────────────────────────────────────

    /// Compare current staking state against the active `ValidatorSet` and
    /// produce the list of [`ValidatorUpdate`]s needed to synchronize them.
    ///
    /// This is intended to be called at the end of `execute_block` and
    /// returned in [`EndBlockResponse::validator_updates`].
    pub fn compute_validator_updates(&self, current_set: &ValidatorSet) -> Vec<ValidatorUpdate> {
        let formal = self.formal_validator_list();
        let mut updates = Vec::new();

        // Add or update validators in the formal list
        for vs in &formal {
            let new_power = vs.voting_power();
            let current_power = current_set.power_of(vs.id);
            if new_power != current_power {
                updates.push(ValidatorUpdate {
                    id: vs.id,
                    public_key: vs.public_key.clone(),
                    power: new_power,
                });
            }
        }

        // Remove validators no longer in the formal list
        for vi in current_set.validators() {
            if !formal.iter().any(|f| f.id == vi.id) {
                updates.push(ValidatorUpdate {
                    id: vi.id,
                    public_key: vi.public_key.clone(),
                    power: 0,
                });
            }
        }

        updates
    }
}
