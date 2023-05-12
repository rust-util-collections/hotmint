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

    /// Maximum faulty: f = floor((n-1)/3)
    pub fn max_faulty(&self) -> usize {
        let n = self.validators.len();
        (n - 1) / 3
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
