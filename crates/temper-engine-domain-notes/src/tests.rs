//! Feed the domain events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Env, List, Queue, ReplyTo, Time, Token};

use crate::{
    Author, Change, Domain, Entry, Event, Fact, Fetched, Item, Limits, Line, Listed, Noted, Page, Recall, Reference,
    Refusal, Request, Scope, Scopes, Wrote, max_out, resume, step, worst_case,
};

const LIMITS: Limits = Limits {
    scopes: 3,
    entries: 4,
    name_bytes: 16,
    description_bytes: 32,
    body_bytes: 64,
    references: 2,
    calls: 3,
    lines: 4,
    recalled: 2,
    facts: 64,
};

const REPO: Scope = Scope::Repository(0);
const DEPLOYMENT: Scope = Scope::Deployment;
const GOAL: Scope = Scope::Goal { repository: 0, number: 7 };

/// A run of the first repository, under no goal, and one under goal 7.
const RUN: Scopes = Scopes { repository: 0, goal: None };
const GOAL_RUN: Scopes = Scopes { repository: 0, goal: Some(Item { repository: 0, number: 7 }) };

const ALICE: Author = Author::Person(1);

/// The domain, its environment, and room for one step's output.
struct Harness {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        let out = Queue::with_capacity(max_out(&limits));
        Harness { domain: Domain::new(&limits), env: Env { now: Time::ZERO, limits }, out }
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Resumes a ready call, returning what it emitted.
    fn resume(&mut self) -> Box<[Request]> {
        assert!(self.domain.is_ready(), "a call is ready");
        resume(&mut self.domain, &self.env, &mut self.out);
        self.drain()
    }

    fn drain(&mut self) -> Box<[Request]> {
        let mut requests = List::with_capacity(self.out.len());
        for _ in 0..self.out.len() {
            requests.push(self.out.pop().unwrap()).unwrap();
        }
        // The iteration ends: the reclaim point.
        self.domain.reclaim();
        requests.into_boxed()
    }

    /// Lists `pages` (name, revision) in answer to the list `owner`.
    fn list(&mut self, owner: Token, pages: &[(&[u8], u64)]) -> Box<[Request]> {
        let mut listed = List::with_capacity(u32::try_from(pages.len()).unwrap());
        for (name, revision) in pages {
            listed.push(Listed { name: Box::from(*name), revision: *revision }).unwrap();
        }
        self.step(Event::Listed { owner, pages: Some(listed.into_boxed()) })
    }

    /// Reads a page described so at `revision` in answer to the fetch
    /// `owner`.
    fn read(&mut self, owner: Token, description: &[u8], revision: u64) -> Box<[Request]> {
        self.step(Event::Fetched { owner, fetched: Fetched::Page { revision, page: page(description) } })
    }

    /// Reads `scope` to the end of a pass, with one page per description,
    /// named after it, given the list request `owner`: the pages are read in
    /// the order of their names.
    fn pass(&mut self, owner: Token, scope: Scope, descriptions: &[&[u8]]) -> Box<[Request]> {
        let mut pages = List::with_capacity(4);
        for description in descriptions {
            pages.push((*description, 1)).unwrap();
        }
        let mut asked = self.list(owner, pages.as_slice());
        for _ in descriptions {
            let [Request::Fetch { owner, scope: asked_scope, name }] = &*asked else {
                panic!("the next page is read: {asked:?}")
            };
            assert_eq!(*asked_scope, scope);
            let (owner, name) = (*owner, name.clone());
            asked = self.read(owner, &name, 1);
        }
        asked
    }
}

fn reply(n: u64) -> ReplyTo {
    ReplyTo::new(Token::new(n))
}

fn refused(n: u64, refusal: Refusal) -> Request {
    Request::Refused { reply_to: reply(n), refusal }
}

fn note(n: u64, name: &[u8], change: Change) -> Event {
    Event::Note { reply_to: reply(n), scope: REPO, name: Box::from(name), change }
}

fn page(description: &[u8]) -> Page {
    Page {
        description: Box::from(description),
        author: ALICE,
        references: Box::new([Reference { repository: 0, number: 3 }]),
        body: Box::from(*b"the body"),
    }
}

