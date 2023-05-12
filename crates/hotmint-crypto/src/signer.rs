use ed25519_dalek::Verifier as DalekVerifier;
use ed25519_dalek::{Signer as DalekSigner, SigningKey, VerifyingKey};
use hotmint_types::crypto::{PublicKey, Signature};
use hotmint_types::validator::{ValidatorId, ValidatorSet};
use hotmint_types::{AggregateSignature, Signer, Verifier};

/// Ed25519 signer implementation
pub struct Ed25519Signer {
    signing_key: SigningKey,
    validator_id: ValidatorId,
}

impl Ed25519Signer {
    pub fn new(signing_key: SigningKey, validator_id: ValidatorId) -> Self {
        Self {
            signing_key,
            validator_id,
        }
    }

    pub fn generate(validator_id: ValidatorId) -> Self {
        let mut rng = rand::thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        Self::new(signing_key, validator_id)
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }
}

impl Signer for Ed25519Signer {
    fn sign(&self, message: &[u8]) -> Signature {
        let sig = self.signing_key.sign(message);
        Signature(sig.to_bytes().to_vec())
    }

    fn public_key(&self) -> PublicKey {
        PublicKey(self.signing_key.verifying_key().to_bytes().to_vec())
    }

    fn validator_id(&self) -> ValidatorId {
        self.validator_id
    }
}

/// Ed25519 verifier implementation
pub struct Ed25519Verifier;

impl Verifier for Ed25519Verifier {
    fn verify(&self, pk: &PublicKey, msg: &[u8], sig: &Signature) -> bool {
        let vk_bytes: [u8; 32] = match pk.0.as_slice().try_into() {
            Ok(b) => b,
            Err(_) => return false,
        };
        let vk = match VerifyingKey::from_bytes(&vk_bytes) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let sig_bytes: [u8; 64] = match sig.0.as_slice().try_into() {
            Ok(b) => b,
            Err(_) => return false,
        };
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        vk.verify(msg, &signature).is_ok()
    }

    fn verify_aggregate(&self, vs: &ValidatorSet, msg: &[u8], agg: &AggregateSignature) -> bool {
        let mut sig_idx = 0;
        for (i, signed) in agg.signers.iter().enumerate() {
            if !signed {
                continue;
            }
            if sig_idx >= agg.signatures.len() {
                return false;
            }
            let pk = &vs.validators[i].public_key;
            if !self.verify(pk, msg, &agg.signatures[sig_idx]) {
                return false;
            }
            sig_idx += 1;
        }
        sig_idx == agg.signatures.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_verify() {
        let signer = Ed25519Signer::generate(ValidatorId(0));
        let msg = b"test message";
        let sig = signer.sign(msg);
        let pk = signer.public_key();

        let verifier = Ed25519Verifier;
        assert!(verifier.verify(&pk, msg, &sig));
        assert!(!verifier.verify(&pk, b"wrong message", &sig));
    }
}
