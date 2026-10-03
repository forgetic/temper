//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, List, Queue, Time, Token};

use crate::api::{Answer, Body, Comment, Error, Kind, Mark, Op, Page, Pull, Remark, Review, State, Summary, Verdict};
use crate::{
    Cause, Ci, Config, Content, Event, Fact, Failure, Item, Level, Limits, Model, News, Position, Priority, Read,
    Record, Request, Reviewed, View, Why, Write, Written, fire, max_out, resume, step, worst_case,
};

/// The engine's forge user, and a person's.
const ENGINE: u64 = 1;
const PERSON: u64 = 7;

const LIMITS: Limits = Limits {
    repositories: 2,
    items: 3,
    labels: 3,
    members: 3,
    inbox: 3,
    reviewers: 4,
    reads: 2,
    writes: 3,
    calls: 8,
    page: 3,
    name_bytes: 16,
    title_bytes: 16,
    body_bytes: 32,
    rate: 100,
    window: Duration::from_secs(60),
    reserve: 0,
    poll: Duration::from_secs(30),
    hinted: Duration::from_secs(2),
    resolution: Duration::from_secs(1),
    slow: Duration::from_secs(600),
    probes: 2,
    backoff: Duration::from_secs(1),
    backoff_max: Duration::from_secs(8),
    attempts: 3,
    lifetime: Duration::ZERO,
    facts: 256,
};

const TRACKING: &[u8] = b"temper";
const HAND_IN: &[u8] = b"hand-in";
/// The labels the engine projects; others are people's.
const PROJECTED: [&[u8]; 2] = [b"a", b"b"];

/// What caused a write the parent asks for again.
const CAUSE: Cause = Cause { comment: 50, at: Time::ZERO.saturating_add(Duration::from_secs(20)) };

/// A head's commit.
const HEAD: [u8; 32] = [7; 32];
const OTHER: [u8; 32] = [8; 32];

fn config() -> Config {
    Config { engine: ENGINE, tracking: bytes(TRACKING), hand_in: bytes(HAND_IN), projected: labels(&PROJECTED) }
}

/// The model, its environment, and room for one step's output.
struct Harness {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
}

/// A call that went out.
#[derive(Debug)]
struct Sent {
    call: Token,
    repository: u32,
    op: Op,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        let out = Queue::with_capacity(max_out(&limits));
        Harness { model: Model::new(&limits, config(), 7), env: Env { now: Time::ZERO, limits }, out }
    }

    /// A model whose cold start is done, with nothing tracked.
    fn started(limits: Limits) -> Harness {
        let mut h = Harness::new(limits);
        h.start(&[], &[]);
        h
    }

    fn at(&mut self, secs: u64) {
        self.env.now = at(secs);
    }

    /// Steps `event`, returning what it emitted, oldest first. The iteration
    /// ends: the reclaim point.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Fires every alarm due now.
    fn fire(&mut self) -> Box<[Request]> {
        let mut requests = List::with_capacity(64);
        for _ in 0..64_u32 {
            if !self.model.is_due(self.env.now) {
                break;
            }
            fire(&mut self.model, &self.env, &mut self.out);
            for request in self.drain() {
                requests.push(request).expect("room for what alarms emit");
            }
        }
        requests.into_boxed()
    }

    /// Sends every call the budget allows, oldest first.
    fn send(&mut self) -> Box<[Sent]> {
        let mut sent = List::with_capacity(64);
        for _ in 0..64_u32 {
            if !self.model.is_ready() {
                break;
            }
            resume(&mut self.model, &self.env, &mut self.out);
            for request in self.drain() {
                let Request::Call { call, repository, op } = request else {
                    panic!("a resume sends a call: {request:?}");
                };
                sent.push(Sent { call, repository, op }).expect("room for the calls sent");
            }
        }
        sent.into_boxed()
    }

    /// Sends the one call ready.
    fn send_one(&mut self) -> Sent {
        let sent = self.send();
        assert_eq!(sent.len(), 1, "one call goes out: {sent:?}");
        let mut sent = sent.into_iter();
        sent.next().expect("checked above")
    }

    /// Sends the calls ready, answering listings with nothing, and returns
    /// the one asking `op`.
    fn sends_for(&mut self, op: &Op) -> Sent {
        let mut found = None;
        for sent in self.send() {
            if sent.op == *op {
                found = Some(sent);
            } else {
                assert!(lists_changes(&sent.op), "only listings besides: {sent:?}");
                self.answer(&sent, page(Box::new([]), false));
            }
        }
        found.expect("a call asks it")
    }

    /// Sends the calls ready, and returns the one asking `op`.
    fn send_for(&mut self, op: &Op) -> Sent {
        for sent in self.send() {
            if sent.op == *op {
                return sent;
            }
        }
        panic!("a call asks {op:?}");
    }

    /// Answers `sent` with `result`: a page of items made now, unless it says
    /// when it was made.
    fn answer(&mut self, sent: &Sent, result: Result<Answer, Error>) -> Box<[Request]> {
        let result = match result {
            Ok(Answer::Items { items, more, now }) if now == Time::ZERO => {
                Ok(Answer::Items { items, more, now: self.env.now })
            }
            other => other,
        };
        self.step(Event::Answered { call: sent.call, result })
    }

    fn drain(&mut self) -> Box<[Request]> {
        let most = max_out(&self.env.limits);
        let mut requests = List::with_capacity(most);
        for _ in 0..most {
            let Some(request) = self.out.pop() else { break };
            requests.push(request).expect("room for max_out");
        }
        assert!(self.out.is_empty(), "a step emits at most max_out");
        self.model.reclaim();
        requests.into_boxed()
    }

    /// The cold start: repository 0 lists `tracked` and `handed`, repository
    /// 1 nothing; each tracked item is then read, and found with a record.
    fn start(&mut self, tracked: &[Summary], handed: &[Summary]) -> Box<[Request]> {
        assert!(self.fire().is_empty(), "the first passes are due at once and tell nothing");
        let tracking = tracked_listing(1);
        let handing = Op::Items {
            state: Some(State::Open),
            kind: Some(Kind::Issue),
            label: Some(bytes(HAND_IN)),
            author: None,
            since: Time::ZERO,
            page: 1,
        };
        let mut told = List::with_capacity(64);
        for _ in 0..16_u32 {
            for sent in self.send() {
                let result = if let Op::Item { number, after: 0 } = sent.op {
                    let comments = Box::new([record(100, number, Position::START)]);
                    item_page(issue(number, &[TRACKING], 1), comments, false)
                } else if sent.op == tracking && sent.repository == 0 {
                    page(copies(tracked), false)
                } else if sent.op == handing && sent.repository == 0 {
                    page(copies(handed), false)
                } else {
                    assert!(sent.op == tracking || sent.op == handing, "the cold start: {sent:?}");
                    page(Box::new([]), false)
                };
                for request in self.answer(&sent, result) {
                    told.push(request).expect("room");
                }
            }
            if told.last() == Some(&Request::Loaded) {
                return told.into_boxed();
            }
        }
        panic!("the cold start ends: {:?}", told.as_slice());
    }

    /// Runs the next pass of repository 0 at `secs`, answering its listing
    /// with `changes`; returns what it told, and the calls it then sent.
    fn pass(&mut self, secs: u64, changes: &[Summary]) -> (Box<[Request]>, Box<[Sent]>) {
        self.at(secs);
        self.fire();
        let mut listing = None;
        let mut others = List::with_capacity(16);
        for sent in self.send() {
            let changes = lists_changes(&sent.op);
            if changes && sent.repository == 0 && listing.is_none() {
                listing = Some(sent);
            } else if changes && sent.repository == 1 {
                self.answer(&sent, page(Box::new([]), false));
            } else {
                others.push(sent).expect("room");
            }
        }
        let listing = listing.expect("a pass lists the changes");
        let told = self.answer(&listing, page(copies(changes), false));
        let mut sent = others;
        for call in self.send() {
            sent.push(call).expect("room");
        }
        (told, sent.into_boxed())
    }

    fn facts(&mut self) -> Box<[Fact]> {
        let mut facts = List::with_capacity(256);
        for _ in 0..256_u32 {
            let Some(fact) = self.model.pop_fact() else { break };
            facts.push(fact).expect("room");
        }
        facts.into_boxed()
    }
}

/// Whether `op` lists every item that changed.
fn lists_changes(op: &Op) -> bool {
    match op {
        Op::Items { state: None, label: None, .. } => true,
        Op::Items { .. }
        | Op::Item { .. }
        | Op::Comment { .. }
        | Op::Pull { .. }
        | Op::Reviews { .. }
        | Op::PullFor { .. }
        | Op::Statuses { .. }
        | Op::Permission { .. }
        | Op::Branch { .. }
        | Op::Pages { .. }
        | Op::Page { .. }
        | Op::CreateIssue { .. }
        | Op::Post { .. }
        | Op::EditComment { .. }
        | Op::AddLabels { .. }
        | Op::RemoveLabels { .. }
        | Op::OpenPull { .. }
        | Op::Merge { .. }
        | Op::Remarks { .. }
        | Op::Review { .. }
        | Op::SetReviewers { .. }
        | Op::SetDependencies { .. }
        | Op::Reopen { .. }
        | Op::Close { .. }
        | Op::DeleteBranch { .. }
        | Op::PutPage { .. }
        | Op::DeletePage { .. } => false,
    }
}

fn at(secs: u64) -> Time {
    Time::ZERO.saturating_add(Duration::from_secs(secs))
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    Box::from(text)
}

fn labels(names: &[&[u8]]) -> Box<[Box<[u8]>]> {
    let mut list = List::with_capacity(8);
    for name in names {
        list.push(bytes(name)).expect("room");
    }
    list.into_boxed()
}

fn summary(number: u64, kind: Kind, state: State, carried: &[&[u8]], updated: u64) -> Summary {
    Summary {
        number,
        kind,
        state,
        author: PERSON,
        key: None,
        labels: labels(carried),
        title: bytes(b"title"),
        body: bytes(b"body"),
        updated: at(updated),
    }
}

fn issue(number: u64, carried: &[&[u8]], updated: u64) -> Summary {
    summary(number, Kind::Issue, State::Open, carried, updated)
}

fn copies(summaries: &[Summary]) -> Box<[Summary]> {
    let mut list = List::with_capacity(8);
    for summary in summaries {
        let copy =
            Summary { labels: labels_of(summary), title: bytes(b"title"), body: bytes(b"body"), key: None, ..*summary };
        list.push(copy).expect("room");
    }
    list.into_boxed()
}

fn labels_of(summary: &Summary) -> Box<[Box<[u8]>]> {
    let mut list = List::with_capacity(8);
    for label in &summary.labels {
        list.push(label.clone()).expect("room");
    }
    list.into_boxed()
}

fn comment(id: u64, author: u64) -> Comment {
    Comment { id, author, created: Time::ZERO, revision: id, mark: Mark::None, body: bytes(b"text") }
}

/// The engine's record as the comment `id`, saying `position`. Its revision
/// is the comment's id and the item's number.
fn record(id: u64, number: u64, position: Position) -> Comment {
    let revision = id.saturating_add(number);
    Comment {
        id,
        author: ENGINE,
        created: Time::ZERO,
        revision,
        mark: Mark::Record { position, nonce: 0 },
        body: bytes(b"record"),
    }
}

