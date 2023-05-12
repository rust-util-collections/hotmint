use serde::{Deserialize, Serialize};
use std::fmt;

use crate::validator::ValidatorId;
use crate::view::ViewNumber;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct BlockHash(pub [u8; 32]);

impl BlockHash {
    pub const GENESIS: Self = Self([0u8; 32]);

    pub fn is_genesis(&self) -> bool {
        self.0 == [0u8; 32]
    }
}

impl fmt::Display for BlockHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(&self.0[..4]))
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct Height(pub u64);

impl Height {
    pub const GENESIS: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Height {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "h{}", self.0)
    }
}

impl From<u64> for Height {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Block B_k := (b_k, h_{k-1})
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub height: Height,
    pub parent_hash: BlockHash,
    pub view: ViewNumber,
    pub proposer: ValidatorId,
    pub payload: Vec<u8>,
    pub hash: BlockHash,
}

impl Block {
    pub fn genesis() -> Self {
        Self {
            height: Height::GENESIS,
            parent_hash: BlockHash::GENESIS,
            view: ViewNumber::GENESIS,
            proposer: ValidatorId::default(),
            payload: Vec::new(),
            hash: BlockHash::GENESIS,
        }
    }
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}
