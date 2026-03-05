use ed25519_dalek::SigningKey;
use ruc::*;

use hotmint_consensus::application::Application;
use hotmint_types::Block;
use hotmint_types::context::BlockContext;
use hotmint_types::validator_update::EndBlockResponse;

use crate::*;

/// Demo UTXO app: Alice sends 1 COIN to Bob each block.
pub struct DemoUtxoApp {
    inner: UtxoApplication,
    alice_key: SigningKey,
    bob_pkh: [u8; 32],
}

impl DemoUtxoApp {
    pub fn new(alice_key: SigningKey, bob_key: &SigningKey) -> Self {
        let alice_pk: [u8; 32] = alice_key.verifying_key().to_bytes();
        let alice_pkh = hash_pubkey(&alice_pk);
        let bob_pk: [u8; 32] = bob_key.verifying_key().to_bytes();
        let bob_pkh = hash_pubkey(&bob_pk);

        let config = UtxoConfig {
            genesis_utxos: vec![
                GenesisUtxo {
                    value: 100 * COIN,
                    pubkey_hash: alice_pkh,
                },
                GenesisUtxo {
                    value: 100 * COIN,
                    pubkey_hash: bob_pkh,
                },
            ],
            log_on_commit: true,
            ..Default::default()
        };

        Self {
            inner: UtxoApplication::new(config),
            alice_key,
            bob_pkh,
        }
    }

    /// Build and sign a transfer from Alice to Bob.
    fn build_transfer(&self) -> Option<UtxoTx> {
        let alice_pk = self.alice_key.verifying_key().to_bytes();
        let alice_pkh = hash_pubkey(&alice_pk);

        // Find an Alice UTXO with enough value
        let utxo_list = self.inner.query("balance", &alice_pkh).ok()?;
        if utxo_list.len() < 8 {
            return None;
        }
        let _balance = u64::from_le_bytes(utxo_list[..8].try_into().ok()?);

        // Use query to find Alice's UTXOs by scanning genesis txids
        // For the demo, we know genesis UTXOs have deterministic txids
        for i in 0u64..1000 {
            let genesis_txid = *blake3::hash(&i.to_le_bytes()).as_bytes();
            let outpoint = OutPoint {
                txid: genesis_txid,
                vout: 0,
            };
            if let Some(utxo) = self.inner.get_utxo(&outpoint)
                && utxo.pubkey_hash == alice_pkh
                && utxo.value >= COIN
            {
                let mut tx = UtxoTx {
                    inputs: vec![TxInput {
                        prev_out: outpoint,
                        signature: vec![0u8; 64],
                        pubkey: alice_pk,
                    }],
                    outputs: vec![
                        TxOutput {
                            value: COIN,
                            pubkey_hash: self.bob_pkh,
                        },
                        TxOutput {
                            value: utxo.value - COIN,
                            pubkey_hash: alice_pkh,
                        },
                    ],
                };

                // Remove zero-value change output
                tx.outputs.retain(|o| o.value > 0);

                // Sign
                let hash = tx.signing_hash();
                let sig: ed25519_dalek::Signature =
                    ed25519_dalek::Signer::sign(&self.alice_key, &hash);
                tx.inputs[0].signature = sig.to_bytes().to_vec();

                return Some(tx);
            }
        }
        None
    }
}

impl Application for DemoUtxoApp {
    fn create_payload(&self, _ctx: &BlockContext) -> Vec<u8> {
        match self.build_transfer() {
            Some(tx) => encode_payload(&[tx]),
            None => vec![],
        }
    }

    fn execute_block(&self, txs: &[&[u8]], ctx: &BlockContext) -> Result<EndBlockResponse> {
        self.inner.execute_block(txs, ctx)
    }

    fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()> {
        self.inner.on_commit(block, ctx)
    }

    fn query(&self, path: &str, data: &[u8]) -> Result<Vec<u8>> {
        self.inner.query(path, data)
    }
}
