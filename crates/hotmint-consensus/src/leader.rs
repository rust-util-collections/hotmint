use hotmint_types::{ValidatorId, ValidatorSet, ViewNumber};

/// Check if the given validator is the leader for the given view
pub fn is_leader(vs: &ValidatorSet, view: ViewNumber, id: ValidatorId) -> bool {
    vs.leader_for_view(view).id == id
}

/// Get the leader for the next view
pub fn next_leader(vs: &ValidatorSet, view: ViewNumber) -> ValidatorId {
    vs.leader_for_view(view.next()).id
}
