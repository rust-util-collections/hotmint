use std::collections::HashSet;
use std::sync::Mutex;

use ed25519_dalek::{Signature, VerifyingKey};
use ruc::*;
use tracing::{info, warn};

use hotmint_consensus::application::Application;
use hotmint_types::Block;
use hotmint_types::context::BlockContext;
use hotmint_types::validator_update::EndBlockResponse;

use crate::utxo_state::UtxoState;
use crate::utxo_types::{OutPoint, TxOutput, UtxoTx, hash_pubkey};

/// A genesis UTXO allocation.
#[derive(Debug, Clone)]
pub struct GenesisUtxo {
    pub value: u64,
    pub pubkey_hash: [u8; 32],
}

/// UTXO chain configuration.
#[derive(Debug, Clone)]
pub struct UtxoConfig {
    /// Genesis UTXO allocations.
    pub genesis_utxos: Vec<GenesisUtxo>,
    /// Maximum number of inputs per transaction.
    pub max_tx_inputs: usize,
    /// Maximum number of outputs per transaction.
    pub max_tx_outputs: usize,
    /// Maximum serialized transaction size in bytes.
    pub max_tx_size: usize,
    /// Whether to log state info on commit.
    pub log_on_commit: bool,
}

impl Default for UtxoConfig {
    fn default() -> Self {
        Self {
            genesis_utxos: vec![],
            max_tx_inputs: 256,
            max_tx_outputs: 256,
            max_tx_size: 102_400,
            log_on_commit: false,
        }
    }
}

/// Ready-to-use UTXO application implementing [`Application`].
///
/// Provides a Bitcoin-style UTXO chain with:
/// - ed25519 signature verification
/// - Persistent state via vsdb `VerMapWithProof` with SMT proofs
/// - Address-indexed UTXO queries via `SlotDex`
///
/// **Prerequisite:** `vsdb::vsdb_set_base_dir()` must be called before
/// constructing this type.
pub struct UtxoApplication {
    state: Mutex<UtxoState>,
    config: UtxoConfig,
}

impl UtxoApplication {
    /// Create a new UTXO application with genesis allocations.
    pub fn new(config: UtxoConfig) -> Self {
        let mut state = UtxoState::new();

        // Insert genesis UTXOs with a deterministic "genesis txid"
        for (i, alloc) in config.genesis_utxos.iter().enumerate() {
            let genesis_txid = *blake3::hash(&(i as u64).to_le_bytes()).as_bytes();
            state.insert_genesis_utxo(
                genesis_txid,
                0,
                TxOutput {
                    value: alloc.value,
                    pubkey_hash: alloc.pubkey_hash,
                },
            );
        }

        // Commit genesis state
        let _ = state.commit();

        Self {
            state: Mutex::new(state),
            config,
        }
    }

    /// Query an address's balance.
    pub fn get_balance(&self, pubkey_hash: &[u8; 32]) -> u64 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.get_balance(pubkey_hash)
    }

    /// Query a specific UTXO.
    pub fn get_utxo(&self, outpoint: &OutPoint) -> Option<TxOutput> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.get_utxo(outpoint)
    }

    /// Get the total supply.
    pub fn total_supply(&self) -> u64 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.total_supply()
    }

    /// Validate a decoded transaction against the current UTXO set.
    fn validate_decoded_tx(&self, tx: &UtxoTx, tx_bytes_len: usize, state: &UtxoState) -> bool {
        // 1. Size limits
        if tx_bytes_len > self.config.max_tx_size {
            return false;
        }
        if tx.inputs.is_empty() || tx.outputs.is_empty() {
            return false;
        }
        if tx.inputs.len() > self.config.max_tx_inputs {
            return false;
        }
        if tx.outputs.len() > self.config.max_tx_outputs {
            return false;
        }

        // 2. No zero-value outputs
        if tx.outputs.iter().any(|o| o.value == 0) {
            return false;
        }

        // 3. No duplicate inputs
        let mut seen = HashSet::new();
        for input in &tx.inputs {
            if !seen.insert(input.prev_out.to_key()) {
                return false;
            }
        }

        // 4. Compute signing hash
        let signing_hash = tx.signing_hash();

        // 5. Verify each input
        let mut input_sum: u64 = 0;
        for input in &tx.inputs {
            // 5a. UTXO must exist
            let Some(utxo) = state.get_utxo(&input.prev_out) else {
                return false;
            };

            // 5b. Pubkey must match UTXO owner
            if hash_pubkey(&input.pubkey) != utxo.pubkey_hash {
                return false;
            }

            // 5c. Signature must be valid
            let Ok(verifying_key) = VerifyingKey::from_bytes(&input.pubkey) else {
                return false;
            };
            let sig_bytes: [u8; 64] = match input.signature.as_slice().try_into() {
                Ok(b) => b,
                Err(_) => return false,
            };
            let signature = Signature::from_bytes(&sig_bytes);
            if verifying_key
                .verify_strict(&signing_hash, &signature)
                .is_err()
            {
                return false;
            }

            input_sum = match input_sum.checked_add(utxo.value) {
                Some(v) => v,
                None => return false,
            };
        }

        // 6. Output sum must not exceed input sum
        let mut output_sum: u64 = 0;
        for output in &tx.outputs {
            output_sum = match output_sum.checked_add(output.value) {
                Some(v) => v,
                None => return false,
            };
        }
        if output_sum > input_sum {
            return false;
        }

        true
    }
}

