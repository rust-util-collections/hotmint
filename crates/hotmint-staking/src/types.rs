use serde::{Deserialize, Serialize};

use hotmint_types::crypto::PublicKey;
use hotmint_types::validator::ValidatorId;

/// Validator state within the staking system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorState {
    pub id: ValidatorId,
    pub public_key: PublicKey,
    /// Self-bonded stake.
    pub self_stake: u64,
    /// Total delegated stake from other stakers.
    pub delegated_stake: u64,
    /// Reputation score (0 to `config.max_score`).
    pub score: u32,
    /// Whether the validator is jailed (temporarily removed from active set).
    pub jailed: bool,
    /// Block height until which the validator remains jailed.
    pub jail_until_height: u64,
}

impl ValidatorState {
    /// Total stake = self-bonded + delegated.
    pub fn total_stake(&self) -> u64 {
        self.self_stake.saturating_add(self.delegated_stake)
    }

    /// Voting power: total stake if not jailed, 0 otherwise.
    pub fn voting_power(&self) -> u64 {
        if self.jailed { 0 } else { self.total_stake() }
    }
}

/// A single delegation entry from a staker to a validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakeEntry {
    pub amount: u64,
}

/// Reason for slashing a validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashReason {
    /// Equivocation (double-signing).
    DoubleSign,
    /// Extended downtime / inactivity.
    Downtime,
}

/// Result of a slash operation.
#[derive(Debug, Clone)]
pub struct SlashResult {
    /// Amount slashed from self-stake.
    pub self_slashed: u64,
    /// Amount slashed from delegated stakes.
    pub delegated_slashed: u64,
    /// Whether the validator was jailed.
    pub jailed: bool,
}

/// An entry in the unbonding queue.
///
/// When a delegator undelegates, voting power is reduced immediately but
/// the tokens are locked until `completion_height` to prevent slash evasion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnbondingEntry {
    pub staker: Vec<u8>,
    pub validator: ValidatorId,
    pub amount: u64,
    /// Block height at which the unbonding completes.
    pub completion_height: u64,
}

/// Staking system configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingConfig {
    /// Maximum number of active (non-jailed) validators in the formal set.
    pub max_validators: usize,
    /// Minimum self-stake required to register as a validator.
    pub min_self_stake: u64,
    /// Slash rate for double-signing (basis points: 500 = 5%).
    pub slash_rate_double_sign: u32,
    /// Slash rate for downtime (basis points: 100 = 1%).
    pub slash_rate_downtime: u32,
    /// Number of blocks a jailed validator must wait before unjailing.
    pub jail_duration: u64,
    /// Initial reputation score for new validators.
    pub initial_score: u32,
    /// Maximum reputation score.
    pub max_score: u32,
    /// Block reward (added to proposer's self-stake).
    pub block_reward: u64,
    /// Unbonding period in blocks. 0 = instant (legacy behavior).
    pub unbonding_period: u64,
}

impl StakingConfig {
    pub fn validate(&self) -> ruc::Result<()> {
        if self.max_validators == 0 {
            return Err(ruc::eg!("max_validators must be > 0"));
        }
        if self.slash_rate_double_sign > 10_000 {
            return Err(ruc::eg!(
                "slash_rate_double_sign must be <= 10000 (basis points)"
            ));
        }
        if self.slash_rate_downtime > 10_000 {
            return Err(ruc::eg!(
                "slash_rate_downtime must be <= 10000 (basis points)"
            ));
        }
        if self.initial_score > self.max_score {
            return Err(ruc::eg!("initial_score must be <= max_score"));
        }
        Ok(())
    }
}

impl Default for StakingConfig {
    fn default() -> Self {
        Self {
            max_validators: 100,
            min_self_stake: 1000,
            slash_rate_double_sign: 500, // 5%
            slash_rate_downtime: 100,    // 1%
            jail_duration: 1000,
            initial_score: 10_000,
            max_score: 10_000,
            block_reward: 100,
            unbonding_period: 1000,
        }
    }
}
