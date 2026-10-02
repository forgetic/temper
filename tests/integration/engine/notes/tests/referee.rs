//! The referee of the notes' world, fed observations by hand as the world
//! would feed them, fails a run that breaks an expectation, and says why.

use temper_engine_model_notes::{Author, Entry, Page, Scope};
use temper_engine_model_notes_tests::referee::{Notes, Seen};
use temper_lib::{Duration, Time};
use temper_world::{Referee, Verdict};

const REPO: Scope = Scope::Repository(0);

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn page(description: &[u8]) -> Page {
    Page { description: description.into(), author: Author::Person(1), references: Box::new([]), body: Box::new([]) }
}

fn changed(name: &[u8], page: Option<Page>) -> Seen {
    Seen::Changed { scope: REPO, name: name.to_vec(), page }
}

fn lines(name: &[u8]) -> Seen {
    Seen::Lines { lines: vec![(REPO, name.to_vec())] }
}

fn why(referee: &Referee<Notes>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else { panic!("the referee failed the run") };
    failure.why
}

/// A referee that has seen the page `flaky` made, then deleted, and the
/// repository's scope listed after that.
fn deleted_then_listed() -> Referee<Notes> {
    let mut referee = Referee::new(Notes::new(Duration::from_secs(60)));
    referee.observe(at(0), changed(b"flaky", Some(page(b"flaky test"))));
    referee.observe(at(1), changed(b"flaky", None));
    referee.observe(at(2), Seen::Served { listing: 1, scope: REPO });
    referee.observe(at(3), Seen::TakenIn { listing: 1 });
    referee
}

#[test]
fn a_line_for_a_page_deleted_before_its_scope_was_listed_fails_the_run() {
    let mut referee = deleted_then_listed();
    referee.observe(at(4), lines(b"flaky"));
    assert_eq!(why(&referee), "no line names a page deleted before its scope was last read: Repository(0) flaky");
}

#[test]
fn a_line_for_a_page_made_again_since_or_deleted_after_the_listing_passes() {
    let mut referee = deleted_then_listed();
    referee.observe(at(4), changed(b"flaky", Some(page(b"again"))));
    referee.observe(at(5), lines(b"flaky"));
    referee.observe(at(6), changed(b"build", Some(page(b"build"))));
    referee.observe(at(7), Seen::Served { listing: 2, scope: REPO });
    referee.observe(at(8), changed(b"build", None));
    // The listing is taken in after the page was deleted: it was there when
    // it was served.
    referee.observe(at(9), Seen::TakenIn { listing: 2 });
    referee.observe(at(10), lines(b"build"));
    assert_eq!(referee.verdict(), Verdict::Passed);
}

#[test]
fn a_recall_answered_with_content_older_than_its_asking_fails_the_run() {
    let mut referee = Referee::new(Notes::new(Duration::from_secs(60)));
    referee.observe(at(0), changed(b"flaky", Some(page(b"old"))));
    referee.observe(at(1), changed(b"flaky", Some(page(b"new"))));
    referee.observe(at(2), Seen::Asked { call: 7 });
    let stale = Entry { scope: REPO, name: b"flaky".as_slice().into(), page: page(b"old") };
    referee.observe(at(3), Seen::Recalled { call: 7, entries: vec![stale] });
    assert_eq!(why(&referee), "a recall answers with the page as it was meanwhile: Repository(0) flaky");
}

#[test]
fn a_recall_answered_with_content_from_meanwhile_passes() {
    let mut referee = Referee::new(Notes::new(Duration::from_secs(60)));
    referee.observe(at(0), changed(b"flaky", Some(page(b"old"))));
    referee.observe(at(1), Seen::Asked { call: 7 });
    referee.observe(at(2), changed(b"flaky", Some(page(b"new"))));
    let entries = vec![Entry { scope: REPO, name: b"flaky".as_slice().into(), page: page(b"old") }];
    referee.observe(at(3), Seen::Recalled { call: 7, entries });
    assert_eq!(referee.verdict(), Verdict::Passed, "it was so when the recall was asked");
}

#[test]
fn a_recall_left_unanswered_fails_the_run_once_its_time_is_up() {
    let mut referee = Referee::new(Notes::new(Duration::from_secs(60)));
    referee.observe(at(1), Seen::Asked { call: 7 });
    referee.fire(at(61), &mut Vec::new());
    assert_eq!(why(&referee), "Recall(7) was not met by 61.000000000s");
}
