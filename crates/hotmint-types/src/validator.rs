use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use crate::crypto::{PublicKey, Signer};
use crate::view::ViewNumber;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ValidatorId(pub u64);

impl fmt::Display for ValidatorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "V{}", self.0)
    }
}

impl From<u64> for ValidatorId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    pub id: ValidatorId,
    pub public_key: PublicKey,
    pub power: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidatorSet {
    validators: Vec<ValidatorInfo>,
    total_power: u64,
    /// O(1) lookup: ValidatorId -> index in validators vec
    #[serde(skip)]
    index_map: HashMap<ValidatorId, usize>,
}

impl<'de> Deserialize<'de> for ValidatorSet {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            validators: Vec<ValidatorInfo>,
            total_power: u64,
        }
        let raw = Raw::deserialize(deserializer)?;
        let index_map = raw
            .validators
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id, i))
            .collect();
        Ok(ValidatorSet {
            validators: raw.validators,
            total_power: raw.total_power,
            index_map,
        })
    }
}

impl ValidatorSet {
    pub fn new(validators: Vec<ValidatorInfo>) -> Self {
        let total_power = validators.iter().map(|v| v.power).sum();
        let index_map = validators
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id, i))
            .collect();
        Self {
            validators,
            total_power,
            index_map,
        }
    }

    /// Build a `ValidatorSet` from signers with equal power (1 each).
    pub fn from_signers(signers: &[&dyn Signer]) -> Self {
        let validators: Vec<ValidatorInfo> = signers
            .iter()
            .map(|s| ValidatorInfo {
                id: s.validator_id(),
                public_key: s.public_key(),
                power: 1,
            })
            .collect();
        Self::new(validators)
    }

    /// Rebuild the index map after deserialization.
    ///
    /// NOTE: This is now called automatically during deserialization.
    /// You only need to call this manually if you modify the validators
    /// list directly.
    pub fn rebuild_index(&mut self) {
        self.index_map = self
            .validators
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id, i))
            .collect();
    }

    pub fn validators(&self) -> &[ValidatorInfo] {
        &self.validators
    }

    pub fn total_power(&self) -> u64 {
        self.total_power
    }

    /// Quorum threshold: ceil(2n/3) where n = total_power
    pub fn quorum_threshold(&self) -> u64 {
        self.total_power
            .checked_mul(2)
            .expect("total_power overflow in quorum_threshold")
            .div_ceil(3)
    }

    /// Maximum faulty power: total_power - quorum_threshold
    pub fn max_faulty_power(&self) -> u64 {
        self.total_power - self.quorum_threshold()
    }

    /// Round-robin leader selection: v mod n.
    /// Returns `None` if the validator set is empty.
    pub fn leader_for_view(&self, view: ViewNumber) -> Option<&ValidatorInfo> {
        if self.validators.is_empty() {
            return None;
        }
        let idx = (view.as_u64() as usize) % self.validators.len();
        Some(&self.validators[idx])
    }

    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

    /// O(1) index lookup
    pub fn index_of(&self, id: ValidatorId) -> Option<usize> {
        self.index_map.get(&id).copied()
    }

    /// O(1) validator info lookup
    pub fn get(&self, id: ValidatorId) -> Option<&ValidatorInfo> {
        self.index_map.get(&id).map(|&idx| &self.validators[idx])
    }

    pub fn power_of(&self, id: ValidatorId) -> u64 {
        self.get(id).map_or(0, |v| v.power)
    }

    /// Apply validator updates and return a new ValidatorSet.
    /// - `power > 0`: update existing validator's power/key, or add new validator
    /// - `power == 0`: remove validator
    pub fn apply_updates(
        &self,
        updates: &[crate::validator_update::ValidatorUpdate],
    ) -> ValidatorSet {
        let mut infos: Vec<ValidatorInfo> = self.validators.clone();

        for update in updates {
            if update.power == 0 {
                infos.retain(|v| v.id != update.id);
            } else if let Some(existing) = infos.iter_mut().find(|v| v.id == update.id) {
                existing.power = update.power;
                existing.public_key = update.public_key.clone();
            } else {
                infos.push(ValidatorInfo {
                    id: update.id,
                    public_key: update.public_key.clone(),
                    power: update.power,
                });
            }
        }

        ValidatorSet::new(infos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vs(powers: &[u64]) -> ValidatorSet {
        let validators: Vec<ValidatorInfo> = powers
            .iter()
            .enumerate()
            .map(|(i, &p)| ValidatorInfo {
                id: ValidatorId(i as u64),
                public_key: PublicKey(vec![i as u8]),
                power: p,
            })
            .collect();
        ValidatorSet::new(validators)
    }

    #[test]
    fn test_quorum_4_equal() {
        let vs = make_vs(&[1, 1, 1, 1]);
        assert_eq!(vs.total_power(), 4);
        assert_eq!(vs.quorum_threshold(), 3);
        assert_eq!(vs.max_faulty_power(), 1);
    }

    #[test]
    fn test_quorum_3_equal() {
        let vs = make_vs(&[1, 1, 1]);
        assert_eq!(vs.quorum_threshold(), 2);
        assert_eq!(vs.max_faulty_power(), 1);
    }

    #[test]
    fn test_quorum_weighted() {
        let vs = make_vs(&[10, 10, 10, 70]);
        assert_eq!(vs.quorum_threshold(), 67);
        assert_eq!(vs.max_faulty_power(), 33);
    }

    #[test]
    fn test_quorum_single_validator() {
        let vs = make_vs(&[1]);
        assert_eq!(vs.quorum_threshold(), 1);
        assert_eq!(vs.max_faulty_power(), 0);
    }

    #[test]
    fn test_leader_rotation() {
        let vs = make_vs(&[1, 1, 1, 1]);
        assert_eq!(
            vs.leader_for_view(ViewNumber(0)).unwrap().id,
            ValidatorId(0)
        );
        assert_eq!(
            vs.leader_for_view(ViewNumber(1)).unwrap().id,
            ValidatorId(1)
        );
        assert_eq!(
            vs.leader_for_view(ViewNumber(4)).unwrap().id,
            ValidatorId(0)
        );
        assert_eq!(
            vs.leader_for_view(ViewNumber(7)).unwrap().id,
            ValidatorId(3)
        );
    }

    #[test]
    fn test_index_of_o1() {
        let vs = make_vs(&[5, 10, 15]);
        assert_eq!(vs.index_of(ValidatorId(0)), Some(0));
        assert_eq!(vs.index_of(ValidatorId(1)), Some(1));
        assert_eq!(vs.index_of(ValidatorId(2)), Some(2));
        assert_eq!(vs.index_of(ValidatorId(99)), None);
    }

    #[test]
    fn test_get_and_power_of() {
        let vs = make_vs(&[5, 10, 15]);
        assert_eq!(vs.get(ValidatorId(1)).unwrap().power, 10);
        assert!(vs.get(ValidatorId(99)).is_none());
        assert_eq!(vs.power_of(ValidatorId(2)), 15);
        assert_eq!(vs.power_of(ValidatorId(99)), 0);
    }

    #[test]
    fn test_apply_updates_add_validator() {
        let vs = make_vs(&[1, 1, 1]);
        let updates = vec![crate::validator_update::ValidatorUpdate {
            id: ValidatorId(3),
            public_key: PublicKey(vec![3]),
            power: 2,
        }];
        let new_vs = vs.apply_updates(&updates);
        assert_eq!(new_vs.validator_count(), 4);
        assert_eq!(new_vs.power_of(ValidatorId(3)), 2);
        assert_eq!(new_vs.total_power(), 5);
    }

    #[test]
    fn test_apply_updates_remove_validator() {
        let vs = make_vs(&[1, 1, 1, 1]);
        let updates = vec![crate::validator_update::ValidatorUpdate {
            id: ValidatorId(2),
            public_key: PublicKey(vec![2]),
            power: 0,
        }];
        let new_vs = vs.apply_updates(&updates);
        assert_eq!(new_vs.validator_count(), 3);
        assert!(new_vs.get(ValidatorId(2)).is_none());
        assert_eq!(new_vs.total_power(), 3);
    }

    #[test]
    fn test_apply_updates_change_power() {
        let vs = make_vs(&[1, 1, 1, 1]);
        let updates = vec![crate::validator_update::ValidatorUpdate {
            id: ValidatorId(0),
            public_key: PublicKey(vec![0]),
            power: 10,
        }];
        let new_vs = vs.apply_updates(&updates);
        assert_eq!(new_vs.validator_count(), 4);
        assert_eq!(new_vs.power_of(ValidatorId(0)), 10);
        assert_eq!(new_vs.total_power(), 13);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let vs = make_vs(&[1, 2, 3]);
        let bytes = serde_cbor_2::to_vec(&vs).unwrap();
        let vs2: ValidatorSet = serde_cbor_2::from_slice(&bytes).unwrap();
        // index_map is auto-rebuilt during deserialization
        assert_eq!(vs2.validator_count(), 3);
        assert_eq!(vs2.index_of(ValidatorId(1)), Some(1));
        assert_eq!(vs2.total_power(), 6);
    }
}