fn comments(list: &[Comment]) -> Box<[Comment]> {
    let mut copy = List::with_capacity(8);
    for comment in list {
        let mark = match &comment.mark {
            Mark::None => Mark::None,
            Mark::Key { key, person } => Mark::Key { key: key.clone(), person: *person },
            Mark::Record { position, nonce } => Mark::Record { position: *position, nonce: *nonce },
            Mark::Mangled => Mark::Mangled,
        };
        copy.push(Comment { mark, body: comment.body.clone(), ..*comment }).expect("room");
    }
    copy.into_boxed()
}

#[expect(clippy::unnecessary_wraps, reason = "what a call is answered with")]
fn page(items: Box<[Summary]>, more: bool) -> Result<Answer, Error> {
    Ok(Answer::Items { items, more, now: Time::ZERO })
}

#[expect(clippy::unnecessary_wraps, reason = "what a call is answered with")]
fn item_page(item: Summary, comments: Box<[Comment]>, more: bool) -> Result<Answer, Error> {
    Ok(Answer::Item { item, comments, more })
}

fn tracked_listing(page: u32) -> Op {
    Op::Items {
        state: Some(State::Open),
        kind: None,
        label: Some(bytes(TRACKING)),
        author: None,
        since: Time::ZERO,
        page,
    }
}

fn changes(since: u64, page: u32) -> Op {
    Op::Items { state: None, kind: None, label: None, author: None, since: at(since), page }
}

fn item(number: u64) -> Item {
    Item { repository: 0, number }
}

/// Where the base branch of the pull requests is.
const BASE: [u8; 32] = [9; 32];

fn pull(number: u64, commit: [u8; 32], ci: Ci) -> Pull {
    Pull {
        number,
        state: State::Open,
        head: bytes(b"change"),
        base: bytes(b"main"),
        commit,
        base_commit: Some(BASE),
        merged: None,
        mergeable: true,
        ci,
    }
}

/// A page of reviews, each `(id, author, verdict)` on `commit`.
#[expect(clippy::unnecessary_wraps, reason = "what a call is answered with")]
fn reviews(list: &[(u64, u64, Verdict)], commit: [u8; 32], more: bool) -> Result<Answer, Error> {
    let mut reviews = List::with_capacity(8);
    for (id, author, verdict) in list {
        let review = Review { id: *id, author: *author, verdict: *verdict, commit, key: None, body: bytes(b"review") };
        reviews.push(review).expect("room");
    }
    Ok(Answer::Reviews { reviews: reviews.into_boxed(), more })
}

/// The record found by the cold start of an item numbered `number`.
fn found(number: u64) -> Record {
    Record::Found { comment: 100, revision: number.saturating_add(100), position: Position::START }
}

fn announced(number: u64, record: Record) -> Request {
    let view = View { kind: Kind::Issue, labels: labels(&[TRACKING]), record };
    Request::Announced { item: item(number), view }
}

fn news(number: u64, seq: u64, news: News) -> Request {
    Request::Inbox { item: item(number), seq, news }
}

// Starting.

#[test]
fn a_cold_start_reads_the_tracked_items_offers_those_handed_in_and_ends_once() {
    let mut h = Harness::new(LIMITS);
    let told = h.start(&[issue(5, &[TRACKING], 1)], &[issue(6, &[HAND_IN], 2)]);
    assert_eq!(
        *told,
        [announced(5, found(5)), Request::Offered { item: item(6) }, Request::Loaded],
        "the tracked item announced, the handed in offered"
    );
    assert!(h.model.is_tracked(item(5)) && !h.model.is_tracked(item(6)), "only the tracked item is held");
    assert_eq!(h.model.items(), 1, "one item held");
}

#[test]
fn a_record_is_found_past_the_first_page_and_its_position_ends_the_comments_read() {
    let mut h = Harness::new(LIMITS);
    h.fire();
    let listings = h.send();
    h.answer(&listings[0], page(copies(&[issue(5, &[TRACKING], 1)]), false));
    h.answer(&listings[1], page(Box::new([]), false));
    let read = h.send_for(&Op::Item { number: 5, after: 0 });
    // No record on the first page: the next is read.
    let told = h.answer(&read, item_page(issue(5, &[TRACKING], 1), comments(&[comment(1, PERSON)]), true));
    assert!(told.is_empty(), "nothing is told before the record is found");
    let next = h.send_for(&Op::Item { number: 5, after: 1 });
    // The record says the comment 2 was taken: the comments after it on this
    // page are news, the engine's own aside.
    let position = Position { comment: 2, ..Position::START };
    let page = comments(&[comment(2, PERSON), record(3, 5, position), comment(4, PERSON)]);
    let told = h.answer(&next, item_page(issue(5, &[TRACKING], 1), page, false));
    let record = Record::Found { comment: 3, revision: 8, position };
    assert_eq!(
        *told,
        [announced(5, record), news(5, 1, News::Comment { on: 5, id: 4, author: PERSON })],
        "announced, then the news after its position"
    );
}

#[test]
fn a_record_whose_position_is_before_the_page_it_is_on_has_the_comments_between_read_again() {
    let mut h = Harness::new(LIMITS);
    h.fire();
    let listings = h.send();
    h.answer(&listings[0], page(copies(&[issue(5, &[TRACKING], 1)]), false));
    h.answer(&listings[1], page(Box::new([]), false));
    let read = h.send_for(&Op::Item { number: 5, after: 0 });
    h.answer(&read, item_page(issue(5, &[TRACKING], 1), comments(&[comment(1, PERSON), comment(2, PERSON)]), true));
    let next = h.send_for(&Op::Item { number: 5, after: 2 });
    let position = Position { comment: 1, ..Position::START };
    let told = h.answer(&next, item_page(issue(5, &[TRACKING], 1), comments(&[record(3, 5, position)]), false));
    assert_eq!(*told, [announced(5, Record::Found { comment: 3, revision: 8, position })], "announced");
    let again = h.send_one();
    assert_eq!(again.op, Op::Item { number: 5, after: 1 }, "the comments after its position are read");
    let told = h.answer(&again, item_page(issue(5, &[TRACKING], 1), comments(&[comment(2, PERSON)]), false));
    assert_eq!(*told, [news(5, 1, News::Comment { on: 5, id: 2, author: PERSON })], "the comment between is news");
}

#[test]
fn an_item_without_a_record_is_announced_so_and_all_its_comments_are_news() {
    let mut h = Harness::started(LIMITS);
    let told = h.step(Event::Track { item: item(9) });
    assert!(told.is_empty(), "taken in quietly");
    let read = h.send_one();
    assert_eq!(read.op, Op::Item { number: 9, after: 0 }, "read from its first comment");
    let told = h.answer(&read, item_page(issue(9, &[HAND_IN], 4), comments(&[comment(1, PERSON)]), false));
    let view = View { kind: Kind::Issue, labels: labels(&[HAND_IN]), record: Record::Missing };
    assert_eq!(
        *told,
        [Request::Announced { item: item(9), view }, news(9, 1, News::Comment { on: 9, id: 1, author: PERSON })],
        "announced with no record, and every comment is news"
    );
    assert!(h.send().is_empty(), "read once");
}

#[test]
fn a_mangled_record_is_announced_as_such_and_news_starts_after_it() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    let mangled =
        Comment { id: 2, author: ENGINE, created: Time::ZERO, revision: 20, mark: Mark::Mangled, body: bytes(b"?") };
    let page = comments(&[comment(1, PERSON), mangled, comment(3, PERSON)]);
    let told = h.answer(&read, item_page(issue(9, &[TRACKING], 4), page, false));
    let view =
        View { kind: Kind::Issue, labels: labels(&[TRACKING]), record: Record::Mangled { comment: 2, revision: 20 } };
    assert_eq!(
        *told,
        [Request::Announced { item: item(9), view }, news(9, 1, News::Comment { on: 9, id: 3, author: PERSON })],
        "held for a person by its parent; what came after it is news"
    );
}

#[test]
fn a_record_of_someone_else_is_not_the_engines() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    let forged = Comment {
        id: 2,
        author: PERSON,
        created: Time::ZERO,
        revision: 2,
        mark: Mark::Record { position: Position::START, nonce: 0 },
        body: bytes(b"!"),
    };
    let told = h.answer(&read, item_page(issue(9, &[TRACKING], 4), comments(&[forged]), false));
    let view = View { kind: Kind::Issue, labels: labels(&[TRACKING]), record: Record::Missing };
    assert_eq!(
        *told,
        [Request::Announced { item: item(9), view }, news(9, 1, News::Comment { on: 9, id: 2, author: PERSON })],
        "a person's record block is a comment"
    );
}

#[test]
fn a_tracked_item_closed_by_the_time_it_is_read_leaves() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    let closed = summary(9, Kind::Issue, State::Closed, &[TRACKING], 4);
    let told = h.answer(&read, item_page(closed, Box::new([]), false));
    assert_eq!(*told, [Request::Left { item: item(9), why: Why::Closed }], "it left");
    assert!(!h.model.is_tracked(item(9)), "and is not held");
}

// Keeping up.

#[test]
fn a_pass_lists_the_changes_since_the_newest_time_seen_and_reads_what_changed() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let (told, sent) = h.pass(30, &[issue(5, &[TRACKING], 20), issue(8, &[], 25)]);
    assert!(told.is_empty(), "nothing to tell yet: {told:?}");
    assert_eq!(sent.len(), 1, "the changed item is read: {sent:?}");
    assert_eq!(sent[0].op, Op::Item { number: 5, after: 100 }, "after the last comment passed");
    let told = h.answer(&sent[0], item_page(issue(5, &[TRACKING], 20), comments(&[comment(101, PERSON)]), false));
    assert_eq!(*told, [news(5, 1, News::Comment { on: 5, id: 101, author: PERSON })], "news");
    // The next pass starts at the newest time seen, inclusive.
    h.at(60);
    h.fire();
    let sent = h.send();
    assert_eq!(sent[0].op, changes(25, 1), "from the newest updated time seen");
}

#[test]
fn an_item_listed_within_the_second_it_changed_is_read_once_more_once_that_second_passed() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    // Listed in the very second it changed: a change may follow in it.
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 30)]);
    assert_eq!(sent.len(), 1, "read for the change");
    h.answer(&sent[0], item_page(issue(5, &[TRACKING], 30), Box::new([]), false));
    let (_, sent) = h.pass(60, &[issue(5, &[TRACKING], 30)]);
    assert_eq!(sent.len(), 1, "read once more, by a listing made after that second");
    h.answer(&sent[0], item_page(issue(5, &[TRACKING], 30), Box::new([]), false));
    let (_, sent) = h.pass(90, &[issue(5, &[TRACKING], 30)]);
    assert!(sent.is_empty(), "and then no more: {sent:?}");
    // Listed when the second it changed had passed: read once.
    let (_, sent) = h.pass(120, &[issue(5, &[TRACKING], 100)]);
    h.answer(&sent[0], item_page(issue(5, &[TRACKING], 100), Box::new([]), false));
    let (_, sent) = h.pass(150, &[issue(5, &[TRACKING], 100)]);
    assert!(sent.is_empty(), "the read after it found all of it: {sent:?}");
}

