use serde::{Deserialize, Serialize};
use std::fmt;

use crate::crypto::PublicKey;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSet {
    pub validators: Vec<ValidatorInfo>,
    pub total_power: u64,
}

impl ValidatorSet {
    pub fn new(validators: Vec<ValidatorInfo>) -> Self {
        let total_power = validators.iter().map(|v| v.power).sum();
        Self {
            validators,
            total_power,
        }
    }

    /// Quorum threshold: ceil(2n/3) where n = total_power
    pub fn quorum_threshold(&self) -> u64 {
        (self.total_power * 2).div_ceil(3)
    }

    /// Maximum faulty power: f = (total_power - quorum_threshold)
    /// i.e. total_power - ceil(2*total_power/3)
    pub fn max_faulty_power(&self) -> u64 {
        self.total_power - self.quorum_threshold()
    }

    /// Round-robin leader selection: v mod n
    pub fn leader_for_view(&self, view: ViewNumber) -> &ValidatorInfo {
        let idx = (view.as_u64() as usize) % self.validators.len();
        &self.validators[idx]
    }

    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

    pub fn index_of(&self, id: ValidatorId) -> Option<usize> {
        self.validators.iter().position(|v| v.id == id)
    }

    pub fn get(&self, id: ValidatorId) -> Option<&ValidatorInfo> {
        self.validators.iter().find(|v| v.id == id)
    }

    pub fn power_of(&self, id: ValidatorId) -> u64 {
        self.get(id).map_or(0, |v| v.power)
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
        assert_eq!(vs.total_power, 4);
        assert_eq!(vs.quorum_threshold(), 3); // ceil(8/3) = 3
        assert_eq!(vs.max_faulty_power(), 1); // 4 - 3 = 1
    }

    #[test]
    fn test_quorum_3_equal() {
        let vs = make_vs(&[1, 1, 1]);
        assert_eq!(vs.quorum_threshold(), 2);
        assert_eq!(vs.max_faulty_power(), 1);
    }

    #[test]
    fn test_quorum_weighted() {
        // 10+10+10+70 = 100, quorum = ceil(200/3) = 67
        let vs = make_vs(&[10, 10, 10, 70]);
        assert_eq!(vs.quorum_threshold(), 67);
        // max faulty = 100 - 67 = 33
        assert_eq!(vs.max_faulty_power(), 33);
    }

    #[test]
    fn test_leader_rotation() {
        let vs = make_vs(&[1, 1, 1, 1]);
        assert_eq!(vs.leader_for_view(ViewNumber(0)).id, ValidatorId(0));
        assert_eq!(vs.leader_for_view(ViewNumber(1)).id, ValidatorId(1));
        assert_eq!(vs.leader_for_view(ViewNumber(4)).id, ValidatorId(0));
        assert_eq!(vs.leader_for_view(ViewNumber(7)).id, ValidatorId(3));
    }

    #[test]
    fn test_index_of_and_power_of() {
        let vs = make_vs(&[5, 10, 15]);
        assert_eq!(vs.index_of(ValidatorId(1)), Some(1));
        assert_eq!(vs.index_of(ValidatorId(99)), None);
        assert_eq!(vs.power_of(ValidatorId(2)), 15);
        assert_eq!(vs.power_of(ValidatorId(99)), 0);
    }
}
