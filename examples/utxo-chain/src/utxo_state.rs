use ruc::*;
use vsdb::versioned::BranchId;
use vsdb::{Orphan, SlotDex128, SmtCalc, SmtProof, VerMapWithProof};

use crate::utxo_types::{OutPoint, TxOutput, UtxoTx};

/// Map a pubkey_hash to a u128 slot for SlotDex address indexing.
///
/// Uses the first 16 bytes of the 32-byte hash, giving 2^128 address
/// space — collision-free in practice.
fn addr_slot(pubkey_hash: &[u8; 32]) -> u128 {
    u128::from_be_bytes(pubkey_hash[..16].try_into().unwrap())
}

/// Persistent UTXO state backed by vsdb.
///
/// Manages the UTXO set (with SMT proofs), address-based index, and
/// total supply tracking. All operations are synchronous and expect
/// the caller to hold the appropriate lock.
///
/// **Prerequisite:** `vsdb::vsdb_set_base_dir()` must be called before
/// constructing this type.
pub struct UtxoState {
    /// Primary UTXO set with Sparse Merkle Tree commitment.
    utxos: VerMapWithProof<[u8; 36], TxOutput, SmtCalc>,
    /// Address -> UTXO index for paginated queries.
    addr_index: SlotDex128<[u8; 36]>,
    /// Total supply (sum of all UTXO values).
    total_supply: Orphan<u64>,
    /// Cached main branch ID.
    main_branch: BranchId,
}

impl Default for UtxoState {
    fn default() -> Self {
        Self::new()
    }
}

impl UtxoState {
    /// Create a new empty UTXO state.
    pub fn new() -> Self {
        let utxos = VerMapWithProof::new();
        let main_branch = utxos.map().main_branch();
        Self {
            utxos,
            addr_index: SlotDex128::new(8, false),
            total_supply: Orphan::new(0),
            main_branch,
        }
    }

    /// Look up a single UTXO by outpoint.
    pub fn get_utxo(&self, outpoint: &OutPoint) -> Option<TxOutput> {
        let key = outpoint.to_key();
        self.utxos.map().get(self.main_branch, &key).ok().flatten()
    }

    /// Apply a validated transaction: spend inputs, create outputs.
    pub fn apply_tx(&mut self, tx: &UtxoTx, txid: &[u8; 32]) {
        let branch = self.main_branch;
        let mut supply = self.total_supply.get_value();

        // Spend inputs
        for input in &tx.inputs {
            let key = input.prev_out.to_key();
            if let Ok(Some(spent_output)) = self.utxos.map().get(branch, &key) {
                supply = supply.saturating_sub(spent_output.value);
                let _ = self.utxos.map_mut().remove(branch, &key);
                let slot = addr_slot(&spent_output.pubkey_hash);
                self.addr_index.remove(slot, &key);
            }
        }

        // Create outputs
        for (vout, output) in tx.outputs.iter().enumerate() {
            let outpoint = OutPoint {
                txid: *txid,
                vout: vout as u32,
            };
            let key = outpoint.to_key();
            let _ = self.utxos.map_mut().insert(branch, &key, output);
            let slot = addr_slot(&output.pubkey_hash);
            let _ = self.addr_index.insert(slot, key);
            supply = supply.saturating_add(output.value);
        }

        self.total_supply.set_value(&supply);
    }

    /// Commit the current state (call once per block after all txs applied).
    pub fn commit(&mut self) -> Result<()> {
        self.utxos
            .map_mut()
            .commit(self.main_branch)
            .c(d!("utxo commit failed"))?;
        Ok(())
    }

    /// Compute the UTXO set SMT root hash (32 bytes).
    pub fn utxo_root(&mut self) -> Result<Vec<u8>> {
        self.utxos.merkle_root(self.main_branch).c(d!())
    }

    /// Generate an SMT inclusion/exclusion proof for a UTXO.
    ///
    /// Call `utxo_root()` first to ensure the trie is synced.
    pub fn prove_utxo(&self, outpoint: &OutPoint) -> Result<SmtProof> {
        let key = outpoint.to_key();
        self.utxos.prove(&key).c(d!())
    }

    /// Insert a genesis UTXO (before any blocks are committed).
    pub fn insert_genesis_utxo(&mut self, txid: [u8; 32], vout: u32, output: TxOutput) {
        let outpoint = OutPoint { txid, vout };
        let key = outpoint.to_key();
        let branch = self.main_branch;
        let slot = addr_slot(&output.pubkey_hash);
        let value = output.value;

        let _ = self.utxos.map_mut().insert(branch, &key, &output);
        let _ = self.addr_index.insert(slot, key);

        let supply = self.total_supply.get_value();
        self.total_supply.set_value(&supply.saturating_add(value));
    }

    /// Query all UTXOs owned by `pubkey_hash` (paginated).
    pub fn get_utxos_by_address(
        &self,
        pubkey_hash: &[u8; 32],
        page_index: u32,
        page_size: u16,
    ) -> Vec<OutPoint> {
        let slot = addr_slot(pubkey_hash);
        self.addr_index
            .get_entries_by_page_slot(Some(slot), Some(slot), page_size, page_index, false)
            .into_iter()
            .map(|key| OutPoint::from_key(&key))
            .collect()
    }

    /// Get the total balance for an address (sum of all its UTXO values).
    pub fn get_balance(&self, pubkey_hash: &[u8; 32]) -> u64 {
        let slot = addr_slot(pubkey_hash);
        let keys =
            self.addr_index
                .get_entries_by_page_slot(Some(slot), Some(slot), u16::MAX, 0, false);
        let branch = self.main_branch;
        keys.iter()
            .filter_map(|key| self.utxos.map().get(branch, key).ok().flatten())
            .map(|out| out.value)
            .sum()
    }

    /// Get the total supply across all UTXOs.
    pub fn total_supply(&self) -> u64 {
        self.total_supply.get_value()
    }
}