#[test]
fn a_listing_made_in_the_same_second_does_not_settle_what_it_shows() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 30)]);
    h.answer(&sent[0], item_page(issue(5, &[TRACKING], 30), Box::new([]), false));
    // Another pass, a hint after, in the same second as the change, by the
    // forge's clock: it settles nothing.
    h.at(32);
    h.step(Event::Hint { repository: 0, item: Some(5), commit: None, branch: None });
    h.fire();
    let listing = h.send_for(&changes(30, 1));
    let made = Time::ZERO.saturating_add(Duration::from_millis(30_500));
    let listed = Ok(Answer::Items { items: copies(&[issue(5, &[TRACKING], 30)]), more: false, now: made });
    h.answer(&listing, listed);
    assert!(h.send().is_empty(), "not read: the second has not passed for the forge");
    let (_, sent) = h.pass(62, &[issue(5, &[TRACKING], 30)]);
    assert_eq!(sent.len(), 1, "read once more by a listing made after it: {sent:?}");
}

#[test]
fn a_listing_pages_by_time_and_by_number_only_within_one_time() {
    let mut h = Harness::new(LIMITS);
    h.start(&[], &[]);
    h.at(30);
    h.fire();
    let sent = h.send();
    let first = &sent[0];
    assert_eq!(first.op, changes(0, 1), "the first page");
    h.answer(first, page(copies(&[issue(1, &[], 3), issue(2, &[], 4), issue(3, &[], 4)]), true));
    let next = h.send_one();
    assert_eq!(next.op, changes(4, 1), "from the last item's time, inclusive");
    h.answer(&next, page(copies(&[issue(2, &[], 4), issue(3, &[], 4), issue(4, &[], 4)]), true));
    let next = h.send_one();
    assert_eq!(next.op, changes(4, 2), "a page all of one time: the next page by number");
    h.answer(&next, page(copies(&[issue(5, &[], 6)]), false));
    h.at(60);
    h.fire();
    assert_eq!(h.send()[0].op, changes(6, 1), "the next pass from the newest time");
}

#[test]
fn a_pass_that_paged_by_number_and_moved_meanwhile_lists_that_time_again() {
    let mut h = Harness::started(LIMITS);
    h.at(30);
    h.fire();
    let sent = h.send();
    h.answer(&sent[0], page(copies(&[issue(1, &[], 4), issue(2, &[], 4), issue(3, &[], 4)]), true));
    let next = h.send_one();
    assert_eq!(next.op, changes(4, 1), "by time");
    h.answer(&next, page(copies(&[issue(1, &[], 4), issue(2, &[], 4), issue(3, &[], 4)]), true));
    let next = h.send_one();
    assert_eq!(next.op, changes(4, 2), "by number within one time");
    // An item changed as the pass ran, as the forge made its first page or
    // after: one of the time it paged at may have shifted onto the page it
    // had read.
    h.answer(&next, page(copies(&[issue(4, &[], 4), issue(1, &[], 30)]), false));
    h.at(60);
    h.fire();
    assert_eq!(h.send()[0].op, changes(4, 1), "from the time it paged at, again");
}

#[test]
fn labels_changed_are_told_and_a_closed_item_leaves() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let (told, sent) = h.pass(30, &[issue(5, &[TRACKING, b"bug"], 20)]);
    assert_eq!(*told, [Request::Changed { item: item(5), labels: labels(&[TRACKING, b"bug"]) }], "labels told");
    h.answer(&sent[0], item_page(issue(5, &[TRACKING, b"bug"], 20), Box::new([]), false));
    let (told, sent) = h.pass(60, &[summary(5, Kind::Issue, State::Closed, &[TRACKING], 50)]);
    assert_eq!(*told, [Request::Left { item: item(5), why: Why::Closed }], "it left");
    assert!(sent.is_empty(), "and is not read");
    assert!(!h.model.is_tracked(item(5)), "nor held");
}

#[test]
fn an_item_found_carrying_the_tracking_label_is_admitted_and_one_handed_in_offered() {
    let mut h = Harness::new(LIMITS);
    h.start(&[], &[]);
    let (told, sent) = h.pass(30, &[issue(5, &[TRACKING], 20), issue(6, &[HAND_IN], 21)]);
    assert_eq!(*told, [Request::Offered { item: item(6) }], "offered");
    assert_eq!(sent[0].op, Op::Item { number: 5, after: 0 }, "the tracked one is read for its record");
}

#[test]
fn a_hint_brings_the_next_pass_forward_and_a_pass_running_is_followed_soon() {
    let mut h = Harness::new(LIMITS);
    h.start(&[], &[]);
    assert_eq!(h.model.next_deadline(), Some(at(30)), "a poll after the start began");
    h.at(10);
    h.step(Event::Hint { repository: 0, item: Some(3), commit: None, branch: None });
    assert_eq!(h.model.next_deadline(), Some(at(10)), "at once: the last pass began long enough ago");
    h.fire();
    let listing = h.send_one();
    h.at(11);
    h.step(Event::Hint { repository: 0, item: None, commit: None, branch: None });
    h.answer(&listing, page(Box::new([]), false));
    assert_eq!(h.model.next_deadline(), Some(at(12)), "the next follows soon after the one that ran");
}

/// Links item 5 to the pull request 9 at `HEAD` with `ci`, its verdicts
/// none and its comments none, and returns what was told.
fn linked(h: &mut Harness, ci: Ci) -> Box<[Request]> {
    h.step(Event::Link { item: item(5), pull: Some(9) });
    let read = h.send_one();
    assert_eq!(read.op, Op::Pull { number: 9 }, "the linked pull request is read");
    let told = h.answer(&read, Ok(Answer::Pull(pull(9, HEAD, ci))));
    let verdicts = h.send_one();
    assert_eq!(verdicts.op, Op::Reviews { number: 9, page: 1 }, "then its verdicts");
    assert!(h.answer(&verdicts, reviews(&[], HEAD, false)).is_empty(), "none");
    let remarks = h.send_one();
    assert_eq!(remarks.op, Op::Item { number: 9, after: 0 }, "then its comments");
    assert!(h.answer(&remarks, item_page(summary(9, Kind::Pull, State::Open, &[], 1), Box::new([]), false)).is_empty());
    told
}

#[test]
fn a_pull_request_is_read_on_a_backoff_of_its_own_and_on_a_hint_whatever_its_state() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let told = linked(&mut h, Ci::Pending);
    let level = News::Pull { commit: HEAD, ci: Ci::Pending, open: true, merged: None, mergeable: true };
    assert_eq!(*told, [news(5, 1, level)], "its head and CI are news");
    let (_, sent) = h.pass(30, &[]);
    assert_eq!(sent.len(), 1, "read again with the pass: {sent:?}");
    let told = h.answer(&sent[0], Ok(Answer::Pull(pull(9, HEAD, Ci::Passed))));
    let level = News::Pull { commit: HEAD, ci: Ci::Passed, open: true, merged: None, mergeable: true };
    assert_eq!(*told, [news(5, 2, level)], "CI passed");
    let (_, sent) = h.pass(60, &[]);
    assert_eq!(sent.len(), 1, "settled CI is read again: it may run again");
    assert!(h.answer(&sent[0], Ok(Answer::Pull(pull(9, HEAD, Ci::Passed)))).is_empty(), "unchanged");
    let (_, sent) = h.pass(90, &[]);
    assert_eq!(sent.len(), 1, "a poll after");
    h.answer(&sent[0], Ok(Answer::Pull(pull(9, HEAD, Ci::Passed))));
    let (_, sent) = h.pass(120, &[]);
    assert!(sent.is_empty(), "unchanged twice: twice as long before the next: {sent:?}");
    let (_, sent) = h.pass(150, &[]);
    assert_eq!(sent.len(), 1, "then read: {sent:?}");
    let mut moved = pull(9, HEAD, Ci::Passed);
    moved.mergeable = false;
    moved.base_commit = Some(OTHER);
    let told = h.answer(&sent[0], Ok(Answer::Pull(moved)));
    let level = News::Pull { commit: HEAD, ci: Ci::Passed, open: true, merged: None, mergeable: false };
    assert_eq!(*told, [news(5, 3, level)], "its base moved: it no longer merges cleanly");
    h.step(Event::Hint { repository: 0, item: None, commit: Some(HEAD), branch: None });
    let read = h.send_one();
    assert_eq!(read.op, Op::Pull { number: 9 }, "a status on its head reads it again");
    h.answer(&read, Ok(Answer::Pull(pull(9, HEAD, Ci::Failed))));
    let level =
        Level { number: 9, commit: HEAD, base: Some(BASE), ci: Ci::Failed, open: true, merged: None, mergeable: true };
    assert_eq!(h.model.pull(item(5)), Some(level), "the level as last read");
    h.step(Event::Hint { repository: 0, item: None, commit: None, branch: Some(bytes(b"main")) });
    assert_eq!(h.send_one().op, Op::Pull { number: 9 }, "a push to its base reads it again");
}

/// Sends what is ready, among `sent` and after, answering every read of the
/// pull request 9 with it at `commit` with `ci`, until no more is; returns
/// what was told, and the other calls sent.
fn pull_reads(h: &mut Harness, sent: Box<[Sent]>, commit: [u8; 32], ci: Ci) -> (Box<[Request]>, List<Sent>) {
    let mut told = List::with_capacity(16);
    let mut others = List::with_capacity(16);
    let mut sent = sent;
    for _ in 0..8_u32 {
        if sent.is_empty() {
            break;
        }
        for call in sent {
            if call.op == (Op::Pull { number: 9 }) {
                for request in h.answer(&call, Ok(Answer::Pull(pull(9, commit, ci)))) {
                    told.push(request).expect("room");
                }
            } else {
                others.push(call).expect("room");
            }
        }
        sent = h.send();
    }
    (told.into_boxed(), others)
}

#[test]
fn a_pull_request_is_read_whatever_room_the_inbox_has_and_told_when_there_is() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    linked(&mut h, Ci::Pending);
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 20)]);
    let (_, others) = pull_reads(&mut h, sent, HEAD, Ci::Pending);
    let read = others.get(0).expect("the item read");
    assert_eq!(read.op, Op::Item { number: 5, after: 100 });
    let page = comments(&[comment(101, PERSON)]);
    h.answer(read, item_page(issue(5, &[TRACKING], 20), page, false));
    // The inbox's last place is the level's.
    h.step(Event::Hint { repository: 0, item: None, commit: Some(HEAD), branch: None });
    let read = h.send_one();
    let told = h.answer(&read, Ok(Answer::Pull(pull(9, HEAD, Ci::Failed))));
    let failed = News::Pull { commit: HEAD, ci: Ci::Failed, open: true, merged: None, mergeable: true };
    assert_eq!(*told, [news(5, 3, failed)], "told in the place kept for it");
    // The inbox is full: the pull request is read all the same.
    h.step(Event::Hint { repository: 0, item: None, commit: Some(HEAD), branch: None });
    let read = h.send_one();
    assert!(h.answer(&read, Ok(Answer::Pull(pull(9, HEAD, Ci::Passed)))).is_empty(), "no room to tell it");
    let level = h.model.pull(item(5)).expect("read");
    assert_eq!(level.ci, Ci::Passed, "the level is fresh");
    let told = h.step(Event::Took { item: item(5), through: 3 });
    let passed = News::Pull { commit: HEAD, ci: Ci::Passed, open: true, merged: None, mergeable: true };
    assert_eq!(*told, [news(5, 4, passed)], "told once there is room");
}

