//! The referee of the notes' world, fed observations by hand as the world
//! would feed them, fails a run that breaks an expectation, and says why.

use skein_lib::{Duration, Time};
use temper_engine_domain_notes::{Author, Entry, Line, Page, Scope};
use temper_engine_notes_world::referee::{Notes, Seen};
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

/// An answer's line naming `name`, saying `description`.
fn lines(name: &[u8], description: &[u8]) -> Seen {
    let page = page(description);
    let line = Line {
        scope: REPO,
        name: name.into(),
        description: page.description,
        author: page.author,
        references: page.references,
    };
    Seen::Lines { lines: vec![line] }
}

fn entry(name: &[u8], description: &[u8]) -> Entry {
    Entry { scope: REPO, name: name.into(), revision: 1, page: page(description) }
}

fn referee() -> Referee<Notes> {
    Referee::new(Notes::new(Duration::from_secs(60)))
}

fn see(referee: &mut Referee<Notes>, secs: u64, seen: Seen) {
    referee.observe(at(secs), seen, &mut Vec::new());
}

fn why(referee: &Referee<Notes>) -> String {
    let Verdict::Failed(failure) = referee.verdict() else { panic!("the referee failed the run") };
    failure.why
}

/// A referee that has seen the page `flaky` made, then deleted, and the
/// repository's scope listed after that.
fn deleted_then_listed() -> Referee<Notes> {
    let mut referee = referee();
    see(&mut referee, 0, changed(b"flaky", Some(page(b"flaky test"))));
    see(&mut referee, 1, changed(b"flaky", None));
    see(&mut referee, 2, Seen::Served { listing: 1, scope: REPO });
    see(&mut referee, 3, Seen::TakenIn { listing: 1 });
    referee
}

#[test]
fn a_line_for_a_page_deleted_before_its_scope_was_listed_fails_the_run() {
    let mut referee = deleted_then_listed();
    see(&mut referee, 4, lines(b"flaky", b"flaky test"));
    assert_eq!(why(&referee), "no line names a page deleted before its scope was last read: Repository(0) flaky");
}

#[test]
fn a_line_for_a_page_made_again_since_or_deleted_after_the_listing_passes() {
    let mut referee = deleted_then_listed();
    see(&mut referee, 4, changed(b"flaky", Some(page(b"again"))));
    see(&mut referee, 5, lines(b"flaky", b"again"));
    see(&mut referee, 6, changed(b"build", Some(page(b"build"))));
    see(&mut referee, 7, Seen::Served { listing: 2, scope: REPO });
    see(&mut referee, 8, changed(b"build", None));
    // The listing is taken in after the page was deleted: it was there when
    // it was served.
    see(&mut referee, 9, Seen::TakenIn { listing: 2 });
    see(&mut referee, 10, lines(b"build", b"build"));
    assert_eq!(referee.verdict(), Verdict::Passed);
}

#[test]
fn a_line_that_says_what_its_page_never_held_fails_the_run() {
    let mut referee = referee();
    see(&mut referee, 0, changed(b"flaky", Some(page(b"flaky test"))));
    see(&mut referee, 1, lines(b"flaky", b"slow build"));
    assert_eq!(why(&referee), r#"a line says what its page held: Repository(0) flaky "slow build""#);
}

#[test]
fn a_recall_answered_with_content_older_than_its_asking_fails_the_run() {
    let mut referee = referee();
    see(&mut referee, 0, changed(b"flaky", Some(page(b"old"))));
    see(&mut referee, 1, changed(b"flaky", Some(page(b"new"))));
    see(&mut referee, 2, Seen::Asked { call: 7, page: None });
    see(&mut referee, 3, Seen::Recalled { call: 7, entries: vec![entry(b"flaky", b"old")], failed: 0 });
    assert_eq!(why(&referee), "a recall answers with the page as it was meanwhile: Repository(0) flaky");
}

#[test]
fn a_recall_answered_with_content_from_meanwhile_passes() {
    let mut referee = referee();
    see(&mut referee, 0, changed(b"flaky", Some(page(b"old"))));
    see(&mut referee, 1, Seen::Asked { call: 7, page: None });
    see(&mut referee, 2, changed(b"flaky", Some(page(b"new"))));
    see(&mut referee, 3, Seen::Recalled { call: 7, entries: vec![entry(b"flaky", b"old")], failed: 0 });
    assert_eq!(referee.verdict(), Verdict::Passed, "it was so when the recall was asked");
}

#[test]
fn a_recall_by_name_that_finds_nothing_while_the_page_was_there_throughout_fails_the_run() {
    let mut referee = referee();
    see(&mut referee, 0, changed(b"flaky", Some(page(b"old"))));
    see(&mut referee, 1, Seen::Asked { call: 7, page: Some((REPO, b"flaky".to_vec())) });
    see(&mut referee, 2, Seen::Recalled { call: 7, entries: Vec::new(), failed: 1 });
    assert_eq!(referee.verdict(), Verdict::Passed, "a read that failed is said");
    see(&mut referee, 3, Seen::Asked { call: 8, page: Some((REPO, b"flaky".to_vec())) });
    see(&mut referee, 4, Seen::Recalled { call: 8, entries: Vec::new(), failed: 0 });
    assert_eq!(why(&referee), "a recall that found nothing asked for a page absent meanwhile: Repository(0) flaky");
}

#[test]
fn a_recall_by_name_that_finds_a_page_deleted_meanwhile_gone_passes() {
    let mut referee = referee();
    see(&mut referee, 0, changed(b"flaky", Some(page(b"old"))));
    see(&mut referee, 1, Seen::Asked { call: 7, page: Some((REPO, b"flaky".to_vec())) });
    see(&mut referee, 2, changed(b"flaky", None));
    see(&mut referee, 3, changed(b"flaky", Some(page(b"again"))));
    see(&mut referee, 4, Seen::Recalled { call: 7, entries: Vec::new(), failed: 0 });
    assert_eq!(referee.verdict(), Verdict::Passed);
}

#[test]
fn a_call_left_unanswered_fails_the_run_once_its_time_is_up() {
    let mut referee = referee();
    see(&mut referee, 1, Seen::Asked { call: 7, page: None });
    referee.fire(at(61), &mut Vec::new());
    assert_eq!(why(&referee), "Call(7) was not met by 61.000000000s");
}
