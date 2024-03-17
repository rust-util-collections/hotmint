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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregate_signature_add() {
        let mut agg = AggregateSignature::new(4);
        assert_eq!(agg.count(), 0);
        agg.add(0, Signature(vec![1])).unwrap();
        assert_eq!(agg.count(), 1);
        agg.add(2, Signature(vec![2])).unwrap();
        assert_eq!(agg.count(), 2);
    }

    #[test]
    fn test_aggregate_signature_duplicate_rejected() {
        let mut agg = AggregateSignature::new(4);
        agg.add(1, Signature(vec![1])).unwrap();
        assert!(agg.add(1, Signature(vec![2])).is_err());
    }

    #[test]
    fn test_aggregate_signature_out_of_bounds() {
        let mut agg = AggregateSignature::new(3);
        assert!(agg.add(3, Signature(vec![1])).is_err());
        assert!(agg.add(100, Signature(vec![1])).is_err());
    }

    #[test]
    fn test_aggregate_signature_zero_size() {
        let agg = AggregateSignature::new(0);
        assert_eq!(agg.count(), 0);
    }
}