#[test]
fn news_the_parent_had_no_room_for_is_told_again_as_it_was() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    linked(&mut h, Ci::Pending);
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 20)]);
    let (_, others) = pull_reads(&mut h, sent, HEAD, Ci::Pending);
    let read = others.get(0).expect("the item read");
    let page = comments(&[comment(101, PERSON)]);
    let first = h.answer(read, item_page(issue(5, &[TRACKING], 20), page, false));
    let message = News::Comment { on: 5, id: 101, author: PERSON };
    assert!(first.contains(&news(5, 2, message)), "the message is told: {first:?}");
    // The parent took the first, and had no room for the message: it is
    // told again, as it was, and nothing it took.
    h.step(Event::Took { item: item(5), through: 1 });
    let told = h.step(Event::Retell { item: item(5), from: 2 });
    assert_eq!(*told, [news(5, 2, message)], "told again from where the parent had no room");
    assert!(h.step(Event::Retell { item: item(5), from: 3 }).is_empty(), "nothing held from there");
}

#[test]
fn the_verdicts_on_the_head_are_level_state_told_when_they_move() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    linked(&mut h, Ci::Passed);
    h.step(Event::Took { item: item(5), through: 1 });
    let (_, sent) = h.pass(30, &[summary(9, Kind::Pull, State::Open, &[], 25)]);
    let (told, others) = pull_reads(&mut h, sent, HEAD, Ci::Passed);
    assert!(told.is_empty(), "its state is the same");
    let first = others.get(0).expect("its verdicts read");
    assert_eq!(first.op, Op::Reviews { number: 9, page: 1 }, "its verdicts, a page at a time");
    let page = [(3, PERSON, Verdict::Approve), (4, 8, Verdict::RequestChanges), (5, ENGINE, Verdict::Approve)];
    assert!(h.answer(first, reviews(&page, HEAD, true)).is_empty(), "nothing told before the last page");
    let second = h.send_one();
    assert_eq!(second.op, Op::Reviews { number: 9, page: 2 });
    let page = [(6, PERSON, Verdict::Comment), (7, 8, Verdict::Approve)];
    let told = h.answer(&second, reviews(&page, HEAD, false));
    assert_eq!(*told, [news(5, 2, News::Reviews { commit: HEAD })], "they moved: told once");
    let verdicts =
        [Reviewed { author: PERSON, verdict: Verdict::Approve }, Reviewed { author: 8, verdict: Verdict::Approve }];
    assert_eq!(h.model.reviews(item(5)), Some(verdicts.as_slice()), "the latest of each, the engine's aside");
    let remarks = h.send_one();
    h.answer(&remarks, item_page(summary(9, Kind::Pull, State::Open, &[], 25), Box::new([]), false));
    h.step(Event::Took { item: item(5), through: 2 });
    // A pending review submitted lands earlier: the same verdicts, read in
    // another order, are no news.
    let (_, sent) = h.pass(60, &[summary(9, Kind::Pull, State::Open, &[], 55)]);
    let (_, others) = pull_reads(&mut h, sent, HEAD, Ci::Passed);
    let read = others.get(0).expect("its verdicts read");
    let page = [(2, 8, Verdict::Approve), (3, PERSON, Verdict::Approve), (5, ENGINE, Verdict::RequestChanges)];
    assert!(h.answer(read, reviews(&page, HEAD, false)).is_empty(), "no news");
    let remarks = h.send_one();
    h.answer(&remarks, item_page(summary(9, Kind::Pull, State::Open, &[], 55), Box::new([]), false));
    // A new head: the verdicts on the old one say nothing of it.
    let (_, sent) = h.pass(90, &[summary(9, Kind::Pull, State::Open, &[], 85)]);
    let (told, others) = pull_reads(&mut h, sent, OTHER, Ci::Pending);
    let level = News::Pull { commit: OTHER, ci: Ci::Pending, open: true, merged: None, mergeable: true };
    assert_eq!(*told, [news(5, 3, level)], "the head moved");
    let read = others.get(0).expect("its verdicts read");
    let told = h.answer(read, reviews(&page, HEAD, false));
    assert_eq!(*told, [news(5, 4, News::Reviews { commit: OTHER })], "no verdicts on the new head");
    assert_eq!(h.model.reviews(item(5)), Some([].as_slice()));
}

#[test]
fn a_persons_comment_on_the_linked_pull_request_is_news_with_a_position_of_its_own() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    linked(&mut h, Ci::Passed);
    h.step(Event::Took { item: item(5), through: 1 });
    let (_, sent) = h.pass(30, &[summary(9, Kind::Pull, State::Open, &[], 25)]);
    let (_, others) = pull_reads(&mut h, sent, HEAD, Ci::Passed);
    let read = others.get(0).expect("its verdicts read");
    h.answer(read, reviews(&[], HEAD, false));
    let remarks = h.send_one();
    assert_eq!(remarks.op, Op::Item { number: 9, after: 0 }, "the pull request's comments");
    let page = comments(&[comment(201, ENGINE), comment(202, PERSON)]);
    let told = h.answer(&remarks, item_page(summary(9, Kind::Pull, State::Open, &[], 25), page, false));
    assert_eq!(*told, [news(5, 2, News::Comment { on: 9, id: 202, author: PERSON })], "news for the item");
    // Its state, owed again by the listing as it was read, is read last.
    let sent = h.send();
    let (_, others) = pull_reads(&mut h, sent, HEAD, Ci::Passed);
    assert!(others.is_empty(), "nothing else owed");
    h.step(Event::Took { item: item(5), through: 2 });
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload: Token::new(2) }, resumed: None });
    let check = h.send_one();
    h.answer(&check, Ok(Answer::Comment(record(100, 5, Position::START))));
    let edit = h.send_one();
    let Op::EditComment { body: Body::Record { position, .. }, .. } = edit.op else {
        panic!("the record edited: {edit:?}");
    };
    assert_eq!(position.pull_comment, 202, "the record carries where the pull request's comments were taken");
    assert_eq!(position.head, Some(HEAD));
}

#[test]
fn relinking_drops_what_was_held_of_the_other_pull_request() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    linked(&mut h, Ci::Passed);
    h.step(Event::Link { item: item(5), pull: Some(11) });
    let read = h.send_one();
    assert_eq!(read.op, Op::Pull { number: 11 }, "the new one is read");
    assert_eq!(h.model.pull(item(5)), None, "nothing known of it yet");
    // The parent takes the news of the old one: the position carries nothing
    // of it.
    h.step(Event::Took { item: item(5), through: 1 });
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload: Token::new(2) }, resumed: None });
    let check = h.send_for(&Op::Comment { number: 5, id: 100 });
    h.answer(&check, Ok(Answer::Comment(record(100, 5, Position::START))));
    let edit = h.send_one();
    let Op::EditComment { body: Body::Record { position, .. }, .. } = edit.op else {
        panic!("the record edited: {edit:?}");
    };
    assert_eq!(position, Position { comment: 100, ..Position::START }, "nothing of the old pull request is taken");
    h.answer(&edit, Ok(Answer::Edited { revision: 7 }));
    let told = h.answer(&read, Ok(Answer::Pull(pull(11, OTHER, Ci::Pending))));
    let level = News::Pull { commit: OTHER, ci: Ci::Pending, open: true, merged: None, mergeable: true };
    assert_eq!(*told, [news(5, 2, level)], "the new one's state is news");
}

#[test]
fn a_full_inbox_waits_until_the_parent_takes_news() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 20)]);
    let page = comments(&[comment(101, PERSON), comment(102, ENGINE), comment(103, PERSON)]);
    let told = h.answer(&sent[0], item_page(issue(5, &[TRACKING], 20), page, true));
    let expected = [
        news(5, 1, News::Comment { on: 5, id: 101, author: PERSON }),
        news(5, 2, News::Comment { on: 5, id: 103, author: PERSON }),
    ];
    assert_eq!(*told, expected, "as much news as the inbox holds, its last place kept");
    assert!(h.send().is_empty(), "nothing more is read while it is full");
    h.step(Event::Took { item: item(5), through: 1 });
    let read = h.send_one();
    assert_eq!(read.op, Op::Item { number: 5, after: 103 }, "read on from the last comment told");
    let page = comments(&[comment(104, PERSON), comment(106, PERSON)]);
    let told = h.answer(&read, item_page(issue(5, &[TRACKING], 20), page, false));
    assert_eq!(*told, [news(5, 3, News::Comment { on: 5, id: 104, author: PERSON })], "news resumes");
    h.step(Event::Took { item: item(5), through: 3 });
    let read = h.send_one();
    assert_eq!(read.op, Op::Item { number: 5, after: 104 });
    let told = h.answer(&read, item_page(issue(5, &[TRACKING], 20), comments(&[comment(106, PERSON)]), false));
    assert_eq!(*told, [news(5, 4, News::Comment { on: 5, id: 106, author: PERSON })], "and on");
}

#[test]
fn an_item_the_forge_no_longer_has_leaves_as_missing() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    let told = h.answer(&read, Err(Error::Missing));
    assert_eq!(*told, [Request::Left { item: item(9), why: Why::Missing }], "never there");
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 20)]);
    let told = h.answer(&sent[0], Err(Error::Missing));
    assert_eq!(*told, [Request::Left { item: item(5), why: Why::Missing }], "deleted, or moved: not finished");
}

#[test]
fn an_item_the_forge_forbids_is_read_again_and_told_once_the_attempts_run_out() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 20)]);
    let mut read = sent.into_iter().next().expect("the item read");
    for attempt in 1..=4_u64 {
        let told = h.answer(&read, Err(Error::Forbidden));
        if attempt == 3 {
            assert_eq!(*told, [Request::Forbidden { item: item(5) }], "told once the attempts ran out");
        } else {
            assert!(told.is_empty(), "a backoff, and nothing told: {told:?}");
        }
        assert!(h.model.is_tracked(item(5)), "held all the same");
        h.at(30 + attempt * 10);
        h.fire();
        read = h.send_for(&Op::Item { number: 5, after: 100 });
    }
    let told = h.answer(&read, item_page(issue(5, &[TRACKING], 20), comments(&[comment(101, PERSON)]), false));
    assert_eq!(*told, [news(5, 1, News::Comment { on: 5, id: 101, author: PERSON })], "news, once it is shown");
}

#[test]
fn labels_beyond_the_limits_are_cut_keeping_the_engines() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let long: &[u8] = &[b'x'; 17];
    let many = issue(5, &[b"a", b"b", long, b"c", HAND_IN, TRACKING], 20);
    let (told, _) = h.pass(30, &[many]);
    let kept = labels(&[HAND_IN, TRACKING, b"a"]);
    assert_eq!(*told, [Request::Changed { item: item(5), labels: kept }], "the engine's first, as many as fit");
}

