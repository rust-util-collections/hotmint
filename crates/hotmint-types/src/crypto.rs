use ruc::*;
use serde::{Deserialize, Serialize};

use crate::validator::{ValidatorId, ValidatorSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature(pub Vec<u8>);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PublicKey(pub Vec<u8>);

/// Simple aggregate signature: bitfield indicating which validators signed + individual signatures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateSignature {
    pub signers: Vec<bool>,
    pub signatures: Vec<Signature>,
}

impl AggregateSignature {
    pub fn new(validator_count: usize) -> Self {
        Self {
            signers: vec![false; validator_count],
            signatures: Vec::new(),
        }
    }

    pub fn add(&mut self, index: usize, sig: Signature) -> Result<()> {
        if index >= self.signers.len() {
            return Err(eg!("signer index out of bounds"));
        }
        if self.signers[index] {
            return Err(eg!("duplicate signature from validator {}", index));
        }
        self.signers[index] = true;
        self.signatures.push(sig);
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.signers.iter().filter(|&&s| s).count()
    }
}

pub trait Signer: Send + Sync {
    fn sign(&self, message: &[u8]) -> Signature;
    fn public_key(&self) -> PublicKey;
    fn validator_id(&self) -> ValidatorId;
}

pub trait Verifier: Send + Sync {
    fn verify(&self, pk: &PublicKey, msg: &[u8], sig: &Signature) -> bool;
    fn verify_aggregate(&self, vs: &ValidatorSet, msg: &[u8], agg: &AggregateSignature) -> bool;
}
