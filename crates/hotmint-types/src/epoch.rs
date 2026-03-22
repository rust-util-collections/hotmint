use serde::{Deserialize, Serialize};
use std::fmt;

use crate::validator::ValidatorSet;
use crate::view::ViewNumber;

/// Epoch number — changes when the validator set changes
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EpochNumber(pub u64);

impl EpochNumber {
    pub const GENESIS: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0.checked_add(1).expect("epoch number overflow"))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EpochNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}", self.0)
    }
}

/// An epoch defines a validator set with a starting view
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Epoch {
    pub number: EpochNumber,
    pub start_view: ViewNumber,
    pub validator_set: ValidatorSet,
}

impl Epoch {
    pub fn new(number: EpochNumber, start_view: ViewNumber, validator_set: ValidatorSet) -> Self {
        Self {
            number,
            start_view,
            validator_set,
        }
    }

    pub fn genesis(validator_set: ValidatorSet) -> Self {
        Self::new(EpochNumber::GENESIS, ViewNumber(1), validator_set)
    }

    pub fn contains_view(&self, view: ViewNumber) -> bool {
        view >= self.start_view
    }
}