#[test]
fn the_slow_pass_reads_a_few_candidates_a_page_and_items_held_it_never_shows() {
    let limits = Limits { probes: 1, ..LIMITS };
    let mut h = Harness::new(limits);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let mut probed = List::with_capacity(4);
    for cycle in 0..2_u64 {
        h.at(600 * (cycle + 1));
        h.fire();
        let listing =
            Op::Items { state: Some(State::Open), kind: None, label: None, author: None, since: Time::ZERO, page: 1 };
        let mut slow = None;
        for call in h.send() {
            if call.op == listing && call.repository == 0 {
                slow = Some(call);
            } else {
                h.answer(&call, page(Box::new([]), false));
            }
        }
        let slow = slow.expect("the slow pass lists the open items");
        // Item 5, held, is not shown: it may be gone.
        h.answer(&slow, page(copies(&[issue(3, &[], 1), issue(4, &[], 1)]), false));
        let sent = h.send();
        for call in sent {
            match call.op {
                Op::Item { number: 5, after: 100 } => {
                    // Admitted during the first cycle, it is owed the second.
                    assert_eq!(cycle, 1, "read once, as the cycle ends");
                    h.answer(&call, Err(Error::Missing));
                }
                Op::Item { number, after: 0 } => {
                    probed.push(number).expect("room");
                    h.answer(&call, item_page(issue(number, &[], 1), Box::new([]), false));
                }
                Op::Items { .. }
                | Op::Item { .. }
                | Op::Comment { .. }
                | Op::Pull { .. }
                | Op::Reviews { .. }
                | Op::PullFor { .. }
                | Op::Statuses { .. }
                | Op::Permission { .. }
                | Op::Branch { .. }
                | Op::Pages { .. }
                | Op::Page { .. }
                | Op::CreateIssue { .. }
                | Op::Post { .. }
                | Op::EditComment { .. }
                | Op::AddLabels { .. }
                | Op::RemoveLabels { .. }
                | Op::OpenPull { .. }
                | Op::Merge { .. }
                | Op::Remarks { .. }
                | Op::Review { .. }
                | Op::SetReviewers { .. }
                | Op::SetDependencies { .. }
                | Op::Reopen { .. }
                | Op::Close { .. }
                | Op::DeleteBranch { .. }
                | Op::PutPage { .. }
                | Op::DeletePage { .. } => panic!("the slow pass: {call:?}"),
            }
        }
        assert!(h.send().is_empty(), "one probe a page");
    }
    assert_eq!(probed.as_slice(), [4, 3], "a different one each cycle");
    assert!(!h.model.is_tracked(item(5)), "gone");
}

#[test]
fn the_slow_pass_finds_an_item_whose_tracking_label_was_removed() {
    let mut h = Harness::new(LIMITS);
    h.start(&[], &[]);
    h.at(600);
    h.fire();
    let mut slow = None;
    for sent in h.send() {
        match sent.op {
            Op::Items { state: Some(State::Open), label: None, since, page: 1, .. } if since == Time::ZERO => {
                if sent.repository == 0 {
                    slow = Some(sent);
                } else {
                    h.answer(&sent, page(Box::new([]), false));
                }
            }
            Op::Items { .. }
            | Op::Item { .. }
            | Op::Comment { .. }
            | Op::Pull { .. }
            | Op::Reviews { .. }
            | Op::PullFor { .. }
            | Op::Statuses { .. }
            | Op::Permission { .. }
            | Op::Branch { .. }
            | Op::Pages { .. }
            | Op::Page { .. }
            | Op::CreateIssue { .. }
            | Op::Post { .. }
            | Op::EditComment { .. }
            | Op::AddLabels { .. }
            | Op::RemoveLabels { .. }
            | Op::OpenPull { .. }
            | Op::Merge { .. }
            | Op::Remarks { .. }
            | Op::Review { .. }
            | Op::SetReviewers { .. }
            | Op::SetDependencies { .. }
            | Op::Reopen { .. }
            | Op::Close { .. }
            | Op::DeleteBranch { .. }
            | Op::PutPage { .. }
            | Op::DeletePage { .. } => {
                h.answer(&sent, page(Box::new([]), false));
            }
        }
    }
    let slow = slow.expect("the slow pass lists the open items");
    h.answer(&slow, page(copies(&[issue(3, &[], 1), issue(4, &[TRACKING], 1), issue(5, &[b"bug"], 1)]), false));
    let probe = h.send_one();
    assert_eq!(probe.op, Op::Item { number: 3, after: 0 }, "a candidate is read for a record");
    h.answer(&probe, item_page(issue(3, &[], 1), comments(&[comment(1, PERSON)]), false));
    let probe = h.send_one();
    assert_eq!(probe.op, Op::Item { number: 5, after: 0 }, "the next: the labelled one is the listings'");
    h.answer(&probe, item_page(issue(5, &[b"bug"], 1), comments(&[record(2, 5, Position::START)]), false));
    let read = h.send_one();
    assert_eq!(read.op, Op::Item { number: 5, after: 0 }, "found: admitted, and read");
    assert!(h.model.is_tracked(item(5)), "held");
}

// The entrance.

#[test]
fn a_full_working_set_refuses_and_lists_the_labels_again_once_there_is_room() {
    let limits = Limits { items: 1, ..LIMITS };
    let mut h = Harness::new(limits);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let told = h.step(Event::Track { item: item(6) });
    assert_eq!(*told, [Request::Full { item: item(6) }], "refused at the entrance");
    let (_, sent) = h.pass(30, &[issue(7, &[TRACKING], 20)]);
    assert!(sent.is_empty(), "no room for one found either: {sent:?}");
    let (told, _) = h.pass(60, &[summary(5, Kind::Issue, State::Closed, &[TRACKING], 50)]);
    assert_eq!(*told, [Request::Left { item: item(5), why: Why::Closed }], "room frees");
    assert_eq!(h.model.next_deadline(), Some(at(62)), "the labels are listed again soon");
    h.at(62);
    assert_eq!(*h.fire(), [Request::Room], "the parent hears there is room again, once");
    let sent = h.send();
    assert_eq!(sent[0].op, tracked_listing(1), "the tracking label first");
    assert!(h.step(Event::Took { item: item(5), through: 0 }).is_empty(), "told once");
}

#[test]
fn reads_and_writes_beyond_the_limits_or_the_room_are_refused_at_the_entrance() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    let long = Read::Branch { repository: 0, branch: bytes(&[b'b'; 17]) };
    assert_eq!(*h.step(Event::Read { owner, read: long }), [Request::Read { owner, result: Err(Failure::Invalid) }]);
    let elsewhere = Read::Pull { item: Item { repository: 2, number: 1 } };
    assert_eq!(
        *h.step(Event::Read { owner, read: elsewhere }),
        [Request::Read { owner, result: Err(Failure::Invalid) }]
    );
    h.step(Event::Read { owner, read: Read::Pull { item: item(1) } });
    h.step(Event::Read { owner, read: Read::Pull { item: item(2) } });
    let busy = h.step(Event::Read { owner, read: Read::Pull { item: item(3) } });
    assert_eq!(*busy, [Request::Read { owner, result: Err(Failure::Busy) }], "two reads at once");
    let labels = labels(&[b"a", b"b", b"c", b"d"]);
    let many = Write::SetLabels { item: item(1), labels };
    let refused = h.step(Event::Write { owner, write: many, resumed: None });
    assert_eq!(*refused, [Request::Wrote { owner, result: Err(Failure::Invalid) }], "more labels than the limits");
    let record = Write::Record { item: item(1), payload: Token::new(2) };
    let unknown = h.step(Event::Write { owner, write: record, resumed: None });
    assert_eq!(*unknown, [Request::Wrote { owner, result: Err(Failure::Unknown) }], "a record of an item not held");
    for number in 1..=3 {
        h.step(Event::Write { owner, write: Write::Close { item: item(number) }, resumed: None });
    }
    let busy = h.step(Event::Write { owner, write: Write::Close { item: item(4) }, resumed: None });
    assert_eq!(*busy, [Request::Wrote { owner, result: Err(Failure::Busy) }], "three writes at once");
}

// Fresh reads.

#[test]
fn fresh_reads_go_out_first_are_tried_again_and_answered_once() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(4);
    h.step(Event::Write { owner: Token::new(5), write: Write::Close { item: item(1) }, resumed: None });
    h.step(Event::Read { owner, read: Read::Pull { item: item(9) } });
    let sent = h.send();
    assert_eq!(sent[0].op, Op::Pull { number: 9 }, "the read before the write: {sent:?}");
    assert_eq!(sent[1].op, Op::Close { number: 1 }, "then the write");
    assert!(h.answer(&sent[0], Err(Error::Unavailable)).is_empty(), "tried again");
    h.at(2);
    h.fire();
    let again = h.send_one();
    let told = h.answer(&again, Ok(Answer::Pull(pull(9, HEAD, Ci::None))));
    assert_eq!(*told, [Request::Read { owner, result: Ok(Answer::Pull(pull(9, HEAD, Ci::None))) }], "answered");
}

#[test]
fn a_fresh_read_that_keeps_failing_gives_up_after_its_attempts() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(4);
    h.step(Event::Read { owner, read: Read::Page { repository: 1, name: bytes(b"notes") } });
    for attempt in 1..=3_u64 {
        let sent = h.send_one();
        assert_eq!(sent.repository, 1, "of its repository");
        let told = h.answer(&sent, Err(Error::Timeout));
        if attempt == 3 {
            assert_eq!(*told, [Request::Read { owner, result: Err(Failure::Forge(Error::Timeout)) }], "gave up");
        } else {
            assert!(told.is_empty(), "tried again");
            h.at(attempt.saturating_mul(9));
            h.fire();
        }
    }
    let missing = Event::Read { owner, read: Read::Page { repository: 1, name: bytes(b"gone") } };
    h.step(missing);
    let sent = h.send_one();
    let told = h.answer(&sent, Err(Error::Missing));
    assert_eq!(*told, [Request::Read { owner, result: Err(Failure::Forge(Error::Missing)) }], "at once");
}

// Writes.

fn set_labels(names: &[&[u8]]) -> Write {
    Write::SetLabels { item: item(1), labels: labels(names) }
}

/// The nonce a record or a wiki page written carries.
fn nonce(op: &Op) -> u64 {
    if let Op::PutPage { nonce, .. } = op {
        return *nonce;
    }
    let (Op::Post { body, .. } | Op::EditComment { body, .. }) = op else {
        panic!("a record written: {op:?}");
    };
    let Body::Record { nonce, .. } = body else {
        panic!("a record written: {op:?}");
    };
    *nonce
}

/// The engine's record as the comment `id` on the item `number`, saying
/// `position`, made by the write of `nonce`, at `revision`.
fn written(id: u64, revision: u64, position: Position, nonce: u64) -> Comment {
    Comment {
        id,
        author: ENGINE,
        created: Time::ZERO,
        revision,
        mark: Mark::Record { position, nonce },
        body: bytes(b"record"),
    }
}

