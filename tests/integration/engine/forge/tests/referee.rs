//! The referee of the forge's world, fed observations by hand as the world
//! would feed them, fails a run that breaks an expectation, and says why.

use temper_engine_model_forge::Item;
use temper_engine_model_forge_tests::referee::{Forge, Planned, Seen};
use temper_engine_model_forge_tests::{ENGINE, REPOSITORIES, TRACKING, translate};
use temper_forge_model::Observation;
use temper_forge_model::api::Kind;
use temper_lib::{Duration, Time};
use temper_world::{Referee, Verdict};

const PERSON: u64 = 10;
const ITEM: Item = Item { repository: 0, number: 5 };

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn referee() -> Referee<Forge> {
    Referee::new(Forge::new(Duration::from_secs(60)))
}

fn why(referee: &Referee<Forge>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else {
        panic!("the referee failed the run: {:?}", referee.verdict())
    };
    failure.why
}

fn opened(key: &[u8], by: u64) -> Seen {
    Seen::Forge(Observation::Opened {
        repository: REPOSITORIES[0].into(),
        number: 7,
        kind: Kind::Issue,
        title: b"a task".as_slice().into(),
        body: translate::keyed(key, b"what").into_boxed_slice(),
        labels: Box::new([]),
        branches: None,
        by,
    })
}

fn commented(id: u64, body: &[u8], by: u64) -> Seen {
    Seen::Forge(Observation::Commented {
        repository: REPOSITORIES[0].into(),
        number: ITEM.number,
        id,
        body: body.into(),
        by,
    })
}

fn labelled(labels: &[&[u8]], by: u64) -> Seen {
    let labels = labels.iter().map(|label| (*label).into()).collect();
    Seen::Forge(Observation::Labelled { repository: REPOSITORIES[0].into(), number: ITEM.number, labels, by })
}

fn set(plan: u64, labels: &[&[u8]]) -> Seen {
    let labels = labels.iter().map(|label| label.to_vec()).collect();
    Seen::Planned { plan, write: Planned::SetLabels { item: ITEM, labels } }
}

fn announced() -> Seen {
    Seen::Announced { item: ITEM, labels: vec![TRACKING.to_vec()] }
}

#[test]
fn a_write_the_parent_did_not_plan_fails_the_run() {
    let mut referee = referee();
    referee.observe(at(1), commented(3, &translate::keyed(b"reply", b"hi"), ENGINE));
    assert_eq!(why(&referee), "a comment the parent planned: Item { repository: 0, number: 5 }");
}

#[test]
fn a_creation_made_twice_for_one_key_fails_the_run() {
    let mut referee = referee();
    referee.observe(
        at(0),
        Seen::Planned { plan: 1, write: Planned::CreateIssue { repository: 0, key: b"task-1".to_vec() } },
    );
    referee.observe(at(1), opened(b"task-1", ENGINE));
    assert!(matches!(referee.verdict(), Verdict::Passed), "once is the plan");
    referee.observe(at(2), opened(b"task-1", ENGINE));
    assert_eq!(why(&referee), "one issue per key: task-1");
}

#[test]
fn a_second_record_on_an_item_fails_the_run() {
    let mut referee = referee();
    referee.observe(at(0), Seen::Planned { plan: 1, write: Planned::Record { item: ITEM } });
    let record = translate::recorded(temper_engine_model_forge::Position::START, b"the record");
    referee.observe(at(1), commented(3, &record, ENGINE));
    referee.observe(at(2), commented(4, &record, ENGINE));
    assert_eq!(why(&referee), "one record per item: Item { repository: 0, number: 5 }");
}

#[test]
fn labels_landing_out_of_the_order_planned_fail_the_run() {
    let mut referee = referee();
    referee.observe(at(0), set(1, &[TRACKING, b"a"]));
    referee.observe(at(0), set(2, &[TRACKING, b"b"]));
    referee.observe(at(1), labelled(&[TRACKING, b"b"], ENGINE));
    referee.observe(at(2), labelled(&[TRACKING, b"a"], ENGINE));
    assert!(why(&referee).starts_with("labels land in the order planned"), "{:?}", referee.verdict());
}

