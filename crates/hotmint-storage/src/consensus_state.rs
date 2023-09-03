use hotmint_types::{Height, QuorumCertificate, ViewNumber};
use serde::{Deserialize, Serialize};
use vsdb::MapxOrd;

/// Key constants for the consensus state KV store
const KEY_CURRENT_VIEW: u64 = 1;
const KEY_LOCKED_QC: u64 = 2;
const KEY_HIGHEST_QC: u64 = 3;
const KEY_LAST_COMMITTED_HEIGHT: u64 = 4;

/// Persisted consensus state fields (serialized as a single blob per key)
#[derive(Debug, Clone, Serialize, Deserialize)]
enum StateValue {
    View(ViewNumber),
    Height(Height),
    Qc(QuorumCertificate),
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

    pub fn flush(&self) {
        vsdb::vsdb_flush();
    }
}

impl Default for PersistentConsensusState {
    fn default() -> Self {
        Self::new()
    }
}
