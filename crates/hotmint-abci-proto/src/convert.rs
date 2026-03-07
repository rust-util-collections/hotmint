use hotmint_types::block::{Block, BlockHash, Height};
use hotmint_types::context::{OwnedBlockContext, TxContext};
use hotmint_types::crypto::{PublicKey, Signature};
use hotmint_types::epoch::EpochNumber;
use hotmint_types::evidence::EquivocationProof;
use hotmint_types::validator::{ValidatorId, ValidatorInfo, ValidatorSet};
use hotmint_types::validator_update::{EndBlockResponse, Event, EventAttribute, ValidatorUpdate};
use hotmint_types::view::ViewNumber;
use hotmint_types::vote::VoteType;

use crate::pb;

// ---- Block ----

impl From<&Block> for pb::Block {
    fn from(b: &Block) -> Self {
        Self {
            height: b.height.0,
            parent_hash: b.parent_hash.0.to_vec(),
            view: b.view.0,
            proposer: b.proposer.0,
            payload: b.payload.clone(),
            hash: b.hash.0.to_vec(),
        }
    }
}

impl From<Block> for pb::Block {
    fn from(b: Block) -> Self {
        Self {
            height: b.height.0,
            parent_hash: b.parent_hash.0.to_vec(),
            view: b.view.0,
            proposer: b.proposer.0,
            payload: b.payload,
            hash: b.hash.0.to_vec(),
        }
    }
}

impl From<pb::Block> for Block {
    fn from(b: pb::Block) -> Self {
        Self {
            height: Height(b.height),
            parent_hash: bytes_to_hash(&b.parent_hash),
            view: ViewNumber(b.view),
            proposer: ValidatorId(b.proposer),
            payload: b.payload,
            hash: bytes_to_hash(&b.hash),
        }
    }
}

// ---- TxContext ----

impl From<&TxContext> for pb::TxContext {
    fn from(c: &TxContext) -> Self {
        Self {
            height: c.height.0,
            epoch: c.epoch.0,
        }
    }
}

impl From<pb::TxContext> for TxContext {
    fn from(c: pb::TxContext) -> Self {
        Self {
            height: Height(c.height),
            epoch: EpochNumber(c.epoch),
        }
    }
}

// ---- ValidatorInfo ----

impl From<&ValidatorInfo> for pb::ValidatorInfo {
    fn from(v: &ValidatorInfo) -> Self {
        Self {
            id: v.id.0,
            public_key: v.public_key.0.clone(),
            power: v.power,
        }
    }
}

impl From<pb::ValidatorInfo> for ValidatorInfo {
    fn from(v: pb::ValidatorInfo) -> Self {
        Self {
            id: ValidatorId(v.id),
            public_key: PublicKey(v.public_key),
            power: v.power,
        }
    }
}

// ---- ValidatorSet ----

impl From<&ValidatorSet> for pb::ValidatorSet {
    fn from(vs: &ValidatorSet) -> Self {
        Self {
            validators: vs
                .validators()
                .iter()
                .map(pb::ValidatorInfo::from)
                .collect(),
            total_power: vs.total_power(),
        }
    }
}

impl From<pb::ValidatorSet> for ValidatorSet {
    fn from(vs: pb::ValidatorSet) -> Self {
        let infos: Vec<ValidatorInfo> = vs.validators.into_iter().map(Into::into).collect();
        ValidatorSet::new(infos)
    }
}

// ---- OwnedBlockContext ----

impl From<&OwnedBlockContext> for pb::BlockContext {
    fn from(c: &OwnedBlockContext) -> Self {
        Self {
            height: c.height.0,
            view: c.view.0,
            proposer: c.proposer.0,
            epoch: c.epoch.0,
            epoch_start_view: c.epoch_start_view.0,
            validator_set: Some(pb::ValidatorSet::from(&c.validator_set)),
        }
    }
}

impl From<OwnedBlockContext> for pb::BlockContext {
    fn from(c: OwnedBlockContext) -> Self {
        pb::BlockContext::from(&c)
    }
}

