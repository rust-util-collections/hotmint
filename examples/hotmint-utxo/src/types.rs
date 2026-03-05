use serde::{Deserialize, Serialize};

/// A reference to a specific output of a previous transaction.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutPoint {
    pub txid: [u8; 32],
    pub vout: u32,
}

impl OutPoint {
    /// Serialize to a 36-byte key: `txid(32) || vout_be(4)`.
    ///
    /// This format is used as the key in vsdb storage and preserves
    /// lexicographic ordering (same txid groups together, ascending vout).
    pub fn to_key(&self) -> [u8; 36] {
        let mut key = [0u8; 36];
        key[..32].copy_from_slice(&self.txid);
        key[32..].copy_from_slice(&self.vout.to_be_bytes());
        key
    }

    /// Deserialize from a 36-byte key.
    pub fn from_key(key: &[u8; 36]) -> Self {
        let mut txid = [0u8; 32];
        txid.copy_from_slice(&key[..32]);
        let vout = u32::from_be_bytes(key[32..36].try_into().unwrap());
        Self { txid, vout }
    }
}

/// A transaction output (value locked to a pubkey hash).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxOutput {
    /// Value in satoshis.
    pub value: u64,
    /// blake3 hash of the owner's ed25519 public key.
    pub pubkey_hash: [u8; 32],
}

/// A transaction input (spends a previous output).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxInput {
    /// The output being spent.
    pub prev_out: OutPoint,
    /// ed25519 signature over the transaction's signing hash.
    pub signature: Vec<u8>,
    /// ed25519 public key (32 bytes) for verification.
    pub pubkey: [u8; 32],
}

/// A UTXO transaction: consumes inputs, creates outputs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UtxoTx {
    pub inputs: Vec<TxInput>,
    pub outputs: Vec<TxOutput>,
}

impl UtxoTx {
    /// Compute the signing hash (used for both txid and signature verification).
    ///
    /// Process: clone the tx, zero all signatures, CBOR-encode, blake3-hash.
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut tx_copy = self.clone();
        for input in &mut tx_copy.inputs {
            input.signature = vec![0u8; 64];
        }
        let bytes = serde_cbor_2::to_vec(&tx_copy).unwrap_or_default();
        *blake3::hash(&bytes).as_bytes()
    }

    /// Transaction ID (same as signing hash).
    pub fn txid(&self) -> [u8; 32] {
        self.signing_hash()
    }

    /// CBOR-encode this transaction.
    pub fn encode(&self) -> Vec<u8> {
        serde_cbor_2::to_vec(self).unwrap_or_default()
    }

    /// Decode a transaction from CBOR bytes.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_cbor_2::from_slice(bytes).ok()
    }
}

/// Hash an ed25519 public key to produce an address (pubkey_hash).
pub fn hash_pubkey(pubkey: &[u8; 32]) -> [u8; 32] {
    *blake3::hash(pubkey).as_bytes()
}

/// Encode multiple transactions into a hotmint payload (length-prefixed).
pub fn encode_payload(txs: &[UtxoTx]) -> Vec<u8> {
    let mut payload = Vec::new();
    for tx in txs {
        let bytes = tx.encode();
        let len = bytes.len() as u32;
        payload.extend_from_slice(&len.to_le_bytes());
        payload.extend_from_slice(&bytes);
    }
    payload
}