fn line(scope: Scope, description: &[u8]) -> Line {
    let page = page(description);
    Line {
        scope,
        name: Box::from(description),
        description: page.description,
        author: page.author,
        references: page.references,
    }
}

/// The owner of the list of `scope` among `asked`.
fn listing(asked: &[Request], scope: Scope) -> Token {
    for request in asked {
        match request {
            Request::List { owner, scope: listed } => {
                if *listed == scope {
                    return *owner;
                }
            }
            Request::Indexed { .. }
            | Request::Found { .. }
            | Request::Recalled { .. }
            | Request::Noted { .. }
            | Request::Refused { .. }
            | Request::Fetch { .. }
            | Request::Create { .. }
            | Request::Edit { .. }
            | Request::Delete { .. } => {}
        }
    }
    panic!("{scope:?} is listed: {asked:?}")
}

/// The owner of the one operation `asked` holds.
fn owner(asked: &[Request]) -> Token {
    match asked {
        [
            Request::List { owner, .. }
            | Request::Fetch { owner, .. }
            | Request::Create { owner, .. }
            | Request::Edit { owner, .. }
            | Request::Delete { owner, .. },
        ] => *owner,
        _ => panic!("one operation: {asked:?}"),
    }
}

/// A harness whose repository and deployment scopes have been read, with
/// the pages `repository` and `deployment` describe, and an index call
/// answered.
fn read(repository: &[&[u8]], deployment: &[&[u8]]) -> Harness {
    let mut h = Harness::new(LIMITS);
    let asked = h.step(Event::Index { reply_to: reply(1), scopes: RUN, budget: 1000 });
    assert_eq!(asked.len(), 2, "both scopes are listed at once");
    let (repo, deploy) = (listing(&asked, REPO), listing(&asked, DEPLOYMENT));
    assert!(h.pass(repo, REPO, repository).is_empty());
    assert!(!h.domain.is_ready(), "the deployment's pass has not ended");
    assert!(h.pass(deploy, DEPLOYMENT, deployment).is_empty());
    let [Request::Indexed { .. }] = &*h.resume() else { panic!("the index is answered") };
    h
}

// Indexes.

#[test]
fn an_index_lists_its_scopes_reads_their_pages_and_answers_narrowest_first() {
    let mut h = Harness::new(LIMITS);
    let asked = h.step(Event::Index { reply_to: reply(1), scopes: RUN, budget: 1000 });
    let (repo, deploy) = (listing(&asked, REPO), listing(&asked, DEPLOYMENT));
    assert!(h.pass(deploy, DEPLOYMENT, &[b"style"]).is_empty());
    assert!(h.pass(repo, REPO, &[b"flaky", b"build"]).is_empty());
    let expected = Request::Indexed {
        reply_to: reply(1),
        lines: Box::new([line(REPO, b"build"), line(REPO, b"flaky"), line(DEPLOYMENT, b"style")]),
        more: 0,
        unread: 0,
    };
    assert_eq!(*h.resume(), [expected]);
    assert_eq!(h.domain.calls(), 0, "the call is answered");
}

#[test]
fn an_index_answers_at_once_from_scopes_read_and_takes_what_fits_its_budget() {
    let mut h = read(&[b"aa", b"bb"], &[b"cc"]);
    // A line costs its name and description: four bytes here.
    let asked = h.step(Event::Index { reply_to: reply(2), scopes: RUN, budget: 6 });
    let expected = Request::Indexed { reply_to: reply(2), lines: Box::new([line(REPO, b"aa")]), more: 2, unread: 0 };
    assert_eq!(*asked, [expected], "no wiki read: the index is kept");
    let asked = h.step(Event::Index { reply_to: reply(3), scopes: RUN, budget: 0 });
    assert_eq!(*asked, [Request::Indexed { reply_to: reply(3), lines: Box::new([]), more: 3, unread: 0 }]);
}

