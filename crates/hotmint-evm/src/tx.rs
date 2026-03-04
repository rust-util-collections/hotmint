use serde::{Deserialize, Serialize};

/// Simple EVM transaction for hotmint chains.
///
/// Uses CBOR encoding for simplicity. Production chains may use full
/// Ethereum RLP-encoded signed transactions instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTx {
    pub from: [u8; 20],
    pub to: [u8; 20],
    pub value: u128,
    pub nonce: u64,
    pub gas_limit: u64,
    pub data: Vec<u8>,
}

impl EvmTx {
    /// Create a plain ETH transfer transaction.
    pub fn transfer(from: [u8; 20], to: [u8; 20], value_wei: u128, nonce: u64) -> Self {
        Self {
            from,
            to,
            value: value_wei,
            nonce,
            gas_limit: 21_000,
            data: vec![],
        }
    }

    /// Create a contract call / deploy transaction.
    pub fn call(
        from: [u8; 20],
        to: [u8; 20],
        value_wei: u128,
        nonce: u64,
        gas_limit: u64,
        data: Vec<u8>,
    ) -> Self {
        Self {
            from,
            to,
            value: value_wei,
            nonce,
            gas_limit,
            data,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_cbor_2::to_vec(self).expect("tx serialization")
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_cbor_2::from_slice(bytes).ok()
    }
}

/// Encode multiple transactions into a hotmint payload (length-prefixed).
pub fn encode_payload(txs: &[EvmTx]) -> Vec<u8> {
    let mut payload = Vec::new();
    for tx in txs {
        let bytes = tx.encode();
        let len = bytes.len() as u32;
        payload.extend_from_slice(&len.to_le_bytes());
        payload.extend_from_slice(&bytes);
    }
    payload
}
