use crate::types::StakingConfig;

/// Calculate the block reward for the proposer.
///
/// This is the simplest reward model: a fixed amount per block goes to the
/// proposer's self-stake. More sophisticated models (proportional to voting
/// power, shared with voters, fee-based) can be implemented by the
/// application on top of this.
pub fn proposer_reward(config: &StakingConfig) -> u64 {
    config.block_reward
}
