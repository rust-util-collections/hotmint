use hotmint_staking::*;
use hotmint_types::crypto::PublicKey;
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};

fn pk(n: u8) -> PublicKey {
    PublicKey(vec![n])
}

fn make_manager() -> StakingManager<InMemoryStakingStore> {
    StakingManager::new(
        InMemoryStakingStore::new(),
        StakingConfig {
            max_validators: 4,
            min_self_stake: 100,
            ..StakingConfig::default()
        },
    )
}

#[test]
fn register_and_query() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();
    let vs = mgr.get_validator(ValidatorId(0)).unwrap();
    assert_eq!(vs.self_stake, 1000);
    assert_eq!(vs.delegated_stake, 0);
    assert_eq!(vs.voting_power(), 1000);
    assert!(!vs.jailed);
}

#[test]
fn register_below_minimum_fails() {
    let mut mgr = make_manager();
    assert!(mgr.register_validator(ValidatorId(0), pk(0), 50).is_err());
}

#[test]
fn register_duplicate_fails() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();
    assert!(mgr.register_validator(ValidatorId(0), pk(0), 1000).is_err());
}

#[test]
fn delegate_and_undelegate() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();

    mgr.delegate(b"alice", ValidatorId(0), 500).unwrap();
    assert_eq!(mgr.voting_power(ValidatorId(0)), 1500);

    mgr.undelegate(b"alice", ValidatorId(0), 200).unwrap();
    assert_eq!(mgr.voting_power(ValidatorId(0)), 1300);

    // Full undelegate
    mgr.undelegate(b"alice", ValidatorId(0), 300).unwrap();
    assert_eq!(mgr.voting_power(ValidatorId(0)), 1000);
}

#[test]
fn undelegate_insufficient_fails() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();
    mgr.delegate(b"alice", ValidatorId(0), 100).unwrap();
    assert!(mgr.undelegate(b"alice", ValidatorId(0), 200).is_err());
}

#[test]
fn slash_double_sign() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 10_000)
        .unwrap();
    mgr.delegate(b"alice", ValidatorId(0), 5_000).unwrap();

    let result = mgr
        .slash(ValidatorId(0), SlashReason::DoubleSign, 100)
        .unwrap();

    // 5% of 10000 = 500
    assert_eq!(result.self_slashed, 500);
    // 5% of 5000 = 250
    assert_eq!(result.delegated_slashed, 250);
    assert!(result.jailed);

    let vs = mgr.get_validator(ValidatorId(0)).unwrap();
    assert!(vs.jailed);
    assert_eq!(vs.voting_power(), 0); // jailed → power 0
    assert_eq!(vs.self_stake, 9_500);
}

#[test]
fn slash_downtime() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 10_000)
        .unwrap();

    let result = mgr
        .slash(ValidatorId(0), SlashReason::Downtime, 50)
        .unwrap();
    // 1% of 10000 = 100
    assert_eq!(result.self_slashed, 100);
    assert!(result.jailed);
}

#[test]
fn unjail_after_jail_period() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 10_000)
        .unwrap();
    mgr.slash(ValidatorId(0), SlashReason::Downtime, 100)
        .unwrap();

    // Too early
    assert!(mgr.unjail(ValidatorId(0), 500).is_err());

    // After jail period (100 + 1000 = 1100)
    mgr.unjail(ValidatorId(0), 1100).unwrap();
    let vs = mgr.get_validator(ValidatorId(0)).unwrap();
    assert!(!vs.jailed);
    assert!(vs.voting_power() > 0);
}

#[test]
fn reward_proposer() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();

    let reward = mgr.reward_proposer(ValidatorId(0)).unwrap();
    assert_eq!(reward, 100); // default block_reward
    assert_eq!(mgr.get_validator(ValidatorId(0)).unwrap().self_stake, 1100);
}

#[test]
fn total_staked() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();
    mgr.register_validator(ValidatorId(1), pk(1), 2000).unwrap();
    mgr.delegate(b"alice", ValidatorId(0), 500).unwrap();
    assert_eq!(mgr.total_staked(), 3500);
}