#[test]
fn writes_about_one_item_go_one_at_a_time_in_order() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Write { owner: Token::new(1), write: set_labels(&[TRACKING, b"a"]), resumed: None });
    h.step(Event::Write { owner: Token::new(2), write: set_labels(&[TRACKING, b"b"]), resumed: None });
    h.step(Event::Write { owner: Token::new(3), write: Write::Close { item: item(2) }, resumed: None });
    let sent = h.send();
    assert_eq!(sent.len(), 2, "one per item: {sent:?}");
    let add = Op::AddLabels { number: 1, labels: labels(&[TRACKING, b"a"]) };
    assert_eq!(sent[0].op, add, "the first of its lane");
    assert_eq!(sent[1].op, Op::Close { number: 2 }, "the other item's");
    let told = h.answer(&sent[0], Err(Error::Timeout));
    assert!(told.is_empty(), "a set is written again after a backoff");
    h.at(2);
    h.fire();
    let again = h.send_one();
    assert_eq!(again.op, add, "the same set, still first");
    assert!(h.answer(&again, Ok(Answer::Done)).is_empty(), "added; the rest to remove");
    let remove = h.send_one();
    assert_eq!(remove.op, Op::RemoveLabels { number: 1, labels: labels(&[HAND_IN, b"b"]) }, "the others it owns");
    let told = h.answer(&remove, Ok(Answer::Done));
    assert_eq!(*told, [Request::Wrote { owner: Token::new(1), result: Ok(Written::Done) }], "done");
    let next = h.send_one();
    assert_eq!(next.op, Op::AddLabels { number: 1, labels: labels(&[TRACKING, b"b"]) }, "then the next of its lane");
}

#[test]
fn labels_people_set_are_never_written() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    let told = h.step(Event::Write { owner, write: set_labels(&[TRACKING, b"bug"]), resumed: None });
    assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Invalid) }], "not the engine's to set");
    h.step(Event::Write { owner, write: set_labels(&[]), resumed: None });
    let remove = h.send_one();
    let ours = labels(&[TRACKING, HAND_IN, b"a", b"b"]);
    assert_eq!(remove.op, Op::RemoveLabels { number: 1, labels: ours }, "none wanted: those it owns removed");
    let told = h.answer(&remove, Ok(Answer::Done));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "done");
}

#[test]
fn an_issue_whose_creation_timed_out_is_found_by_its_key_before_it_is_tried_again() {
    let mut h = Harness::started(LIMITS);
    // The forge's clock is behind the engine's: what the forge says is what
    // the find goes by.
    h.at(30);
    h.fire();
    let listing = h.send_for(&changes(0, 1));
    let made = at(20);
    h.answer(&listing, Ok(Answer::Items { items: copies(&[issue(3, &[], 7)]), more: false, now: made }));
    h.at(31);
    let owner = Token::new(1);
    let create = Write::CreateIssue {
        repository: 0,
        key: bytes(b"k1"),
        title: bytes(b"task"),
        body: Content::Payload(Token::new(9)),
        labels: labels(&[TRACKING]),
    };
    h.step(Event::Write { owner, write: create, resumed: None });
    let made = h.send_one();
    assert_eq!(
        made.op,
        Op::CreateIssue {
            key: bytes(b"k1"),
            title: bytes(b"task"),
            body: Body::Payload(Token::new(9)),
            labels: labels(&[TRACKING])
        },
        "the payload named by its token"
    );
    h.answer(&made, Err(Error::Timeout));
    h.at(34);
    h.fire();
    let find = h.send_one();
    let issues =
        Op::Items { state: None, kind: Some(Kind::Issue), label: None, author: Some(ENGINE), since: at(20), page: 1 };
    assert_eq!(find.op, issues, "the engine's, since the forge last said its time");
    let mut other = issue(11, &[], 8);
    other.author = ENGINE;
    other.key = Some(bytes(b"k0"));
    let mut ours = issue(12, &[TRACKING], 8);
    ours.author = ENGINE;
    ours.key = Some(bytes(b"k1"));
    let told = h.answer(&find, page(Box::new([other, ours]), false));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Created(12)) }], "found, not made twice");
    let facts = h.facts();
    assert!(facts.contains(&Fact::Found { owner }), "{facts:?}");
}

#[test]
fn a_resumed_creation_is_looked_for_after_its_cause_not_where_the_working_set_is() {
    let mut h = Harness::new(LIMITS);
    // The record read since the restart has passed the engine's comments.
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    h.at(40);
    let owner = Token::new(1);
    let create = Write::CreateIssue {
        repository: 0,
        key: bytes(b"k1"),
        title: bytes(b"task"),
        body: Content::Text(bytes(b"what")),
        labels: labels(&[]),
    };
    h.step(Event::Write { owner, write: create, resumed: Some(CAUSE) });
    let find = h.send_one();
    let issues =
        Op::Items { state: None, kind: Some(Kind::Issue), label: None, author: Some(ENGINE), since: CAUSE.at, page: 1 };
    assert_eq!(find.op, issues, "issues the engine opened since its cause");
    h.answer(&find, page(Box::new([]), false));
    let made = h.send_one();
    let create = Op::CreateIssue {
        key: bytes(b"k1"),
        title: bytes(b"task"),
        body: Body::Text(bytes(b"what")),
        labels: labels(&[]),
    };
    assert_eq!(made.op, create, "not found: made");
    let owner = Token::new(2);
    let reply = Write::Comment { item: item(5), key: bytes(b"reply"), person: None, body: Content::Text(bytes(b"hi")) };
    h.step(Event::Write { owner, write: reply, resumed: Some(CAUSE) });
    let find = h.send_for(&Op::Item { number: 5, after: CAUSE.comment });
    let keyed = Comment {
        id: 60,
        author: ENGINE,
        created: Time::ZERO,
        revision: 1,
        mark: Mark::Key { key: bytes(b"reply"), person: None },
        body: bytes(b"x"),
    };
    let told = h.answer(&find, item_page(issue(5, &[TRACKING], 1), comments(&[keyed]), false));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Commented(60)) }], "found before the record");
}

#[test]
fn a_comment_not_found_after_a_timeout_is_posted_again() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let owner = Token::new(1);
    let write =
        Write::Comment { item: item(5), key: bytes(b"reply"), person: None, body: Content::Text(bytes(b"hello")) };
    h.step(Event::Write { owner, write, resumed: None });
    let made = h.send_one();
    let post = Op::Post { number: 5, key: Some(bytes(b"reply")), person: None, body: Body::Text(bytes(b"hello")) };
    assert_eq!(made.op, post, "posted with its key");
    h.answer(&made, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let find = h.send_one();
    assert_eq!(find.op, Op::Item { number: 5, after: 100 }, "looked for after the last comment passed");
    let keyed = Comment {
        id: 102,
        author: PERSON,
        created: Time::ZERO,
        revision: 1,
        mark: Mark::Key { key: bytes(b"reply"), person: None },
        body: bytes(b"x"),
    };
    let told = h.answer(&find, item_page(issue(5, &[TRACKING], 1), comments(&[comment(101, PERSON), keyed]), false));
    assert!(told.is_empty(), "a person's marker is not the engine's");
    let again = h.send_one();
    assert_eq!(again.op, post, "not made: posted again");
    let told = h.answer(&again, Ok(Answer::Commented { id: 103, revision: 1 }));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Commented(103)) }], "done");
}

#[test]
fn a_resumed_creation_is_looked_for_first_and_a_pull_request_by_its_branches() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    let open = Write::OpenPull {
        repository: 1,
        title: bytes(b"change"),
        body: Content::Text(bytes(b"why")),
        head: bytes(b"change"),
        base: bytes(b"main"),
    };
    h.step(Event::Write { owner, write: open, resumed: Some(CAUSE) });
    let find = h.send_one();
    assert_eq!(find.op, Op::PullFor { head: bytes(b"change"), base: bytes(b"main") }, "looked for first");
    let told = h.answer(&find, Ok(Answer::Pull(pull(4, HEAD, Ci::None))));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Created(4)) }], "found open");
    let open = Write::OpenPull {
        repository: 1,
        title: bytes(b"change"),
        body: Content::Text(bytes(b"why")),
        head: bytes(b"other"),
        base: bytes(b"main"),
    };
    h.step(Event::Write { owner, write: open, resumed: None });
    let made = h.send_one();
    let told = h.answer(&made, Err(Error::Exists));
    assert!(told.is_empty(), "one exists for its branches: looked for");
    let find = h.send_one();
    assert_eq!(find.op, Op::PullFor { head: bytes(b"other"), base: bytes(b"main") }, "by its branches");
}

#[test]
fn a_merge_that_timed_out_is_done_if_the_pull_request_merged_at_its_head() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::Merge { item: item(4), head: HEAD }, resumed: None });
    let made = h.send_one();
    assert_eq!(made.op, Op::Merge { number: 4, head: HEAD }, "at its head");
    h.answer(&made, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let check = h.send_one();
    assert_eq!(check.op, Op::Pull { number: 4 }, "checked");
    let mut merged = pull(4, HEAD, Ci::Passed);
    merged.state = State::Closed;
    merged.merged = Some(OTHER);
    let told = h.answer(&check, Ok(Answer::Pull(merged)));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Merged(OTHER)) }], "merged");
}

#[test]
fn a_deletion_that_finds_nothing_after_a_timeout_is_done() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::DeletePage { repository: 0, name: bytes(b"n") }, resumed: None });
    let made = h.send_one();
    h.answer(&made, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let again = h.send_one();
    let told = h.answer(&again, Err(Error::Missing));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "done");
    h.step(Event::Write { owner, write: Write::DeletePage { repository: 0, name: bytes(b"m") }, resumed: None });
    let made = h.send_one();
    let told = h.answer(&made, Err(Error::Missing));
    assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Forge(Error::Missing)) }], "never there");
}

#[test]
fn a_write_failing_for_a_while_gives_up_after_its_attempts_and_the_rate_counts_none() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::Close { item: item(1) }, resumed: None });
    let sent = h.send_one();
    h.answer(&sent, Err(Error::RateLimited { after: Duration::from_secs(5) }));
    assert!(h.send().is_empty(), "nothing goes out until the reset");
    h.at(5);
    h.fire();
    for attempt in 1..=3_u64 {
        let sent = h.send_one();
        let told = h.answer(&sent, Err(Error::Unavailable));
        if attempt < 3 {
            assert!(told.is_empty(), "tried again");
            h.at(attempt.saturating_mul(9).saturating_add(5));
            h.fire();
        } else {
            assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Forge(Error::Unavailable)) }], "gave up");
        }
    }
}

#[test]
fn a_record_is_posted_once_then_edited_after_a_fresh_read_carrying_the_position_taken() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    h.answer(&read, item_page(issue(9, &[HAND_IN], 4), comments(&[comment(1, PERSON)]), false));
    h.step(Event::Took { item: item(9), through: 1 });
    let owner = Token::new(1);
    let payload = Token::new(77);
    h.step(Event::Write { owner, write: Write::Record { item: item(9), payload }, resumed: None });
    let post = h.send_one();
    let position = Position { comment: 1, ..Position::START };
    let mine = nonce(&post.op);
    assert_eq!(
        post.op,
        Op::Post { number: 9, key: None, person: None, body: Body::Record { payload, position, nonce: mine } }
    );
    h.answer(&post, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let find = h.send_one();
    assert_eq!(find.op, Op::Item { number: 9, after: 0 }, "looked for among all its comments");
    let landed = written(2, 11, position, mine);
    let told = h.answer(&find, item_page(issue(9, &[HAND_IN], 4), comments(&[landed]), false));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "found, not posted twice");
    h.step(Event::Write { owner, write: Write::Record { item: item(9), payload }, resumed: None });
    let check = h.send_one();
    assert_eq!(check.op, Op::Comment { number: 9, id: 2 }, "read afresh before it is edited");
    h.answer(&check, Ok(Answer::Comment(written(2, 11, position, mine))));
    let edit = h.send_one();
    let second = nonce(&edit.op);
    assert_ne!(second, mine, "a nonce per write");
    assert_eq!(edit.op, Op::EditComment { number: 9, id: 2, body: Body::Record { payload, position, nonce: second } });
    let told = h.answer(&edit, Ok(Answer::Edited { revision: 50 }));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "done");
    h.step(Event::Write { owner, write: Write::Record { item: item(9), payload }, resumed: None });
    let check = h.send_one();
    h.answer(&check, Ok(Answer::Comment(written(2, 50, position, second))));
    let edit = h.send_one();
    let body = Body::Record { payload, position, nonce: nonce(&edit.op) };
    assert_eq!(edit.op, Op::EditComment { number: 9, id: 2, body }, "its own revision");
}

