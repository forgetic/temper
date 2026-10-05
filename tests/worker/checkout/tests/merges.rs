use temper_worker_checkout_world::merges::{self, Case};

#[test]
fn merges_conflicts_saving_head_movement_and_ambiguous_pushes_settle() {
    for case in [Case::Clean, Case::Conflict, Case::Unchanged, Case::Moved, Case::Ambiguous, Case::Saved] {
        let _ = merges::run(73, case, 64);
    }
}

#[test]
fn merge_scenarios_replay_and_facts_do_not_change_requests_or_effects() {
    for case in [Case::Clean, Case::Conflict, Case::Unchanged, Case::Moved, Case::Ambiguous, Case::Saved] {
        let expected = merges::run(119, case, 64);
        assert_eq!(merges::run(119, case, 64), expected);
        assert_eq!(merges::run(119, case, 0), expected);
    }
}
