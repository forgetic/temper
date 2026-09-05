const PARALLEL_ACTIVE_PROVIDER_SELECTOR: &str =
    "temper-v1-production.src.route.worker_slot";
const PARALLEL_FIRST_PROVIDER_SELECTOR: &str =
    "temper-v1-production.src.model.affinity_topic";

#[test]
fn same_batch_candidate_selection_is_transactional_when_loser_finishes_first() {
    run_parallel_overlapping_roots_execute_same_batch_candidate_selection(false);
}

#[test]
fn same_batch_candidate_selection_is_transactional_when_winner_finishes_first_and_order_reverses() {
    run_parallel_overlapping_roots_execute_same_batch_candidate_selection(true);
}
