//! Complete worker stories with real child domains and outside observations.
use temper_worker_domain_world::whole::{Case, run};
#[test]
fn a_prepared_run_delivers_the_complete_tree_and_answers() {
    assert_eq!(run(1, Case::Delivered).cancellations, 0);
}
#[test]
fn cancelling_mid_delivery_waits_for_the_landing_before_answering() {
    assert_eq!(run(2, Case::CancelledDelivery).cancellations, 1);
}
#[test]
fn contact_restored_within_the_grace_keeps_the_agent_running() {
    assert_eq!(run(3, Case::ContactWithin).cancellations, 0);
}
#[test]
fn contact_lost_past_the_grace_saves_before_the_cancellation_answer() {
    assert_eq!(run(4, Case::ContactPast).saves, 1);
}
#[test]
fn the_hello_carries_the_complete_stop_bound_and_stories_replay() {
    for case in [Case::Delivered, Case::CancelledDelivery, Case::ContactWithin, Case::ContactPast] {
        assert_eq!(run(71, case), run(71, case));
    }
}