#[test]
fn a_record_post_that_finds_an_earlier_writes_record_edits_it_with_its_own() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    h.answer(&read, item_page(issue(9, &[TRACKING], 4), Box::new([]), false));
    let owner = Token::new(1);
    let payload = Token::new(77);
    // Asked for again after a restart: a record an earlier life posted may
    // have landed since the item was read.
    h.step(Event::Write { owner, write: Write::Record { item: item(9), payload }, resumed: Some(CAUSE) });
    let find = h.send_one();
    assert_eq!(find.op, Op::Item { number: 9, after: 0 }, "looked for first");
    let theirs = written(3, 30, Position::START, 4_242);
    h.answer(&find, item_page(issue(9, &[TRACKING], 4), comments(&[comment(2, PERSON), theirs]), false));
    let edit = h.send_one();
    let mine = nonce(&edit.op);
    let body = Body::Record { payload, position: Position::START, nonce: mine };
    assert_eq!(edit.op, Op::EditComment { number: 9, id: 3, body }, "the record found says what this write carries");
    let told = h.answer(&edit, Ok(Answer::Edited { revision: 31 }));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "done");
    let record = Record::Found { comment: 3, revision: 31, position: Position::START };
    let owner = Token::new(2);
    h.step(Event::Write { owner, write: Write::Record { item: item(9), payload }, resumed: None });
    assert_eq!(h.send_one().op, Op::Comment { number: 9, id: 3 }, "the next edits it: {record:?}");
}

#[test]
fn a_record_edit_that_timed_out_is_read_before_it_is_tried_again() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let payload = Token::new(3);
    // Landed: done, once read.
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload }, resumed: None });
    let check = h.send_one();
    h.answer(&check, Ok(Answer::Comment(record(100, 5, Position::START))));
    let edit = h.send_one();
    let mine = nonce(&edit.op);
    h.answer(&edit, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let check = h.send_one();
    assert_eq!(check.op, Op::Comment { number: 5, id: 100 }, "read, not edited blind");
    let told = h.answer(&check, Ok(Answer::Comment(written(100, 7, Position::START, mine))));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "its own edit landed");
    // A person edited the record since, keeping its block: not written over.
    let owner = Token::new(2);
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload }, resumed: None });
    let check = h.send_one();
    let told = h.answer(&check, Ok(Answer::Comment(written(100, 8, Position::START, mine))));
    let record = Record::Found { comment: 100, revision: 8, position: Position::START };
    assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Edited { record }) }], "someone else's change");
}

#[test]
fn a_record_write_that_gave_up_after_its_edit_timed_out_leaves_it_its_own_whatever_failed_last() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let payload = Token::new(3);
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload }, resumed: None });
    let check = h.send_one();
    h.answer(&check, Ok(Answer::Comment(record(100, 5, Position::START))));
    let edit = h.send_one();
    let mine = nonce(&edit.op);
    h.answer(&edit, Err(Error::Timeout));
    for secs in [3, 10] {
        h.at(secs);
        h.fire();
        let check = h.send_one();
        let told = h.answer(&check, Err(Error::Unavailable));
        if secs == 10 {
            let gave_up = [Request::Wrote { owner, result: Err(Failure::Forge(Error::Unavailable)) }];
            assert_eq!(*told, gave_up, "gave up, reading");
        }
    }
    // Its edit landed: the next write takes the record saying it as the
    // engine's own.
    let owner = Token::new(2);
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload }, resumed: None });
    let check = h.sends_for(&Op::Comment { number: 5, id: 100 });
    h.answer(&check, Ok(Answer::Comment(written(100, 999, Position::START, mine))));
    let edit = h.send_one();
    let body = Body::Record { payload, position: Position::START, nonce: nonce(&edit.op) };
    assert_eq!(edit.op, Op::EditComment { number: 5, id: 100, body }, "edited, not held");
}

#[test]
fn a_record_post_queued_behind_one_that_found_a_record_changed_is_not_posted() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    h.answer(&read, item_page(issue(9, &[TRACKING], 4), Box::new([]), false));
    let first = Token::new(1);
    let second = Token::new(2);
    h.step(Event::Write {
        owner: first,
        write: Write::Record { item: item(9), payload: Token::new(5) },
        resumed: None,
    });
    h.step(Event::Write {
        owner: second,
        write: Write::Record { item: item(9), payload: Token::new(6) },
        resumed: None,
    });
    let post = h.send_one();
    h.answer(&post, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let find = h.send_one();
    // It landed, and a person mangled it since.
    let mangled =
        Comment { id: 2, author: ENGINE, created: Time::ZERO, revision: 20, mark: Mark::Mangled, body: bytes(b"?") };
    let told = h.answer(&find, item_page(issue(9, &[TRACKING], 4), comments(&[mangled]), false));
    let record = Record::Mangled { comment: 2, revision: 20 };
    let expected = [
        Request::Wrote { owner: first, result: Err(Failure::Edited { record }) },
        Request::Wrote { owner: second, result: Err(Failure::Edited { record }) },
    ];
    assert_eq!(*told, expected, "the one behind it is not posted over it");
    assert!(h.send().is_empty(), "nothing posted");
}

#[test]
fn a_record_someone_else_changed_is_not_written_over() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let owner = Token::new(1);
    let write = Write::Record { item: item(5), payload: Token::new(3) };
    h.step(Event::Write { owner, write, resumed: None });
    let check = h.send_one();
    assert_eq!(check.op, Op::Comment { number: 5, id: 100 }, "read afresh");
    let mangled =
        Comment { id: 100, author: ENGINE, created: Time::ZERO, revision: 999, mark: Mark::Mangled, body: bytes(b"?") };
    let told = h.answer(&check, Ok(Answer::Comment(mangled)));
    let record = Record::Mangled { comment: 100, revision: 999 };
    assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Edited { record }) }], "held for a person");
    assert!(h.facts().contains(&Fact::Edited { item: item(5) }), "told as a fact");
    // The parent, released by a person, writes over the record as it now is.
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload: Token::new(3) }, resumed: None });
    let check = h.send_one();
    let mangled =
        Comment { id: 100, author: ENGINE, created: Time::ZERO, revision: 999, mark: Mark::Mangled, body: bytes(b"?") };
    h.answer(&check, Ok(Answer::Comment(mangled)));
    let edit = h.send_one();
    let body = Body::Record { payload: Token::new(3), position: Position::START, nonce: nonce(&edit.op) };
    assert_eq!(edit.op, Op::EditComment { number: 5, id: 100, body }, "edited over the record it last read");
}

#[test]
fn a_wiki_page_is_written_only_at_the_revision_last_read() {
    let mut h = Harness::started(LIMITS);
    let page_at = |revision: u64, nonce: Option<u64>| {
        Ok(Answer::Page(Page { name: bytes(b"n"), content: bytes(b"c"), revision, nonce }))
    };
    let owner = Token::new(1);
    let put =
        Write::PutPage { repository: 0, name: bytes(b"n"), content: Content::Text(bytes(b"new")), revision: Some(4) };
    h.step(Event::Write { owner, write: put, resumed: None });
    let check = h.send_one();
    assert_eq!(check.op, Op::Page { name: bytes(b"n") }, "read afresh");
    h.answer(&check, page_at(4, None));
    let made = h.send_one();
    let mine = nonce(&made.op);
    assert_eq!(made.op, Op::PutPage { name: bytes(b"n"), content: Body::Text(bytes(b"new")), nonce: mine });
    h.answer(&made, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let check = h.send_one();
    let told = h.answer(&check, page_at(6, Some(mine)));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Revision(6)) }], "its own write landed");
    let owner = Token::new(2);
    let put =
        Write::PutPage { repository: 0, name: bytes(b"n"), content: Content::Text(bytes(b"again")), revision: Some(6) };
    h.step(Event::Write { owner, write: put, resumed: None });
    let check = h.send_one();
    let told = h.answer(&check, page_at(7, None));
    assert_eq!(
        *told,
        [Request::Wrote { owner, result: Err(Failure::Revised { revision: Some(7) }) }],
        "not written over"
    );
    let owner = Token::new(3);
    let put =
        Write::PutPage { repository: 0, name: bytes(b"m"), content: Content::Text(bytes(b"new")), revision: None };
    h.step(Event::Write { owner, write: put, resumed: None });
    let check = h.send_one();
    h.answer(&check, Err(Error::Missing));
    let made = h.send_one();
    let put = Op::PutPage { name: bytes(b"m"), content: Body::Text(bytes(b"new")), nonce: nonce(&made.op) };
    assert_eq!(made.op, put, "none there: made");
}

#[test]
fn a_write_that_timed_out_waits_until_it_can_no_longer_land() {
    let limits = Limits { lifetime: Duration::from_secs(20), ..LIMITS };
    let mut h = Harness::new(limits);
    assert!(h.fire().is_empty(), "its first moment");
    assert!(h.send().is_empty(), "no call until what an earlier life asked for has landed");
    h.at(20);
    h.fire();
    assert!(!h.send().is_empty(), "the cold start, then");
    h.at(30);
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: set_labels(&[TRACKING]), resumed: None });
    h.step(Event::Write { owner: Token::new(2), write: set_labels(&[TRACKING, b"a"]), resumed: None });
    let add = h.send_for(&Op::AddLabels { number: 1, labels: labels(&[TRACKING]) });
    h.answer(&add, Err(Error::Timeout));
    h.at(33);
    h.fire();
    assert!(h.send().is_empty(), "not tried again while the first may land");
    h.at(50);
    h.fire();
    let again = h.send_one();
    assert_eq!(again.op, add.op, "tried again once it cannot");
    h.answer(&again, Err(Error::Timeout));
    h.at(70);
    h.fire();
    let last = h.send_one();
    let told = h.answer(&last, Err(Error::Timeout));
    assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Forge(Error::Timeout)) }], "gave up");
    h.at(80);
    h.fire();
    assert!(h.send().is_empty(), "its lane is held while its last call may land");
    h.at(90);
    h.fire();
    assert_eq!(h.send_one().op, Op::AddLabels { number: 1, labels: labels(&[TRACKING, b"a"]) }, "then the next");
}