#[test]
fn an_index_answers_with_no_more_lines_than_its_limit() {
    let mut h = read(&[b"a", b"b", b"c"], &[b"d", b"e"]);
    let [Request::Indexed { lines, more, .. }] =
        &*h.step(Event::Index { reply_to: reply(2), scopes: RUN, budget: 1000 })
    else {
        panic!("answered")
    };
    assert_eq!((lines.len(), *more), (4, 1));
}

#[test]
fn a_goal_s_notes_come_first() {
    let mut h = read(&[b"repo"], &[b"deploy"]);
    let asked = h.step(Event::Index { reply_to: reply(2), scopes: GOAL_RUN, budget: 1000 });
    assert!(h.pass(owner(&asked), GOAL, &[b"goal"]).is_empty());
    let [Request::Indexed { lines, .. }] = &*h.resume() else { panic!("answered") };
    assert_eq!(**lines, [line(GOAL, b"goal"), line(REPO, b"repo"), line(DEPLOYMENT, b"deploy")]);
}

#[test]
fn a_goal_in_another_repository_is_a_scope_of_that_repository() {
    let mut h = read(&[b"repo"], &[b"deploy"]);
    let goal = Scope::Goal { repository: 1, number: 9 };
    let away = Scopes { repository: 0, goal: Some(Item { repository: 1, number: 9 }) };
    let asked = h.step(Event::Index { reply_to: reply(2), scopes: away, budget: 1000 });
    assert_eq!(listing(&asked, goal), owner(&asked));
    assert!(h.pass(owner(&asked), goal, &[b"goal"]).is_empty());
    let [Request::Indexed { lines, .. }] = &*h.resume() else { panic!("answered") };
    assert_eq!(**lines, [line(goal, b"goal"), line(REPO, b"repo"), line(DEPLOYMENT, b"deploy")]);
}

#[test]
fn a_search_finds_the_descriptions_that_hold_its_query() {
    let mut h = read(&[b"flaky test", b"build cache"], &[b"test style"]);
    let asked = h.step(Event::Search { reply_to: reply(2), scopes: RUN, query: Box::from(*b"test"), most: 1 });
    let expected =
        Request::Found { reply_to: reply(2), lines: Box::new([line(REPO, b"flaky test")]), more: 1, unread: 0 };
    assert_eq!(*asked, [expected]);
    let asked = h.step(Event::Search { reply_to: reply(3), scopes: RUN, query: Box::new([]), most: 9 });
    let [Request::Found { lines, more: 0, .. }] = &*asked else { panic!("an empty query finds them all") };
    assert_eq!(lines.len(), 3);
}

// Keeping up.

