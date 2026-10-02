//! The referee of the work hub's world, fed observations by hand as the world
//! would feed them, fails a run that breaks an expectation, and says why.

use temper_engine_model_work::{Failures, Hold, Item, Lifecycle, Phase};
use temper_engine_model_work_tests::referee::{Key, Seen, Stimulus, Work};
use temper_lib::{Duration, Time};
use temper_world::{Referee, Verdict};

const ITEM: Item = Item { repository: 0, number: 7 };

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn recorded(phase: Phase, attempts: u64) -> Seen {
    Seen::Recorded { item: ITEM, lifecycle: Lifecycle { phase, attempts, failures: Failures::NONE } }
}

fn why(referee: &Referee<Work>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else { panic!("the referee failed the run") };
    failure.why
}

/// A referee that has seen the item taken in and its first run claimed.
fn claimed() -> Referee<Work> {
    let mut referee = Referee::new(Work::new(Duration::from_secs(600)));
    referee.observe(at(0), Seen::Taken { item: ITEM }, &mut Vec::new());
    referee.observe(at(1), recorded(Phase::Waiting, 0), &mut Vec::new());
    referee.observe(at(2), Seen::Asked { item: ITEM }, &mut Vec::new());
    referee.observe(at(3), recorded(Phase::Claimed, 1), &mut Vec::new());
    referee
}

#[test]
fn a_run_that_starts_before_its_claim_is_written_fails_the_run() {
    let mut referee = claimed();
    referee.observe(at(4), Seen::Started { item: ITEM, attempt: 2 }, &mut Vec::new());
    let why = why(&referee);
    assert!(why.starts_with("a run starts only after its claim is written"), "{why}");
}

#[test]
fn a_second_run_of_an_item_fails_the_run() {
    let mut referee = claimed();
    referee.observe(at(4), Seen::Started { item: ITEM, attempt: 1 }, &mut Vec::new());
    referee.observe(at(5), recorded(Phase::Claimed, 2), &mut Vec::new());
    referee.observe(at(6), Seen::Started { item: ITEM, attempt: 2 }, &mut Vec::new());
    assert_eq!(why(&referee), "one run per item: Item { repository: 0, number: 7 } 2 while Some(1) runs");
}

#[test]
fn an_attempt_taken_again_after_a_restart_fails_the_run() {
    let mut referee = claimed();
    referee.observe(at(4), Seen::Started { item: ITEM, attempt: 1 }, &mut Vec::new());
    referee.observe(at(5), Seen::Finished { item: ITEM, attempt: 1 }, &mut Vec::new());
    referee.observe(at(6), Seen::Restarted, &mut Vec::new());
    // A record that went back to an attempt before its last.
    referee.observe(at(7), recorded(Phase::Claimed, 0), &mut Vec::new());
    let why = why(&referee);
    assert!(why.starts_with("attempts only grow"), "{why}");
}

#[test]
fn a_keyed_creation_made_twice_fails_the_run() {
    let mut referee = claimed();
    let key = Key::Write { item: ITEM, attempt: 1, index: 0 };
    referee.observe(at(4), Seen::Created { key }, &mut Vec::new());
    referee.observe(at(5), Seen::Created { key }, &mut Vec::new());
    let why = why(&referee);
    assert!(why.starts_with("an outcome is applied at most once"), "{why}");
}

#[test]
fn asking_what_is_due_for_a_held_item_fails_the_run_until_it_is_released() {
    let mut referee = claimed();
    referee.observe(at(4), recorded(Phase::Held(Hold::Stopped), 1), &mut Vec::new());
    referee.observe(at(5), Seen::Released { item: ITEM }, &mut Vec::new());
    referee.observe(at(6), recorded(Phase::Waiting, 1), &mut Vec::new());
    referee.observe(at(7), Seen::Asked { item: ITEM }, &mut Vec::new());
    let Verdict::Open { .. } = referee.verdict() else { panic!("released, it may be due: {:?}", referee.verdict()) };
    referee.observe(at(8), Seen::Refused { item: ITEM }, &mut Vec::new());
    referee.observe(at(9), Seen::Asked { item: ITEM }, &mut Vec::new());
    assert_eq!(why(&referee), "a held item is never due until released: Item { repository: 0, number: 7 }");
}

#[test]
fn a_record_that_says_done_before_its_item_is_closed_fails_the_run() {
    let mut referee = claimed();
    referee.observe(at(4), recorded(Phase::Done, 1), &mut Vec::new());
    let why = why(&referee);
    assert!(why.starts_with("the record's update comes last"), "{why}");
}

#[test]
fn an_item_that_ends_passes_and_one_that_does_not_fails_once_its_time_is_up() {
    let mut referee = claimed();
    referee.observe(at(4), Seen::Closed { item: ITEM }, &mut Vec::new());
    referee.observe(at(5), recorded(Phase::Done, 1), &mut Vec::new());
    assert_eq!(referee.verdict(), Verdict::Passed);
    let mut referee = claimed();
    referee.fire(at(600), &mut Vec::new());
    assert_eq!(why(&referee), "End(Item { repository: 0, number: 7 }) was not met by 600.000000000s");
}

#[test]
fn a_restart_is_injected_at_its_moment_and_forgets_holds_the_records_do_not_show() {
    let mut referee = claimed();
    referee.observe(at(4), Seen::Refused { item: ITEM }, &mut Vec::new());
    assert_eq!(referee.verdict(), Verdict::Passed, "held since its write was refused");
    referee.inject(at(10), Stimulus::Restart);
    let mut stimuli = Vec::new();
    referee.fire(at(9), &mut stimuli);
    assert!(stimuli.is_empty());
    referee.fire(at(10), &mut stimuli);
    assert_eq!(stimuli, [Stimulus::Restart]);
    referee.observe(at(10), Seen::Restarted, &mut Vec::new());
    // The item goes on from its record, claimed: it is to end again.
    referee.observe(at(11), Seen::Taken { item: ITEM }, &mut Vec::new());
    let Verdict::Open { .. } = referee.verdict() else { panic!("it is to end: {:?}", referee.verdict()) };
}