impl From<pb::BlockContext> for OwnedBlockContext {
    fn from(c: pb::BlockContext) -> Self {
        Self {
            height: Height(c.height),
            view: ViewNumber(c.view),
            proposer: ValidatorId(c.proposer),
            epoch: EpochNumber(c.epoch),
            epoch_start_view: ViewNumber(c.epoch_start_view),
            validator_set: c
                .validator_set
                .map(ValidatorSet::from)
                .unwrap_or_else(|| ValidatorSet::new(vec![])),
        }
    }
}

// ---- EquivocationProof ----

impl From<&EquivocationProof> for pb::EquivocationProof {
    fn from(e: &EquivocationProof) -> Self {
        Self {
            validator: e.validator.0,
            view: e.view.0,
            vote_type: match e.vote_type {
                VoteType::Vote => 0,
                VoteType::Vote2 => 1,
            },
            block_hash_a: e.block_hash_a.0.to_vec(),
            signature_a: e.signature_a.0.clone(),
            block_hash_b: e.block_hash_b.0.to_vec(),
            signature_b: e.signature_b.0.clone(),
        }
    }
}

impl From<EquivocationProof> for pb::EquivocationProof {
    fn from(e: EquivocationProof) -> Self {
        pb::EquivocationProof::from(&e)
    }
}

impl From<pb::EquivocationProof> for EquivocationProof {
    fn from(e: pb::EquivocationProof) -> Self {
        Self {
            validator: ValidatorId(e.validator),
            view: ViewNumber(e.view),
            vote_type: if e.vote_type == 1 {
                VoteType::Vote2
            } else {
                VoteType::Vote
            },
            block_hash_a: bytes_to_hash(&e.block_hash_a),
            signature_a: Signature(e.signature_a),
            block_hash_b: bytes_to_hash(&e.block_hash_b),
            signature_b: Signature(e.signature_b),
        }
    }
}

// ---- ValidatorUpdate ----

impl From<&ValidatorUpdate> for pb::ValidatorUpdate {
    fn from(u: &ValidatorUpdate) -> Self {
        Self {
            id: u.id.0,
            public_key: u.public_key.0.clone(),
            power: u.power,
        }
    }
}

impl From<pb::ValidatorUpdate> for ValidatorUpdate {
    fn from(u: pb::ValidatorUpdate) -> Self {
        Self {
            id: ValidatorId(u.id),
            public_key: PublicKey(u.public_key),
            power: u.power,
        }
    }
}

// ---- EventAttribute ----

impl From<&EventAttribute> for pb::EventAttribute {
    fn from(a: &EventAttribute) -> Self {
        Self {
            key: a.key.clone(),
            value: a.value.clone(),
        }
    }
}

impl From<pb::EventAttribute> for EventAttribute {
    fn from(a: pb::EventAttribute) -> Self {
        Self {
            key: a.key,
            value: a.value,
        }
    }
}

// ---- Event ----

impl From<&Event> for pb::Event {
    fn from(e: &Event) -> Self {
        Self {
            r#type: e.r#type.clone(),
            attributes: e.attributes.iter().map(pb::EventAttribute::from).collect(),
        }
    }
}

impl From<pb::Event> for Event {
    fn from(e: pb::Event) -> Self {
        Self {
            r#type: e.r#type,
            attributes: e.attributes.into_iter().map(Into::into).collect(),
        }
    }
}

// ---- EndBlockResponse ----

impl From<&EndBlockResponse> for pb::EndBlockResponse {
    fn from(r: &EndBlockResponse) -> Self {
        Self {
            validator_updates: r
                .validator_updates
                .iter()
                .map(pb::ValidatorUpdate::from)
                .collect(),
            events: r.events.iter().map(pb::Event::from).collect(),
        }
    }
}

impl From<EndBlockResponse> for pb::EndBlockResponse {
    fn from(r: EndBlockResponse) -> Self {
        pb::EndBlockResponse::from(&r)
    }
}

impl From<pb::EndBlockResponse> for EndBlockResponse {
    fn from(r: pb::EndBlockResponse) -> Self {
        Self {
            validator_updates: r.validator_updates.into_iter().map(Into::into).collect(),
            events: r.events.into_iter().map(Into::into).collect(),
        }
    }
}

// ---- Helpers ----

fn bytes_to_hash(bytes: &[u8]) -> BlockHash {
    let mut hash = [0u8; 32];
    let len = bytes.len().min(32);
    hash[..len].copy_from_slice(&bytes[..len]);
    BlockHash(hash)
}