#[test]
fn listing_again_forgets_deleted_pages_and_reads_only_the_changed_ones() {
    let mut h = read(&[b"aa", b"bb", b"cc"], &[]);
    let asked = h.step(Event::Refresh { scope: REPO });
    let asked = h.list(owner(&asked), &[(b"aa", 1), (b"bb", 2), (b"dd", 1)]);
    let [Request::Fetch { owner: first, name, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(&**name, b"bb", "at a revision it does not know");
    let asked = h.read(*first, b"bb", 2);
    let [Request::Fetch { owner: second, name, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(&**name, b"dd", "a page it does not know");
    assert!(h.read(*second, b"dd", 1).is_empty());
    let [Request::Indexed { lines, .. }] = &*h.step(Event::Index { reply_to: reply(2), scopes: RUN, budget: 1000 })
    else {
        panic!("answered")
    };
    assert_eq!(**lines, [line(REPO, b"aa"), line(REPO, b"bb"), line(REPO, b"dd")], "cc is gone");
}

#[test]
fn a_changed_page_is_read_again_and_one_gone_is_forgotten() {
    let mut h = read(&[b"aa", b"bb"], &[]);
    let asked = h.step(Event::Changed { scope: REPO, name: Box::from(*b"aa") });
    let asked = h.step(Event::Fetched { owner: owner(&asked), fetched: Fetched::Gone });
    assert!(asked.is_empty());
    let asked = h.step(Event::Changed { scope: REPO, name: Box::from(*b"bb") });
    assert!(h.step(Event::Fetched { owner: owner(&asked), fetched: Fetched::Failed }).is_empty());
    let [Request::Indexed { lines, .. }] = &*h.step(Event::Index { reply_to: reply(2), scopes: RUN, budget: 1000 })
    else {
        panic!("answered")
    };
    assert_eq!(**lines, [line(REPO, b"bb")], "a failed read keeps what it knew");
    assert!(h.step(Event::Changed { scope: GOAL, name: Box::from(*b"x") }).is_empty(), "a scope not kept");
    assert!(h.step(Event::Refresh { scope: GOAL }).is_empty());
}

#[test]
fn a_scope_runs_one_wiki_operation_at_a_time_and_hints_wait_their_turn() {
    let mut h = read(&[b"aa"], &[]);
    let asked = h.step(Event::Changed { scope: REPO, name: Box::from(*b"aa") });
    let fetch = owner(&asked);
    assert!(h.step(Event::Refresh { scope: REPO }).is_empty(), "the read is in flight");
    assert!(h.step(Event::Changed { scope: REPO, name: Box::from(*b"bb") }).is_empty());
    let asked = h.read(fetch, b"aa", 2);
    let [Request::Fetch { owner: fetch, name, .. }] = &*asked else { panic!("then the page hinted: {asked:?}") };
    assert_eq!(&**name, b"bb");
    let asked = h.read(*fetch, b"bb", 1);
    let [Request::List { owner: list, scope: REPO }] = &*asked else { panic!("then the listing: {asked:?}") };
    assert!(h.list(*list, &[(b"aa", 2), (b"bb", 1)]).is_empty(), "nothing it does not know");
}

#[test]
fn listings_asked_for_meanwhile_do_not_hold_a_first_pass_back() {
    let mut h = Harness::new(LIMITS);
    let asked = h.step(Event::Index { reply_to: reply(1), scopes: RUN, budget: 1000 });
    assert!(h.list(listing(&asked, DEPLOYMENT), &[]).is_empty());
    let asked = h.list(listing(&asked, REPO), &[(b"aa", 1)]);
    assert!(h.step(Event::Refresh { scope: REPO }).is_empty(), "the read is in flight");
    let asked = h.read(owner(&asked), b"aa", 1);
    let [Request::List { scope: REPO, .. }] = &*asked else { panic!("listed again: {asked:?}") };
    let [Request::Indexed { lines, .. }] = &*h.resume() else { panic!("the pass ended all the same") };
    assert_eq!(**lines, [line(REPO, b"aa")]);
}

#[test]
fn pages_past_the_limits_are_left_out_of_the_index() {
    let mut h = Harness::new(LIMITS);
    let asked = h.step(Event::Index { reply_to: reply(1), scopes: RUN, budget: 1000 });
    let deploy = listing(&asked, DEPLOYMENT);
    assert!(h.list(deploy, &[]).is_empty());
    let long: &[u8] = &[b'n'; 17];
    let pages: [(&[u8], u64); 6] = [(b"a", 1), (long, 1), (b"", 1), (b"b", 1), (b"c", 1), (b"d", 1)];
    let mut asked = h.list(listing(&asked, REPO), &pages);
    let mut read = 0_u32;
    for _ in 0..4_u32 {
        let [Request::Fetch { owner, name, .. }] = &*asked else { panic!("{asked:?}") };
        let description: &[u8] = if &**name == b"d" { &[b'x'; 33] } else { name };
        asked = h.read(*owner, description, 1);
        read = read.checked_add(1).unwrap();
    }
    assert_eq!(read, 4, "the first four it may name: a, b, c and d");
    let [Request::Indexed { lines, .. }] = &*h.resume() else { panic!("answered") };
    assert_eq!(**lines, [line(REPO, b"a"), line(REPO, b"b"), line(REPO, b"c")], "d's description is too long");
    let (mut listed, mut left_out) = (0_u32, 0_u32);
    while let Some(fact) = h.domain.pop_fact() {
        if fact == (Fact::Listed { pages: 4, left_out: 2 }) {
            listed = listed.checked_add(1).unwrap();
        }
        if fact == Fact::LeftOut {
            left_out = left_out.checked_add(1).unwrap();
        }
    }
    assert_eq!((listed, left_out), (1, 1), "the listing's two, then d");
}

#[test]
fn a_scope_that_could_not_be_listed_answers_unread_and_is_listed_again_when_needed() {
    let mut h = Harness::new(LIMITS);
    let asked = h.step(Event::Index { reply_to: reply(1), scopes: RUN, budget: 1000 });
    assert!(h.step(Event::Listed { owner: listing(&asked, REPO), pages: None }).is_empty());
    assert!(h.list(listing(&asked, DEPLOYMENT), &[]).is_empty());
    let expected = Request::Indexed { reply_to: reply(1), lines: Box::new([]), more: 0, unread: 1 };
    assert_eq!(*h.resume(), [expected]);
    let asked = h.step(Event::Index { reply_to: reply(2), scopes: RUN, budget: 1000 });
    let [Request::List { owner, scope: REPO }] = &*asked else { panic!("listed again: {asked:?}") };
    assert!(h.pass(*owner, REPO, &[b"aa"]).is_empty());
    let expected = Request::Indexed { reply_to: reply(2), lines: Box::new([line(REPO, b"aa")]), more: 0, unread: 0 };
    assert_eq!(*h.resume(), [expected]);
}

// Recalls.

#[test]
fn a_recall_by_name_reads_the_page_afresh() {
    let mut h = read(&[b"aa"], &[]);
    let recall = Recall::Name { scope: REPO, name: Box::from(*b"aa") };
    let asked = h.step(Event::Recall { reply_to: reply(2), recall });
    let [Request::Fetch { owner: fetch, scope: REPO, name }] = &*asked else { panic!("read afresh: {asked:?}") };
    assert_eq!(&**name, b"aa");
    let asked = h.read(*fetch, b"aa, edited", 5);
    let entry = Entry { scope: REPO, name: Box::from(*b"aa"), revision: 5, page: page(b"aa, edited") };
    assert_eq!(*asked, [Request::Recalled { reply_to: reply(2), entries: Box::new([entry]), failed: 0 }]);
    // A page that is gone is left out; one that cannot be read is counted.
    for (fetched, failed) in [(Fetched::Gone, 0), (Fetched::Failed, 1)] {
        let recall = Recall::Name { scope: GOAL, name: Box::from(*b"zz") };
        let asked = h.step(Event::Recall { reply_to: reply(3), recall });
        let answer = h.step(Event::Fetched { owner: owner(&asked), fetched });
        assert_eq!(*answer, [Request::Recalled { reply_to: reply(3), entries: Box::new([]), failed }]);
    }
    assert_eq!(h.domain.scopes(), 2, "a recall by name keeps no scope");
}

#[test]
fn a_recall_by_search_reads_what_the_index_finds_in_turn() {
    let mut h = read(&[b"flaky test", b"build"], &[b"test style", b"tests run"]);
    let recall = Recall::Search { scopes: RUN, query: Box::from(*b"test"), most: 5 };
    let asked = h.step(Event::Recall { reply_to: reply(2), recall });
    let [Request::Fetch { owner, name, .. }] = &*asked else {
        panic!("its scopes are read: it reads at once {asked:?}")
    };
    assert_eq!(&**name, b"flaky test");
    let asked = h.read(*owner, b"flaky test", 1);
    let [Request::Fetch { owner, name, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(&**name, b"test style", "at most the recall's limit of two");
    let asked = h.step(Event::Fetched { owner: *owner, fetched: Fetched::Gone });
    let entry = Entry { scope: REPO, name: Box::from(*b"flaky test"), revision: 1, page: page(b"flaky test") };
    assert_eq!(*asked, [Request::Recalled { reply_to: reply(2), entries: Box::new([entry]), failed: 0 }]);
}

// Writes.

#[test]
fn a_note_written_is_learned_by_the_index() {
    let mut h = read(&[b"aa"], &[]);
    let change = Change::New(page(b"new"));
    let asked = h.step(Event::Note { reply_to: reply(2), scope: REPO, name: Box::from(*b"new"), change });
    let [Request::Create { owner, scope: REPO, name, page: written }] = &*asked else { panic!("{asked:?}") };
    assert_eq!((&**name, written), (&b"new"[..], &page(b"new")));
    let asked = h.step(Event::Wrote { owner: *owner, wrote: Wrote::Done { revision: 1 } });
    assert_eq!(*asked, [Request::Noted { reply_to: reply(2), noted: Noted::Done }]);
    // A revision of the page the run recalled at revision 1, which it still
    // is: read afresh, then edited.
    let change = Change::Revise { page: page(b"aa, revised"), revision: 1 };
    let asked = h.step(Event::Note { reply_to: reply(3), scope: REPO, name: Box::from(*b"aa"), change });
    let [Request::Fetch { owner, name, .. }] = &*asked else { panic!("read afresh: {asked:?}") };
    assert_eq!(&**name, b"aa");
    let asked = h.read(*owner, b"aa", 1);
    let [Request::Edit { owner, .. }] = &*asked else { panic!("{asked:?}") };
    h.step(Event::Wrote { owner: *owner, wrote: Wrote::Done { revision: 2 } });
    let [Request::Indexed { lines, .. }] = &*h.step(Event::Index { reply_to: reply(4), scopes: RUN, budget: 1000 })
    else {
        panic!("answered")
    };
    let mut revised = line(REPO, b"aa, revised");
    revised.name = Box::from(*b"aa");
    assert_eq!(**lines, [revised, line(REPO, b"new")]);
    let asked = h.step(note(5, b"new", Change::Remove));
    let [Request::Delete { owner, scope: REPO, .. }] = &*asked else { panic!("{asked:?}") };
    h.step(Event::Wrote { owner: *owner, wrote: Wrote::Done { revision: 3 } });
    let [Request::Indexed { lines, .. }] = &*h.step(Event::Index { reply_to: reply(6), scopes: RUN, budget: 1000 })
    else {
        panic!("answered")
    };
    assert_eq!(lines.len(), 1, "removed");
}

#[test]
fn a_note_that_finds_the_wiki_otherwise_says_so() {
    let mut h = read(&[b"aa"], &[]);
    let asked = h.step(note(2, b"aa", Change::Remove));
    let asked = h.step(Event::Wrote { owner: owner(&asked), wrote: Wrote::Missing });
    assert_eq!(*asked, [Request::Noted { reply_to: reply(2), noted: Noted::Missing }], "and the index forgets it");
    let asked = h.step(note(3, b"bb", Change::New(page(b"bb"))));
    let asked = h.step(Event::Wrote { owner: owner(&asked), wrote: Wrote::Exists });
    let [Request::Noted { noted: Noted::Exists, .. }, Request::Fetch { name, .. }] = &*asked else {
        panic!("answered, then the page is read: {asked:?}")
    };
    assert_eq!(&**name, b"bb");
    let asked = h.read(owner(&asked[1..]), b"theirs", 4);
    assert!(asked.is_empty());
    let asked = h.step(note(4, b"cc", Change::New(page(b"cc"))));
    let asked = h.step(Event::Wrote { owner: owner(&asked), wrote: Wrote::Failed });
    assert_eq!(*asked, [Request::Noted { reply_to: reply(4), noted: Noted::Unavailable }]);
    let [Request::Indexed { lines, .. }] = &*h.step(Event::Index { reply_to: reply(5), scopes: RUN, budget: 1000 })
    else {
        panic!("answered")
    };
    let mut theirs = line(REPO, b"theirs");
    theirs.name = Box::from(*b"bb");
    assert_eq!(**lines, [theirs]);
}

#[test]
fn a_revision_of_a_page_written_since_the_run_recalled_it_is_refused_as_moved() {
    let mut h = read(&[b"aa", b"bb"], &[]);
    let change = Change::Revise { page: page(b"mine"), revision: 1 };
    let asked = h.step(note(2, b"aa", change));
    let asked = h.read(owner(&asked), b"theirs", 2);
    assert_eq!(*asked, [Request::Noted { reply_to: reply(2), noted: Noted::Moved }], "nothing written");
    let [Request::Indexed { lines, .. }] = &*h.step(Event::Index { reply_to: reply(3), scopes: RUN, budget: 1000 })
    else {
        panic!("answered")
    };
    let mut theirs = line(REPO, b"theirs");
    theirs.name = Box::from(*b"aa");
    assert_eq!(**lines, [theirs, line(REPO, b"bb")], "the index learned what was read");
    // Gone, or not read.
    let asked = h.step(note(4, b"bb", Change::Revise { page: page(b"mine"), revision: 1 }));
    let asked = h.step(Event::Fetched { owner: owner(&asked), fetched: Fetched::Gone });
    assert_eq!(*asked, [Request::Noted { reply_to: reply(4), noted: Noted::Missing }]);
    let asked = h.step(note(5, b"aa", Change::Revise { page: page(b"mine"), revision: 2 }));
    let asked = h.step(Event::Fetched { owner: owner(&asked), fetched: Fetched::Failed });
    assert_eq!(*asked, [Request::Noted { reply_to: reply(5), noted: Noted::Unavailable }]);
}

#[test]
fn a_pass_reads_what_its_listing_wanted_while_hints_and_notes_wait_for_it() {
    let mut h = Harness::new(LIMITS);
    let asked = h.step(Event::Index { reply_to: reply(1), scopes: RUN, budget: 1000 });
    assert!(h.list(listing(&asked, DEPLOYMENT), &[]).is_empty());
    let asked = h.list(listing(&asked, REPO), &[(b"aa", 1), (b"bb", 1)]);
    let [Request::Fetch { owner: first, .. }] = &*asked else { panic!("{asked:?}") };
    // A storm of hints and a note: none of it goes ahead of the pass.
    for name in [b"cc", b"dd", b"ee", b"aa"] {
        assert!(h.step(Event::Changed { scope: REPO, name: Box::from(*name) }).is_empty());
    }
    assert!(h.step(Event::Refresh { scope: REPO }).is_empty());
    assert!(h.step(note(2, b"ff", Change::New(page(b"ff")))).is_empty());
    let asked = h.read(*first, b"aa", 1);
    let [Request::Fetch { owner: second, name, .. }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(&**name, b"bb", "the pass's own read");
    let asked = h.read(*second, b"bb", 1);
    let [Request::Create { .. }] = &*asked else { panic!("the pass ended, then the note: {asked:?}") };
    let [Request::Indexed { lines, .. }] = &*h.resume() else { panic!("answered once the pass ended") };
    assert_eq!(**lines, [line(REPO, b"aa"), line(REPO, b"bb")]);
}

#[test]
fn a_note_waits_its_turn_behind_the_operation_in_flight() {
    let mut h = Harness::new(LIMITS);
    let asked =
        h.step(Event::Note { reply_to: reply(1), scope: GOAL, name: Box::from(*b"aa"), change: Change::Remove });
    let [Request::List { owner, scope: GOAL }] = &*asked else { panic!("a new scope is listed first: {asked:?}") };
    let asked = h.list(*owner, &[]);
    let [Request::Delete { owner, scope: GOAL, .. }] = &*asked else { panic!("then the write: {asked:?}") };
    let asked = h.step(Event::Wrote { owner: *owner, wrote: Wrote::Done { revision: 1 } });
    assert_eq!(*asked, [Request::Noted { reply_to: reply(1), noted: Noted::Done }]);
}

// The entrance.

#[test]
fn a_call_past_the_limits_is_refused_at_the_entrance() {
    let mut h = Harness::new(LIMITS);
    let long: Box<[u8]> = Box::new([b'q'; 33]);
    let asked = h.step(Event::Search { reply_to: reply(1), scopes: RUN, query: long.clone(), most: 1 });
    assert_eq!(*asked, [refused(1, Refusal::Oversized)]);
    let recall = Recall::Search { scopes: RUN, query: long, most: 1 };
    assert_eq!(*h.step(Event::Recall { reply_to: reply(2), recall }), [refused(2, Refusal::Oversized)]);
    let recall = Recall::Name { scope: REPO, name: Box::new([b'n'; 17]) };
    assert_eq!(*h.step(Event::Recall { reply_to: reply(3), recall }), [refused(3, Refusal::Oversized)]);
    let mut big = page(b"big");
    big.body = Box::new([b'b'; 65]);
    let asked =
        h.step(Event::Note { reply_to: reply(4), scope: REPO, name: Box::from(*b"aa"), change: Change::New(big) });
    assert_eq!(*asked, [refused(4, Refusal::Oversized)]);
    let asked = h.step(Event::Note { reply_to: reply(5), scope: REPO, name: Box::new([]), change: Change::Remove });
    assert_eq!(*asked, [refused(5, Refusal::Oversized)]);
    assert_eq!((h.domain.calls(), h.domain.scopes()), (0, 0), "nothing changed");
}

#[test]
fn calls_past_their_limit_are_refused_as_busy() {
    let mut h = Harness::new(LIMITS);
    h.step(Event::Index { reply_to: reply(1), scopes: RUN, budget: 1 });
    h.step(Event::Index { reply_to: reply(2), scopes: RUN, budget: 1 });
    h.step(Event::Recall { reply_to: reply(3), recall: Recall::Name { scope: REPO, name: Box::from(*b"a") } });
    let asked = h.step(Event::Index { reply_to: reply(4), scopes: RUN, budget: 1 });
    assert_eq!(*asked, [refused(4, Refusal::Busy)]);
    let recall = Recall::Name { scope: REPO, name: Box::from(*b"a") };
    assert_eq!(*h.step(Event::Recall { reply_to: reply(5), recall }), [refused(5, Refusal::Busy)]);
}

#[test]
fn a_scope_idle_is_evicted_for_another_and_one_in_use_never_is() {
    let mut h = read(&[b"aa"], &[b"dd"]);
    // A goal's scope takes the last place.
    let asked = h.step(Event::Index { reply_to: reply(2), scopes: GOAL_RUN, budget: 1000 });
    assert!(h.pass(owner(&asked), GOAL, &[]).is_empty());
    h.resume();
    // Another repository's run: its scope takes the least recently used
    // idle one's place, the goal's: the repository's and the deployment's
    // were used since.
    h.step(Event::Index { reply_to: reply(3), scopes: RUN, budget: 1000 });
    let asked = h.step(Event::Index { reply_to: reply(4), scopes: Scopes { repository: 1, goal: None }, budget: 9 });
    let [Request::List { owner, scope: Scope::Repository(1) }] = &*asked else { panic!("{asked:?}") };
    assert_eq!(h.domain.scopes(), 3);
    assert!(h.step(Event::Refresh { scope: GOAL }).is_empty(), "the goal's scope was evicted");
    // While every scope is pinned by a call, none is evicted: a call that
    // needs another is refused.
    let asked = h.step(Event::Index { reply_to: reply(5), scopes: GOAL_RUN, budget: 9 });
    assert_eq!(*asked, [refused(5, Refusal::Busy)]);
    assert!(h.pass(*owner, Scope::Repository(1), &[]).is_empty());
    let [Request::Indexed { reply_to, unread: 0, .. }] = &*h.resume() else { panic!("answered") };
    assert_eq!(*reply_to, reply(4));
}

#[test]
fn a_terminal_that_names_no_operation_in_flight_is_dropped() {
    let mut h = read(&[b"aa"], &[]);
    let asked = h.step(Event::Changed { scope: REPO, name: Box::from(*b"aa") });
    let fetch = owner(&asked);
    assert!(h.read(fetch, b"aa", 2).is_empty());
    assert!(h.read(fetch, b"aa", 3).is_empty(), "its operation ended, and was reclaimed");
    assert_eq!(h.domain.ops(), 0);
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the test limits fit");
    let entries = u64::from(LIMITS.scopes * LIMITS.entries * (LIMITS.name_bytes + LIMITS.description_bytes));
    assert!(bound > entries, "it counts every scope's index full");
    let more = worst_case(&Limits { scopes: 6, ..LIMITS }).expect("fits");
    assert!(more > bound, "a scope more is more");
    let wider = worst_case(&Limits { body_bytes: 4096, ..LIMITS }).expect("fits");
    assert!(wider > bound, "a note's or a recall's body counts");
    assert_eq!(worst_case(&Limits { scopes: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { calls: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { entries: u32::MAX, name_bytes: u32::MAX, ..LIMITS }), None);
}
