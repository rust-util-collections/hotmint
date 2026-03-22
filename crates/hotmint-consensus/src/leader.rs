use hotmint_types::{ValidatorId, ValidatorSet, ViewNumber};

/// Check if the given validator is the leader for the given view
pub fn is_leader(vs: &ValidatorSet, view: ViewNumber, id: ValidatorId) -> bool {
    vs.leader_for_view(view).is_some_and(|l| l.id == id)
}

/// Get the leader for the next view
pub fn next_leader(vs: &ValidatorSet, view: ViewNumber) -> ValidatorId {
    vs.leader_for_view(view.next())
        .expect("empty validator set in next_leader")
        .id
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotmint_types::crypto::PublicKey;
    use hotmint_types::validator::ValidatorInfo;

    fn make_vs(n: u64) -> ValidatorSet {
        let validators: Vec<ValidatorInfo> = (0..n)
            .map(|i| ValidatorInfo {
                id: ValidatorId(i),
                public_key: PublicKey(vec![i as u8]),
                power: 1,
            })
            .collect();
        ValidatorSet::new(validators)
    }

    #[test]
    fn test_leader_rotation() {
        let vs = make_vs(4);
        assert!(is_leader(&vs, ViewNumber(0), ValidatorId(0)));
        assert!(!is_leader(&vs, ViewNumber(0), ValidatorId(1)));
        assert!(is_leader(&vs, ViewNumber(1), ValidatorId(1)));
        assert!(is_leader(&vs, ViewNumber(3), ValidatorId(3)));
        assert!(is_leader(&vs, ViewNumber(4), ValidatorId(0)));
    }

    #[test]
    fn test_next_leader() {
        let vs = make_vs(4);
        assert_eq!(next_leader(&vs, ViewNumber(0)), ValidatorId(1));
        assert_eq!(next_leader(&vs, ViewNumber(3)), ValidatorId(0));
    }
}
