//! The referee of the brief's world, fed observations by hand as the world
//! would feed them, fails a run that breaks an expectation, and says why.

use temper_engine_model_brief::{Body, Kind, Part, Refusal, Section};
use temper_engine_model_brief_tests::LIMITS;
use temper_engine_model_brief_tests::referee::{Briefs, Seen, Served};
use temper_lib::{Duration, Time};
use temper_world::{Referee, Verdict};

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn referee() -> Referee<Briefs> {
    Referee::new(Briefs::new(LIMITS, Duration::from_secs(20)))
}

fn why(referee: &Referee<Briefs>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else {
        panic!("the referee failed the run: {:?}", referee.verdict())
    };
    failure.why
}

fn part(bytes: &[u8], left: u64) -> Part {
    Part { bytes: bytes.into(), left }
}

fn text(kind: Kind, bytes: &[u8]) -> Section {
    Section { kind, body: Body::Text(bytes.into()) }
}

/// A referee that has seen brief 1 asked for an item and its comments, the
/// comments required, and the item's read serve `parts`.
fn asked(parts: Vec<Part>) -> Referee<Briefs> {
    let mut referee = referee();
    let sections = vec![(Kind::Item, false), (Kind::Comments, true)];
    referee.observe(at(0), Seen::Asked { brief: 1, sections, items: 0 }, &mut Vec::new());
    referee.observe(at(1), Seen::Served { brief: 1, index: 0, read: Served::Content(parts) }, &mut Vec::new());
    referee
}

fn rendered(referee: &mut Referee<Briefs>, item: Section, comments: Section) {
    referee.observe(at(2), Seen::Rendered { brief: 1, sections: vec![item, comments] }, &mut Vec::new());
}

#[test]
fn a_brief_rendered_as_it_should_be_passes() {
    let mut referee = asked(vec![part(b"the item\n", 0), part(b"its parent, long", 9)]);
    referee.observe(
        at(1),
        Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) },
        &mut Vec::new(),
    );
    let item = text(Kind::Item, b"the item\nits\n[22 bytes cut]\n");
    rendered(&mut referee, item, text(Kind::Comments, b"hi"));
    assert_eq!(referee.verdict(), Verdict::Passed);
    assert_eq!(referee.expectations().cuts, 1);
}

#[test]
fn a_cut_line_with_the_wrong_count_fails_the_run() {
    let mut referee = asked(vec![part(b"the item\n", 0), part(b"its parent", 0)]);
    referee.observe(
        at(1),
        Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) },
        &mut Vec::new(),
    );
    rendered(&mut referee, text(Kind::Item, b"the item\n\n[9 bytes cut]\n"), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0's cut lines count what it left out: 9 + 9 != 19");
}

#[test]
fn a_silent_cut_fails_the_run() {
    let mut referee = asked(vec![part(b"the item\n", 0)]);
    referee.observe(
        at(1),
        Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) },
        &mut Vec::new(),
    );
    rendered(&mut referee, text(Kind::Item, b"the item"), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0's cut lines count what it left out: 8 + 0 != 9");
}

#[test]
fn content_changed_around_a_cut_fails_the_run() {
    let mut referee = asked(vec![part(b"the item, long\n", 0)]);
    referee.observe(
        at(1),
        Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) },
        &mut Vec::new(),
    );
    rendered(&mut referee, text(Kind::Item, b"the ITEM\n[7 bytes cut]\n"), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 holds its content unchanged, in order");
}

#[test]
fn a_section_over_its_budget_fails_the_run() {
    let long = vec![b'a'; 161];
    let mut referee = asked(vec![part(&long, 0)]);
    referee.observe(
        at(1),
        Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) },
        &mut Vec::new(),
    );
    rendered(&mut referee, text(Kind::Item, &long), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 is within its budget: 161 > 160");
}

#[test]
fn a_required_section_missing_from_a_rendered_brief_fails_the_run() {
    let mut referee = asked(vec![part(b"item", 0)]);
    referee.observe(at(1), Seen::Served { brief: 1, index: 1, read: Served::Failed }, &mut Vec::new());
    let missing = Section { kind: Kind::Comments, body: Body::Missing };
    rendered(&mut referee, text(Kind::Item, b"item"), missing);
    assert_eq!(why(&referee), "brief 1: a required section is never missing");
}

#[test]
fn a_section_read_in_time_but_missing_fails_the_run() {
    let mut referee = asked(vec![part(b"item", 0)]);
    referee.observe(
        at(1),
        Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) },
        &mut Vec::new(),
    );
    let missing = Section { kind: Kind::Item, body: Body::Missing };
    rendered(&mut referee, missing, text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 was read in time but is missing");
}

#[test]
fn sections_out_of_order_fail_the_run() {
    let mut referee = asked(vec![part(b"item", 0)]);
    referee.observe(
        at(1),
        Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) },
        &mut Vec::new(),
    );
    rendered(&mut referee, text(Kind::Comments, b"hi"), text(Kind::Item, b"item"));
    assert_eq!(why(&referee), "brief 1: a section for each asked, in order");
}

#[test]
fn a_brief_that_fails_with_its_required_sections_read_fails_the_run() {
    let mut referee = asked(vec![part(b"item", 0)]);
    referee.observe(
        at(1),
        Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) },
        &mut Vec::new(),
    );
    referee.observe(at(2), Seen::Failed { brief: 1, missing: Kind::Comments }, &mut Vec::new());
    assert_eq!(why(&referee), "brief 1: it fails only for want of a required section of Comments");
}

#[test]
fn a_brief_within_the_limits_refused_as_oversized_fails_the_run() {
    let mut referee = asked(Vec::new());
    referee.observe(at(2), Seen::Refused { brief: 1, refusal: Refusal::Oversized }, &mut Vec::new());
    assert_eq!(why(&referee), "brief 1: refused as oversized exactly when past the limits: Oversized");
}

#[test]
fn a_brief_left_unanswered_fails_the_run_once_its_time_is_up() {
    let mut referee = asked(Vec::new());
    referee.fire(at(20), &mut Vec::new());
    assert_eq!(why(&referee), "Answer(1) was not met by 20.000000000s");
}