impl Application for UtxoApplication {
    fn validate_tx(
        &self,
        tx_bytes: &[u8],
        _ctx: Option<&hotmint_types::context::TxContext>,
    ) -> bool {
        let Some(tx) = UtxoTx::decode(tx_bytes) else {
            return false;
        };
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.validate_decoded_tx(&tx, tx_bytes.len(), &state)
    }

    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut executed = 0u64;

        for tx_bytes in txs {
            let Some(tx) = UtxoTx::decode(tx_bytes) else {
                continue;
            };

            if !self.validate_decoded_tx(&tx, tx_bytes.len(), &state) {
                warn!(
                    height = ctx.height.as_u64(),
                    "UTXO tx validation failed, skipping"
                );
                continue;
            }

            let txid = tx.txid();
            state.apply_tx(&tx, &txid);
            executed += 1;
        }

        state.commit().c(d!("block commit failed"))?;
        let _root = state.utxo_root().unwrap_or_default();

        if self.config.log_on_commit {
            info!(
                height = ctx.height.as_u64(),
                txs = executed,
                supply = state.total_supply(),
                "block executed"
            );
        }

        Ok(EndBlockResponse::default())
    }

    fn on_commit(&self, _block: &Block, ctx: &BlockContext) -> Result<()> {
        if self.config.log_on_commit {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            for alloc in &self.config.genesis_utxos {
                let bal = state.get_balance(&alloc.pubkey_hash);
                info!(
                    height = ctx.height.as_u64(),
                    address = hex_encode(&alloc.pubkey_hash[..8]),
                    balance = bal,
                    "committed"
                );
            }
        }
        Ok(())
    }

    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match path {
            "balance" if data.len() == 32 => {
                let pubkey_hash: [u8; 32] = data.try_into().unwrap();
                let bal = state.get_balance(&pubkey_hash);
                Ok(bal.to_le_bytes().to_vec())
            }
            "utxo" if data.len() == 36 => {
                let key: [u8; 36] = data.try_into().unwrap();
                let outpoint = OutPoint::from_key(&key);
                match state.get_utxo(&outpoint) {
                    Some(output) => Ok(serde_cbor_2::to_vec(&output).unwrap_or_default()),
                    None => Ok(vec![]),
                }
            }
            "utxo_root" => {
                let root = state.utxo_root().unwrap_or_default();
                Ok(root)
            }
            "total_supply" => {
                let supply = state.total_supply();
                Ok(supply.to_le_bytes().to_vec())
            }
            "prove" if data.len() == 36 => {
                // Ensure trie is synced before proving
                let _ = state.utxo_root();
                let key: [u8; 36] = data.try_into().unwrap();
                let outpoint = OutPoint::from_key(&key);
                let proof = state.prove_utxo(&outpoint)?;
                // SmtProof doesn't derive Serialize; encode manually:
                // key_hash(32) || has_value(1) || [value_len(4) || value] || siblings(N*32)
                let mut buf = Vec::new();
                buf.extend_from_slice(&proof.key_hash);
                match &proof.value {
                    Some(v) => {
                        buf.push(1);
                        buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
                        buf.extend_from_slice(v);
                    }
                    None => buf.push(0),
                }
                for sibling in &proof.siblings {
                    buf.extend_from_slice(sibling);
                }
                Ok(buf)
            }
            _ => Ok(vec![]),
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