#[test]
fn an_item_not_ending_with_the_last_set_written_fails_the_run() {
    let mut referee = referee();
    referee.observe(at(0), set(1, &[TRACKING, b"a"]));
    referee.observe(at(0), set(2, &[TRACKING, b"b"]));
    referee.observe(at(1), labelled(&[TRACKING, b"a"], ENGINE));
    referee.observe(at(2), Seen::Wrote { plan: 1, written: true, edited: false });
    referee.observe(at(3), Seen::Wrote { plan: 2, written: true, edited: false });
    referee.observe(at(4), Seen::Settled);
    assert!(why(&referee).contains("ends with the last set written"), "{:?}", referee.verdict());
}

#[test]
fn a_persons_comment_on_a_tracked_item_must_reach_its_inbox_in_time() {
    let mut referee = referee();
    referee.observe(at(0), announced());
    referee.observe(at(1), commented(3, b"hello", PERSON));
    referee.observe(at(2), commented(4, b"again", PERSON));
    referee.observe(at(30), Seen::News { item: ITEM, comment: Some(3) });
    let mut stimuli = Vec::new();
    referee.fire(at(62), &mut stimuli);
    assert_eq!(
        why(&referee),
        "Comment { item: Item { repository: 0, number: 5 }, id: 4 } was not met by 62.000000000s"
    );
}

#[test]
fn a_comment_on_an_item_that_leaves_is_moot_and_a_label_change_must_be_told() {
    let mut referee = referee();
    referee.observe(at(0), announced());
    referee.observe(at(1), commented(3, b"hello", PERSON));
    referee.observe(at(2), Seen::Left { item: ITEM });
    assert!(matches!(referee.verdict(), Verdict::Passed), "{:?}", referee.verdict());
    referee.observe(at(3), announced());
    referee.observe(at(4), labelled(&[TRACKING, b"bug"], PERSON));
    let mut stimuli = Vec::new();
    referee.fire(at(70), &mut stimuli);
    assert!(why(&referee).starts_with("Labels {"), "{:?}", referee.verdict());
}

#[test]
fn a_call_before_the_reset_a_refusal_named_fails_the_run() {
    let mut referee = referee();
    referee.observe(at(1), Seen::Limited { reset: at(10) });
    referee.observe(at(10), Seen::Sent);
    assert!(matches!(referee.verdict(), Verdict::Passed), "at the reset is in time");
    referee.observe(at(11), Seen::Limited { reset: at(20) });
    referee.observe(at(15), Seen::Sent);
    assert!(why(&referee).starts_with("no call goes out before the reset"), "{:?}", referee.verdict());
}

#[test]
fn the_cold_start_after_a_restart_must_end_in_time() {
    let mut referee = referee();
    referee.observe(at(0), Seen::Restarted);
    let mut stimuli = Vec::new();
    referee.fire(at(61), &mut stimuli);
    assert_eq!(why(&referee), "Loaded { restart: 1 } was not met by 60.000000000s");
}

#[test]
fn a_record_held_as_edited_that_nobody_else_edited_fails_the_run() {
    let mut referee = referee();
    referee.observe(at(0), Seen::Planned { plan: 1, write: Planned::Record { item: ITEM } });
    referee.observe(at(1), Seen::Wrote { plan: 1, written: false, edited: true });
    assert_eq!(why(&referee), "a record held as edited was edited by someone else: Item { repository: 0, number: 5 }");
    let mut referee = super_referee();
    referee.observe(at(1), Seen::Wrote { plan: 1, written: false, edited: true });
    assert!(matches!(referee.verdict(), Verdict::Passed), "{:?}", referee.verdict());
}

/// A referee that has seen a person edit the record of `ITEM`, and a record
/// write planned after it.
fn super_referee() -> Referee<Forge> {
    let mut referee = referee();
    let edited = Observation::Edited {
        repository: REPOSITORIES[0].into(),
        number: ITEM.number,
        id: 3,
        body: b"mine".as_slice().into(),
        by: PERSON,
    };
    referee.observe(at(0), Seen::Forge(edited));
    referee.observe(at(0), Seen::Planned { plan: 1, write: Planned::Record { item: ITEM } });
    referee
}
