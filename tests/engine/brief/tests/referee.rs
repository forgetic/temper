//! The referee of the brief's world, fed observations by hand as the world
//! would feed them, fails a run that breaks an expectation, and says why.

use skein_lib::{Duration, Time};
use temper_engine_brief_world::LIMITS;
use temper_engine_brief_world::referee::{Briefs, Seen, Served};
use temper_engine_domain_brief::{Body, Fit, Item, Keep, Kind, Part, Refusal, Section, Source, Unread};
use temper_world::{Referee, Verdict};

const ITEM: Item = Item { repository: 0, number: 7 };

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn referee() -> Referee<Briefs> {
    Referee::new(Briefs::new(LIMITS, Duration::from_millis(1)))
}

fn see(referee: &mut Referee<Briefs>, secs: u64, seen: Seen) {
    referee.observe(at(secs), seen, &mut Vec::new());
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

fn comments() -> Source {
    Source::Comments { item: ITEM, since: 3 }
}

/// A referee that has seen brief 1 taken in, of an item and its comments,
/// the comments required, and its two reads asked for.
fn asked() -> Referee<Briefs> {
    let mut referee = referee();
    let sections = vec![(Source::Item(ITEM), false), (comments(), true)];
    see(&mut referee, 0, Seen::Asked { brief: 1, sections });
    let item = Source::Item(ITEM);
    see(
        &mut referee,
        0,
        Seen::Read { brief: 1, index: 0, source: item, keep: Keep::Start, fit: Fit::Run, parts: 6, bytes: 160 },
    );
    let source = comments();
    see(
        &mut referee,
        0,
        Seen::Read { brief: 1, index: 1, source, keep: Keep::End, fit: Fit::Run, parts: 6, bytes: 240 },
    );
    see(&mut referee, 0, Seen::Stepped);
    referee
}

/// Brief 1 with its item's read served `parts`, and its comments' "hi".
fn served(parts: Vec<Part>) -> Referee<Briefs> {
    let mut referee = asked();
    see(&mut referee, 1, Seen::Served { brief: 1, index: 0, read: Served::Content(parts) });
    see(&mut referee, 1, Seen::Stepped);
    see(&mut referee, 1, Seen::Served { brief: 1, index: 1, read: Served::Content(vec![part(b"hi", 0)]) });
    referee
}

fn rendered(referee: &mut Referee<Briefs>, item: Section, comments: Section) {
    see(referee, 1, Seen::Rendered { brief: 1, sections: vec![item, comments] });
    see(referee, 1, Seen::Stepped);
}

#[test]
fn a_brief_rendered_as_it_should_be_passes() {
    let mut referee = served(vec![part(b"the item\n", 0), part(b"its parent, long", 9)]);
    let item = text(Kind::Item, b"the item\nits parent, long\n[9 bytes cut]\n");
    rendered(&mut referee, item, text(Kind::Comments, b"hi"));
    assert_eq!(referee.verdict(), Verdict::Passed);
    assert_eq!(referee.expectations().cuts, 1);
}

#[test]
fn a_cut_line_with_the_wrong_count_fails_the_run() {
    let mut referee = served(vec![part(b"the item\n", 0), part(b"its parent, long", 9)]);
    rendered(
        &mut referee,
        text(Kind::Item, b"the item\nits parent, long\n[8 bytes cut]\n"),
        text(Kind::Comments, b"hi"),
    );
    assert_eq!(why(&referee), "brief 1: section 0 of Item is its content, cut as told");
}

#[test]
fn a_silent_cut_fails_the_run() {
    let mut referee = served(vec![part(b"the item\n", 0), part(b"its parent, long", 0)]);
    rendered(&mut referee, text(Kind::Item, b"the item\nits parent"), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 of Item is its content, cut as told");
}

#[test]
fn cut_lines_next_to_each_other_fail_the_run() {
    let mut referee = served(vec![part(b"the item\n", 5), part(b"", 4)]);
    let item = text(Kind::Item, b"the item\n\n[5 bytes cut]\n\n[4 bytes cut]\n");
    rendered(&mut referee, item, text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 of Item is its content, cut as told");
}

#[test]
fn content_changed_around_a_cut_fails_the_run() {
    let mut referee = served(vec![part(b"the item, long\n", 0)]);
    rendered(&mut referee, text(Kind::Item, b"the ITEM\n\n[7 bytes cut]\n"), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 of Item is its content, cut as told");
}

#[test]
fn a_section_cut_that_fits_its_share_fails_the_run() {
    let mut referee = served(vec![part(b"the item, not long\n", 0)]);
    rendered(&mut referee, text(Kind::Item, b"the item\n[11 bytes cut]\n"), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0, within its share 160, is whole");
}

#[test]
fn a_section_over_its_budget_fails_the_run() {
    let mut whole = vec![b'a'; 150];
    let mut referee = served(vec![part(&whole, 5)]);
    whole.extend_from_slice(b"\n[5 bytes cut]\n");
    rendered(&mut referee, text(Kind::Item, &whole), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 is within its budget: 165 > 160");
}

#[test]
fn a_section_cut_far_short_of_its_budget_fails_the_run() {
    let mut referee = served(vec![part(&[b'a'; 150], 200)]);
    let mut cut = vec![b'a'; 10];
    cut.extend_from_slice(b"\n[340 bytes cut]\n");
    rendered(&mut referee, text(Kind::Item, &cut), text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 of Item, cut, is near its share: 27 of 160");
}

#[test]
fn a_required_section_missing_from_a_rendered_brief_fails_the_run() {
    let mut referee = asked();
    see(&mut referee, 1, Seen::Served { brief: 1, index: 0, read: Served::Content(vec![part(b"item", 0)]) });
    see(&mut referee, 1, Seen::Served { brief: 1, index: 1, read: Served::Failed });
    let missing = Section { kind: Kind::Comments, body: Body::Missing(Unread::Failed) };
    rendered(&mut referee, text(Kind::Item, b"item"), missing);
    assert_eq!(why(&referee), "brief 1: a required section is never missing");
}

#[test]
fn a_section_read_in_time_but_missing_fails_the_run() {
    let mut referee = served(vec![part(b"item", 0)]);
    let missing = Section { kind: Kind::Item, body: Body::Missing(Unread::Late) };
    rendered(&mut referee, missing, text(Kind::Comments, b"hi"));
    assert_eq!(why(&referee), "brief 1: section 0 was read in time but is missing");
}

#[test]
fn sections_out_of_order_fail_the_run() {
    let mut referee = served(vec![part(b"item", 0)]);
    rendered(&mut referee, text(Kind::Comments, b"hi"), text(Kind::Item, b"item"));
    assert_eq!(why(&referee), "brief 1: a section for each asked, in order");
}

#[test]
fn a_brief_that_fails_with_its_required_sections_read_fails_the_run() {
    let mut referee = served(vec![part(b"item", 0)]);
    see(&mut referee, 1, Seen::Failed { brief: 1, missing: Kind::Comments, why: Unread::Failed });
    assert_eq!(why(&referee), "brief 1: it fails only for want of a required section of Comments, Failed");
}

#[test]
fn a_brief_not_answered_once_every_read_has_ended_fails_the_run() {
    let mut referee = served(vec![part(b"item", 0)]);
    see(&mut referee, 1, Seen::Stepped);
    assert_eq!(why(&referee), "brief 1: answered as soon as it is due");
}

#[test]
fn a_read_not_as_its_section_is_cut_fails_the_run() {
    let mut referee = referee();
    see(&mut referee, 0, Seen::Asked { brief: 1, sections: vec![(Source::Item(ITEM), false)] });
    let item = Source::Item(ITEM);
    see(
        &mut referee,
        0,
        Seen::Read { brief: 1, index: 0, source: item, keep: Keep::Start, fit: Fit::Run, parts: 6, bytes: 600 },
    );
    assert_eq!(why(&referee), "brief 1: section 0 of Item is read as it is cut: Start Run 6 600");
}

#[test]
fn a_brief_within_the_limits_refused_fails_the_run() {
    let mut referee = asked();
    see(&mut referee, 1, Seen::Asked { brief: 2, sections: vec![(Source::Item(ITEM), false)] });
    see(&mut referee, 1, Seen::Refused { brief: 2, refusal: Refusal::Busy });
    assert_eq!(why(&referee), "brief 2: refused as Busy only when it is due: Admitted");
}

#[test]
fn room_not_told_when_it_is_made_fails_the_run() {
    let mut referee = referee();
    for brief in 1..=3 {
        see(&mut referee, 0, Seen::Asked { brief, sections: vec![(Source::Item(ITEM), false)] });
        let item = Source::Item(ITEM);
        see(
            &mut referee,
            0,
            Seen::Read { brief, index: 0, source: item, keep: Keep::Start, fit: Fit::Run, parts: 6, bytes: 160 },
        );
        see(&mut referee, 0, Seen::Stepped);
    }
    see(&mut referee, 0, Seen::Asked { brief: 4, sections: vec![(Source::Item(ITEM), false)] });
    see(&mut referee, 0, Seen::Refused { brief: 4, refusal: Refusal::Busy });
    see(&mut referee, 0, Seen::Stepped);
    assert_eq!(
        referee.verdict(),
        Verdict::Open {
            pending: vec![
                "Answer(1) by 20.001000000s".to_owned(),
                "Answer(2) by 20.001000000s".to_owned(),
                "Answer(3) by 20.001000000s".to_owned(),
            ]
        }
    );
    see(&mut referee, 1, Seen::Served { brief: 1, index: 0, read: Served::Failed });
    see(
        &mut referee,
        1,
        Seen::Rendered { brief: 1, sections: vec![Section { kind: Kind::Item, body: Body::Missing(Unread::Failed) }] },
    );
    see(&mut referee, 1, Seen::Stepped);
    assert_eq!(why(&referee), "a refusal is told of room in the step that makes it");
}

#[test]
fn a_brief_left_unanswered_fails_the_run_once_its_time_is_up() {
    let mut referee = asked();
    referee.fire(at(21), &mut Vec::new());
    assert_eq!(why(&referee), "Answer(1) was not met by 20.001000000s");
}