#[test]
fn a_verdict_is_a_review_keyed_and_found_by_its_key() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    let write = Write::Review {
        item: item(4),
        key: bytes(b"verdict"),
        verdict: Verdict::RequestChanges,
        body: Content::Text(bytes(b"not yet")),
    };
    h.step(Event::Write { owner, write, resumed: None });
    let made = h.send_one();
    let review = Op::Review {
        number: 4,
        key: bytes(b"verdict"),
        verdict: Verdict::RequestChanges,
        body: Body::Text(bytes(b"not yet")),
    };
    assert_eq!(made.op, review, "made with its key");
    h.answer(&made, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let find = h.send_one();
    assert_eq!(find.op, Op::Reviews { number: 4, page: 1 }, "looked for among the reviews");
    h.answer(&find, reviews(&[(3, PERSON, Verdict::Approve)], HEAD, true));
    let next = h.send_one();
    assert_eq!(next.op, Op::Reviews { number: 4, page: 2 });
    let mut list = List::with_capacity(1);
    let found = Review {
        id: 8,
        author: ENGINE,
        verdict: Verdict::RequestChanges,
        commit: HEAD,
        key: Some(bytes(b"verdict")),
        body: bytes(b"not yet"),
    };
    list.push(found).expect("room");
    let told = h.answer(&next, Ok(Answer::Reviews { reviews: list.into_boxed(), more: false }));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Reviewed(8)) }], "found, not made twice");
}

#[test]
fn reviewers_and_dependencies_are_written_as_sets_and_an_item_reopened() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    let many = Write::SetReviewers { item: item(4), reviewers: Box::new([1, 2, 3, 4]) };
    let told = h.step(Event::Write { owner, write: many, resumed: None });
    assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Invalid) }], "more than the limits");
    for (write, op) in [
        (
            Write::SetReviewers { item: item(4), reviewers: Box::new([PERSON]) },
            Op::SetReviewers { number: 4, reviewers: Box::new([PERSON]) },
        ),
        (
            Write::SetDependencies { item: item(4), dependencies: Box::new([2, 3]) },
            Op::SetDependencies { number: 4, dependencies: Box::new([2, 3]) },
        ),
        (Write::Reopen { item: item(4) }, Op::Reopen { number: 4 }),
    ] {
        h.step(Event::Write { owner, write, resumed: None });
        let made = h.send_one();
        assert_eq!(made.op, op);
        assert!(h.answer(&made, Err(Error::Timeout)).is_empty(), "the same whenever it lands");
        h.at(h.env.now.as_nanos() / 1_000_000_000 + 3);
        h.fire();
        let again = h.send_one();
        assert_eq!(again.op, op, "written again as it is");
        let told = h.answer(&again, Ok(Answer::Done));
        assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }]);
    }
}

#[test]
fn a_persons_message_the_engine_writes_is_their_news() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let owner = Token::new(1);
    let message = Content::Text(bytes(b"please"));
    let write = Write::Comment { item: item(5), key: bytes(b"web-1"), person: Some(PERSON), body: message };
    h.step(Event::Write { owner, write, resumed: None });
    let made = h.send_one();
    let post =
        Op::Post { number: 5, key: Some(bytes(b"web-1")), person: Some(PERSON), body: Body::Text(bytes(b"please")) };
    assert_eq!(made.op, post, "written for the person");
    h.answer(&made, Ok(Answer::Commented { id: 101, revision: 1 }));
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 20)]);
    let theirs = Comment {
        id: 101,
        author: ENGINE,
        created: Time::ZERO,
        revision: 1,
        mark: Mark::Key { key: bytes(b"web-1"), person: Some(PERSON) },
        body: bytes(b"please"),
    };
    let ours = Comment {
        id: 102,
        author: ENGINE,
        created: Time::ZERO,
        revision: 1,
        mark: Mark::Key { key: bytes(b"r"), person: None },
        body: bytes(b"x"),
    };
    let told = h.answer(&sent[0], item_page(issue(5, &[TRACKING], 20), comments(&[theirs, ours]), false));
    assert_eq!(*told, [news(5, 1, News::Comment { on: 5, id: 101, author: PERSON })], "the person's, not the engine's");
}

#[test]
fn a_reviews_inline_comments_are_a_fresh_read() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    h.step(Event::Read { owner, read: Read::Remarks { item: item(4), review: 8, page: 2 } });
    let read = h.send_one();
    assert_eq!(read.op, Op::Remarks { number: 4, review: 8, page: 2 });
    let mut remarks = List::with_capacity(1);
    let remark = Remark { id: 9, author: PERSON, path: bytes(b"src"), line: 3, body: bytes(b"why?") };
    remarks.push(remark).expect("room");
    let answer = Answer::Remarks { remarks: remarks.into_boxed(), more: false };
    let told = h.answer(&read, Ok(answer));
    let Request::Read { owner: answered, result: Ok(Answer::Remarks { remarks, more: false }) } = &told[0] else {
        panic!("the read answered: {told:?}");
    };
    assert_eq!((*answered, remarks.len()), (owner, 1), "answered once, as read");
}

// The budget.

#[test]
fn the_budget_spends_its_window_then_waits_for_it_to_end() {
    let mut h = Harness::started(LIMITS);
    h.env.limits.rate = 2;
    h.at(100);
    h.fire();
    h.step(Event::Read { owner: Token::new(1), read: Read::Pull { item: item(1) } });
    let sent = h.send();
    assert_eq!(sent.len(), 2, "a window's worth: {sent:?}");
    assert_eq!(sent[0].op, Op::Pull { number: 1 }, "the read first");
    assert!(h.send().is_empty(), "spent");
    assert_eq!(h.model.next_deadline(), Some(at(160)), "until the window ends");
    h.at(160);
    h.fire();
    let sent = h.send_one();
    assert_eq!(sent.repository, 1, "the other repository's listing in the next window");
    assert!(h.facts().contains(&Fact::Spent { until: at(160) }), "told as a fact");
}

#[test]
fn calls_go_out_by_priority_and_no_more_than_the_limit_at_once() {
    let limits = Limits { calls: 2, ..LIMITS };
    let mut h = Harness::new(limits);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    h.at(30);
    h.fire();
    h.step(Event::Write { owner: Token::new(1), write: Write::Close { item: item(1) }, resumed: None });
    h.step(Event::Read { owner: Token::new(2), read: Read::Pull { item: item(2) } });
    let sent = h.send();
    assert_eq!(sent.len(), 2, "two at once: {sent:?}");
    assert_eq!(sent[0].op, Op::Pull { number: 2 }, "the fresh read first");
    assert_eq!(sent[1].op, Op::Close { number: 1 }, "then the write");
    h.answer(&sent[0], Ok(Answer::Pull(pull(2, HEAD, Ci::None))));
    let next = h.send_one();
    assert_eq!(next.op, changes(0, 1), "keeping up after them");
    let facts = h.facts();
    assert!(facts.contains(&Fact::Sent { priority: Priority::Fresh }), "{facts:?}");
}

#[test]
fn keeping_up_has_its_share_of_each_window_whatever_the_parent_asks() {
    let mut h = Harness::new(Limits { calls: 8, ..LIMITS });
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    h.env.limits = Limits { rate: 4, reserve: 2, calls: 8, ..LIMITS };
    h.at(100);
    for owner in 1..=2_u64 {
        h.step(Event::Read { owner: Token::new(owner), read: Read::Pull { item: item(owner) } });
    }
    for number in 1..=3_u64 {
        h.step(Event::Write {
            owner: Token::new(10 + number),
            write: Write::Close { item: item(number) },
            resumed: None,
        });
    }
    h.fire();
    let sent = h.send();
    let ops: List<&Op> = {
        let mut ops = List::with_capacity(8);
        for call in &sent {
            ops.push(&call.op).expect("room");
        }
        ops
    };
    assert_eq!(sent.len(), 4, "a window's worth: {sent:?}");
    assert_eq!(*ops.as_slice()[0], Op::Pull { number: 1 }, "the parent's first");
    assert_eq!(*ops.as_slice()[1], Op::Pull { number: 2 });
    assert!(lists_changes(ops.as_slice()[2]) && lists_changes(ops.as_slice()[3]), "then keeping up's share: {sent:?}");
}

#[test]
fn a_rate_limit_refusal_holds_every_call_until_its_reset() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    h.step(Event::Read { owner, read: Read::Pull { item: item(1) } });
    let sent = h.send_one();
    h.step(Event::Write { owner, write: Write::Close { item: item(2) }, resumed: None });
    let told = h.answer(&sent, Err(Error::RateLimited { after: Duration::from_secs(40) }));
    assert!(told.is_empty(), "the read waits");
    assert!(h.send().is_empty(), "nothing goes out before the reset");
    h.at(30);
    h.fire();
    assert!(h.send().is_empty(), "not even keeping up");
    h.at(40);
    h.fire();
    let sent = h.send();
    assert_eq!(sent[0].op, Op::Pull { number: 1 }, "the read again, first: {sent:?}");
    assert!(h.facts().contains(&Fact::Limited { reset: at(40) }), "told as a fact");
}

// Bounds.

#[test]
fn the_worst_case_is_bounded_or_refused() {
    assert!(worst_case(&LIMITS).is_some(), "the test limits fit");
    assert!(worst_case(&Limits { rate: 0, ..LIMITS }).is_none(), "a budget of no calls is refused");
    assert!(worst_case(&Limits { items: u32::MAX, ..LIMITS }).is_none(), "beyond a u64");
    let more = worst_case(&Limits { items: 4, ..LIMITS }).expect("fits");
    assert!(more > worst_case(&LIMITS).expect("fits"), "more room costs more");
}

#[test]
fn an_untracked_item_tells_nothing_more_and_its_read_is_dropped() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    h.step(Event::Untrack { item: item(9) });
    assert!(!h.model.is_tracked(item(9)), "it left");
    let told = h.answer(&read, item_page(issue(9, &[TRACKING], 4), Box::new([]), false));
    assert!(told.is_empty(), "nothing is told of it");
    assert_eq!(h.model.items(), 0, "and it is retired");
}

#[test]
fn a_record_edit_that_gave_up_after_a_timeout_leaves_the_record_its_own_to_the_next() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let owner = Token::new(1);
    let payload = Token::new(3);
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload }, resumed: None });
    let mut check = h.send_one();
    let mut mine = 0;
    for secs in [3, 10, 30] {
        h.answer(&check, Ok(Answer::Comment(record(100, 5, Position::START))));
        let edit = h.send_one();
        mine = nonce(&edit.op);
        let body = Body::Record { payload, position: Position::START, nonce: mine };
        assert_eq!(edit.op, Op::EditComment { number: 5, id: 100, body });
        let told = h.answer(&edit, Err(Error::Timeout));
        if secs == 30 {
            assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Forge(Error::Timeout)) }], "gave up");
        } else {
            h.at(secs);
            h.fire();
            check = h.send_one();
            assert_eq!(check.op, Op::Comment { number: 5, id: 100 }, "read before it is tried again");
        }
    }
    // The edit may have landed: the record says what it carried, at a
    // revision not known.
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload }, resumed: None });
    let check = h.sends_for(&Op::Comment { number: 5, id: 100 });
    h.answer(&check, Ok(Answer::Comment(written(100, 999, Position::START, mine))));
    let edit = h.send_one();
    let body = Body::Record { payload, position: Position::START, nonce: nonce(&edit.op) };
    assert_eq!(edit.op, Op::EditComment { number: 5, id: 100, body }, "its own: edited, not held");
}
