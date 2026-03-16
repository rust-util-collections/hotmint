use hotmint_types::{BlockHash, Epoch, Height, QuorumCertificate, ViewNumber};
use serde::{Deserialize, Serialize};
use vsdb::MapxOrd;

/// Key constants for the consensus state KV store
const KEY_CURRENT_VIEW: u64 = 1;
const KEY_LOCKED_QC: u64 = 2;
const KEY_HIGHEST_QC: u64 = 3;
const KEY_LAST_COMMITTED_HEIGHT: u64 = 4;
const KEY_CURRENT_EPOCH: u64 = 5;
const KEY_LAST_APP_HASH: u64 = 6;

/// Persisted consensus state fields (serialized as a single blob per key)
#[derive(Debug, Clone, Serialize, Deserialize)]
enum StateValue {
    View(ViewNumber),
    Height(Height),
    Qc(QuorumCertificate),
    Epoch(Epoch),
    AppHash(BlockHash),
}

/// Persistent consensus state store backed by vsdb
pub struct PersistentConsensusState {
    store: MapxOrd<u64, StateValue>,
}

impl PersistentConsensusState {
    pub fn new() -> Self {
        Self {
            store: MapxOrd::new(),
        }
    }

    pub fn save_current_view(&mut self, view: ViewNumber) {
        self.store
            .insert(&KEY_CURRENT_VIEW, &StateValue::View(view));
    }

    pub fn load_current_view(&self) -> Option<ViewNumber> {
        self.store.get(&KEY_CURRENT_VIEW).and_then(|v| match v {
            StateValue::View(view) => Some(view),
            _ => None,
        })
    }

    pub fn save_locked_qc(&mut self, qc: &QuorumCertificate) {
        self.store
            .insert(&KEY_LOCKED_QC, &StateValue::Qc(qc.clone()));
    }

    pub fn load_locked_qc(&self) -> Option<QuorumCertificate> {
        self.store.get(&KEY_LOCKED_QC).and_then(|v| match v {
            StateValue::Qc(qc) => Some(qc),
            _ => None,
        })
    }

    pub fn save_highest_qc(&mut self, qc: &QuorumCertificate) {
        self.store
            .insert(&KEY_HIGHEST_QC, &StateValue::Qc(qc.clone()));
    }

    pub fn load_highest_qc(&self) -> Option<QuorumCertificate> {
        self.store.get(&KEY_HIGHEST_QC).and_then(|v| match v {
            StateValue::Qc(qc) => Some(qc),
            _ => None,
        })
    }

    pub fn save_last_committed_height(&mut self, height: Height) {
        self.store
            .insert(&KEY_LAST_COMMITTED_HEIGHT, &StateValue::Height(height));
    }

    pub fn load_last_committed_height(&self) -> Option<Height> {
        self.store
            .get(&KEY_LAST_COMMITTED_HEIGHT)
            .and_then(|v| match v {
                StateValue::Height(h) => Some(h),
                _ => None,
            })
    }

    pub fn save_current_epoch(&mut self, epoch: &Epoch) {
        self.store
            .insert(&KEY_CURRENT_EPOCH, &StateValue::Epoch(epoch.clone()));
    }

    pub fn load_current_epoch(&self) -> Option<Epoch> {
        self.store.get(&KEY_CURRENT_EPOCH).and_then(|v| match v {
            StateValue::Epoch(e) => Some(e),
            _ => None,
        })
    }

    pub fn save_last_app_hash(&mut self, hash: BlockHash) {
        self.store
            .insert(&KEY_LAST_APP_HASH, &StateValue::AppHash(hash));
    }

    pub fn load_last_app_hash(&self) -> Option<BlockHash> {
        self.store.get(&KEY_LAST_APP_HASH).and_then(|v| match v {
            StateValue::AppHash(h) => Some(h),
            _ => None,
        })
    }

    pub fn flush(&self) {
        vsdb::vsdb_flush();
    }
}

impl hotmint_consensus::engine::StatePersistence for PersistentConsensusState {
    fn save_current_view(&mut self, view: ViewNumber) {
        self.save_current_view(view);
    }
    fn save_locked_qc(&mut self, qc: &QuorumCertificate) {
        self.save_locked_qc(qc);
    }
    fn save_highest_qc(&mut self, qc: &QuorumCertificate) {
        self.save_highest_qc(qc);
    }
    fn save_last_committed_height(&mut self, height: Height) {
        self.save_last_committed_height(height);
    }
    fn save_current_epoch(&mut self, epoch: &Epoch) {
        self.save_current_epoch(epoch);
    }
    fn save_last_app_hash(&mut self, hash: BlockHash) {
        self.save_last_app_hash(hash);
    }
    fn flush(&self) {
        self.flush();
    }
}

impl Default for PersistentConsensusState {
    fn default() -> Self {
        Self::new()
    }
}
