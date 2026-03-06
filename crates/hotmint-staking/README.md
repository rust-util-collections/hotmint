# hotmint-staking

[![crates.io](https://img.shields.io/crates/v/hotmint-staking.svg)](https://crates.io/crates/hotmint-staking)
[![docs.rs](https://docs.rs/hotmint-staking/badge.svg)](https://docs.rs/hotmint-staking)

Staking toolkit for the [Hotmint](https://github.com/rust-util-collections/hotmint) BFT consensus framework.

Provides validator registration, delegation, slashing, unbonding, and reward distribution with a pluggable storage backend.

## Features

- **Validator management** — register/unregister validators with minimum self-stake requirements
- **Delegation** — delegate and undelegate stake to validators
- **Slashing** — configurable penalties for double-signing and downtime, applied to both self-stake and delegations
- **Unbonding queue** — time-locked unstaking with configurable unbonding period
- **Rewards** — per-block proposer rewards
- **Jailing** — automatic jailing on slash with configurable jail duration
- **Pluggable storage** — implement `StakingStore` for any backend; includes `InMemoryStakingStore` for testing

## Usage

```rust
use hotmint_staking::*;

let config = StakingConfig {
    min_self_stake: 1_000,
    slash_rate_double_sign: 50,  // 50% slash
    slash_rate_downtime: 5,      // 5% slash
    unbonding_period: 100,       // 100 blocks
    ..Default::default()
};

let store = InMemoryStakingStore::new();
let mut manager = StakingManager::new(config, store);

// Register a validator with self-stake
manager.register_validator(validator_id, 10_000).unwrap();

// Delegate stake
manager.delegate(delegator, validator_id, 5_000).unwrap();

// Slash for misbehavior
manager.slash(validator_id, SlashReason::DoubleSign).unwrap();

// Process unbonding at each block
manager.process_unbonding(current_height);
```

## Types

| Type | Description |
|:-----|:------------|
| `StakingManager<S>` | Central manager, generic over storage backend |
| `StakingConfig` | Configuration (limits, rates, periods) |
| `ValidatorState` | Validator metadata, stakes, reputation, jail status |
| `StakeEntry` | A single delegation entry |
| `UnbondingEntry` | Pending unstake with completion height |
| `SlashReason` | `DoubleSign` or `Downtime` |
| `SlashResult` | Result of a slash operation |

## License

GPL-3.0-only