#[test]
fn formal_validator_list_sorted_by_power() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();
    mgr.register_validator(ValidatorId(1), pk(1), 3000).unwrap();
    mgr.register_validator(ValidatorId(2), pk(2), 2000).unwrap();

    let list = mgr.formal_validator_list();
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].id, ValidatorId(1)); // highest power
    assert_eq!(list[1].id, ValidatorId(2));
    assert_eq!(list[2].id, ValidatorId(0));
}

#[test]
fn formal_list_excludes_jailed() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 5000).unwrap();
    mgr.register_validator(ValidatorId(1), pk(1), 1000).unwrap();

    mgr.slash(ValidatorId(0), SlashReason::DoubleSign, 0)
        .unwrap();

    let list = mgr.formal_validator_list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, ValidatorId(1));
}

#[test]
fn formal_list_respects_max_validators() {
    let mut mgr = StakingManager::new(
        InMemoryStakingStore::new(),
        StakingConfig {
            max_validators: 2,
            min_self_stake: 100,
            ..StakingConfig::default()
        },
    );
    for i in 0..5u64 {
        mgr.register_validator(ValidatorId(i), pk(i as u8), (i + 1) * 1000)
            .unwrap();
    }
    let list = mgr.formal_validator_list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, ValidatorId(4)); // 5000
    assert_eq!(list[1].id, ValidatorId(3)); // 4000
}

#[test]
fn compute_validator_updates() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();
    mgr.register_validator(ValidatorId(1), pk(1), 2000).unwrap();

    // Current set has V0=1000, V1=2000
    let current = ValidatorSet::new(vec![
        ValidatorInfo {
            id: ValidatorId(0),
            public_key: pk(0),
            power: 1000,
        },
        ValidatorInfo {
            id: ValidatorId(1),
            public_key: pk(1),
            power: 2000,
        },
    ]);

    // No changes → empty updates
    let updates = mgr.compute_validator_updates(&current);
    assert!(updates.is_empty());

    // Delegate 500 to V0 → power change
    mgr.delegate(b"alice", ValidatorId(0), 500).unwrap();
    let updates = mgr.compute_validator_updates(&current);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].id, ValidatorId(0));
    assert_eq!(updates[0].power, 1500);
}

#[test]
fn compute_updates_add_and_remove() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();
    mgr.register_validator(ValidatorId(2), pk(2), 3000).unwrap();

    // Current set has V0 and V1 (V1 will be removed, V2 added)
    let current = ValidatorSet::new(vec![
        ValidatorInfo {
            id: ValidatorId(0),
            public_key: pk(0),
            power: 1000,
        },
        ValidatorInfo {
            id: ValidatorId(1),
            public_key: pk(1),
            power: 2000,
        },
    ]);

    let updates = mgr.compute_validator_updates(&current);
    // V2 added with power 3000, V1 removed with power 0
    assert_eq!(updates.len(), 2);
    let add = updates.iter().find(|u| u.id == ValidatorId(2)).unwrap();
    assert_eq!(add.power, 3000);
    let remove = updates.iter().find(|u| u.id == ValidatorId(1)).unwrap();
    assert_eq!(remove.power, 0);
}

#[test]
fn unregister_validator() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();
    mgr.delegate(b"alice", ValidatorId(0), 500).unwrap();

    let total = mgr.unregister_validator(ValidatorId(0)).unwrap();
    assert_eq!(total, 1500);
    assert!(mgr.get_validator(ValidatorId(0)).is_none());
}

#[test]
fn score_management() {
    let mut mgr = make_manager();
    mgr.register_validator(ValidatorId(0), pk(0), 1000).unwrap();

    mgr.decrement_score(ValidatorId(0), 500);
    assert_eq!(mgr.get_validator(ValidatorId(0)).unwrap().score, 9500);

    mgr.increment_score(ValidatorId(0), 1000); // capped at max
    assert_eq!(mgr.get_validator(ValidatorId(0)).unwrap().score, 10_000);

    mgr.decrement_score(ValidatorId(0), 20_000); // saturating
    assert_eq!(mgr.get_validator(ValidatorId(0)).unwrap().score, 0);
}
