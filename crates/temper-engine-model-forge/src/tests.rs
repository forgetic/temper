//! Feed the model events, inspect the requests that come out.

use alloc::boxed::Box;

use temper_lib::{Duration, Env, List, Queue, Time, Token};

use crate::api::{Answer, Body, Check, Comment, Error, Kind, Mark, Op, Pull, Review, State, Status, Summary, Verdict};
use crate::{
    Ci, Config, Content, Event, Fact, Failure, Item, Level, Limits, Model, News, Position, Priority, Read, Record,
    Request, View, Write, Written, fire, max_out, resume, step, worst_case,
};

/// The engine's forge user, and a person's.
const ENGINE: u64 = 1;
const PERSON: u64 = 7;

const LIMITS: Limits = Limits {
    repositories: 2,
    items: 3,
    labels: 3,
    inbox: 3,
    reads: 2,
    writes: 3,
    calls: 8,
    page: 3,
    name_bytes: 16,
    title_bytes: 16,
    body_bytes: 32,
    rate: 100,
    window: Duration::from_secs(60),
    poll: Duration::from_secs(30),
    hinted: Duration::from_secs(2),
    resolution: Duration::from_secs(1),
    slow: Duration::from_secs(600),
    backoff: Duration::from_secs(1),
    backoff_max: Duration::from_secs(8),
    attempts: 3,
    facts: 256,
};

const TRACKING: &[u8] = b"temper";
const HAND_IN: &[u8] = b"hand-in";

/// A head's commit.
const HEAD: [u8; 32] = [7; 32];
const OTHER: [u8; 32] = [8; 32];

fn config() -> Config {
    Config { engine: ENGINE, tracking: bytes(TRACKING), hand_in: bytes(HAND_IN) }
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

    fn answer(&mut self, sent: &Sent, result: Result<Answer, Error>) -> Box<[Request]> {
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
        | Op::PullFor { .. }
        | Op::Statuses { .. }
        | Op::Permission { .. }
        | Op::Branch { .. }
        | Op::Pages { .. }
        | Op::Page { .. }
        | Op::CreateIssue { .. }
        | Op::Post { .. }
        | Op::EditComment { .. }
        | Op::SetLabels { .. }
        | Op::OpenPull { .. }
        | Op::Merge { .. }
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
    Comment { id, author, revision: id, mark: Mark::None, body: bytes(b"text") }
}

/// The engine's record as the comment `id`, saying `position`. Its revision
/// is the comment's id and the item's number.
fn record(id: u64, number: u64, position: Position) -> Comment {
    let revision = id.saturating_add(number);
    Comment { id, author: ENGINE, revision, mark: Mark::Record(position), body: bytes(b"record") }
}

fn comments(list: &[Comment]) -> Box<[Comment]> {
    let mut copy = List::with_capacity(8);
    for comment in list {
        let mark = match &comment.mark {
            Mark::None => Mark::None,
            Mark::Key(key) => Mark::Key(key.clone()),
            Mark::Record(position) => Mark::Record(*position),
            Mark::Mangled => Mark::Mangled,
        };
        copy.push(Comment { mark, body: comment.body.clone(), ..*comment }).expect("room");
    }
    copy.into_boxed()
}

#[expect(clippy::unnecessary_wraps, reason = "what a call is answered with")]
fn page(items: Box<[Summary]>, more: bool) -> Result<Answer, Error> {
    Ok(Answer::Items { items, more })
}

#[expect(clippy::unnecessary_wraps, reason = "what a call is answered with")]
fn item_page(item: Summary, comments: Box<[Comment]>, more: bool) -> Result<Answer, Error> {
    Ok(Answer::Item { item, comments, more })
}

fn tracked_listing(page: u32) -> Op {
    Op::Items { state: Some(State::Open), kind: None, label: Some(bytes(TRACKING)), since: Time::ZERO, page }
}

fn changes(since: u64, page: u32) -> Op {
    Op::Items { state: None, kind: None, label: None, since: at(since), page }
}

fn item(number: u64) -> Item {
    Item { repository: 0, number }
}

fn pull(number: u64, commit: [u8; 32], checks: &[Check], reviews: &[(u64, Verdict)]) -> Pull {
    let mut statuses = List::with_capacity(8);
    for check in checks {
        statuses.push(Status { context: bytes(b"ci"), check: *check }).expect("room");
    }
    let mut list = List::with_capacity(8);
    for (author, verdict) in reviews {
        list.push(Review { author: *author, verdict: *verdict, commit, body: bytes(b"review") }).expect("room");
    }
    Pull {
        number,
        state: State::Open,
        head: bytes(b"change"),
        base: bytes(b"main"),
        commit,
        merged: None,
        mergeable: true,
        reviews: list.into_boxed(),
        more: false,
        statuses: statuses.into_boxed(),
    }
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
        [announced(5, record), news(5, 1, News::Comment { id: 4, author: PERSON })],
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
    assert_eq!(*told, [news(5, 1, News::Comment { id: 2, author: PERSON })], "the comment between is news");
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
        [Request::Announced { item: item(9), view }, news(9, 1, News::Comment { id: 1, author: PERSON })],
        "announced with no record, and every comment is news"
    );
    assert!(h.send().is_empty(), "read once");
}

#[test]
fn a_mangled_record_is_announced_as_such_and_news_starts_after_it() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    let mangled = Comment { id: 2, author: ENGINE, revision: 20, mark: Mark::Mangled, body: bytes(b"?") };
    let page = comments(&[comment(1, PERSON), mangled, comment(3, PERSON)]);
    let told = h.answer(&read, item_page(issue(9, &[TRACKING], 4), page, false));
    let view =
        View { kind: Kind::Issue, labels: labels(&[TRACKING]), record: Record::Mangled { comment: 2, revision: 20 } };
    assert_eq!(
        *told,
        [Request::Announced { item: item(9), view }, news(9, 1, News::Comment { id: 3, author: PERSON })],
        "held for a person by its parent; what came after it is news"
    );
}

#[test]
fn a_record_of_someone_else_is_not_the_engines() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Track { item: item(9) });
    let read = h.send_one();
    let forged = Comment { id: 2, author: PERSON, revision: 2, mark: Mark::Record(Position::START), body: bytes(b"!") };
    let told = h.answer(&read, item_page(issue(9, &[TRACKING], 4), comments(&[forged]), false));
    let view = View { kind: Kind::Issue, labels: labels(&[TRACKING]), record: Record::Missing };
    assert_eq!(
        *told,
        [Request::Announced { item: item(9), view }, news(9, 1, News::Comment { id: 2, author: PERSON })],
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
    assert_eq!(*told, [Request::Left { item: item(9) }], "it left");
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
    assert_eq!(*told, [news(5, 1, News::Comment { id: 101, author: PERSON })], "news");
    // The next pass starts at the newest time seen, inclusive.
    h.at(60);
    h.fire();
    let sent = h.send();
    assert_eq!(sent[0].op, changes(25, 1), "from the newest updated time seen");
}

#[test]
fn an_item_listed_again_at_the_time_it_changed_is_read_once_more_in_a_later_pass() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 20)]);
    assert_eq!(sent.len(), 1, "read for the change");
    h.answer(&sent[0], item_page(issue(5, &[TRACKING], 20), Box::new([]), false));
    let (_, sent) = h.pass(60, &[issue(5, &[TRACKING], 20)]);
    assert_eq!(sent.len(), 1, "read once more: a change may have come in the same second");
    h.answer(&sent[0], item_page(issue(5, &[TRACKING], 20), Box::new([]), false));
    let (_, sent) = h.pass(90, &[issue(5, &[TRACKING], 20)]);
    assert!(sent.is_empty(), "and then no more: {sent:?}");
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
    // An item changed as the pass ran: one of the time it paged at may have
    // shifted onto the page it had read.
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
    assert_eq!(*told, [Request::Left { item: item(5) }], "it left");
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
    h.step(Event::Hint { repository: 0, item: Some(3), commit: None });
    assert_eq!(h.model.next_deadline(), Some(at(10)), "at once: the last pass began long enough ago");
    h.fire();
    let listing = h.send_one();
    h.at(11);
    h.step(Event::Hint { repository: 0, item: None, commit: None });
    h.answer(&listing, page(Box::new([]), false));
    assert_eq!(h.model.next_deadline(), Some(at(12)), "the next follows soon after the one that ran");
}

#[test]
fn pull_requests_whose_ci_has_not_settled_are_read_every_pass_and_on_a_status_hint() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    h.step(Event::Link { item: item(5), pull: Some(9) });
    let read = h.send_one();
    assert_eq!(read.op, Op::Pull { number: 9, reviews: 0 }, "the linked pull request is read");
    let told = h.answer(&read, Ok(Answer::Pull(pull(9, HEAD, &[Check::Pending], &[]))));
    let level = News::Pull { commit: HEAD, ci: Ci::Pending, open: true, merged: None, mergeable: true };
    assert_eq!(*told, [news(5, 1, level)], "its head and CI are news");
    let (_, sent) = h.pass(30, &[]);
    assert_eq!(sent.len(), 1, "pending CI is read again with the pass");
    let told = h.answer(&sent[0], Ok(Answer::Pull(pull(9, HEAD, &[Check::Passed], &[]))));
    let level = News::Pull { commit: HEAD, ci: Ci::Passed, open: true, merged: None, mergeable: true };
    assert_eq!(*told, [news(5, 2, level)], "CI passed");
    let (_, sent) = h.pass(60, &[]);
    assert!(sent.is_empty(), "settled CI is not read again: {sent:?}");
    h.step(Event::Hint { repository: 0, item: None, commit: Some(HEAD) });
    let read = h.send_one();
    assert_eq!(read.op, Op::Pull { number: 9, reviews: 0 }, "a status on its head reads it again");
    h.answer(&read, Ok(Answer::Pull(pull(9, HEAD, &[Check::Failed], &[]))));
    assert_eq!(
        h.model.pull(item(5)),
        Some(Level { number: 9, commit: HEAD, ci: Ci::Failed, open: true, merged: None, mergeable: true }),
        "the level as last read"
    );
}

#[test]
fn reviews_after_those_taken_are_news_and_a_linked_pull_request_listed_is_read() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    h.step(Event::Link { item: item(5), pull: Some(9) });
    let read = h.send_one();
    h.answer(&read, Ok(Answer::Pull(pull(9, HEAD, &[Check::Passed], &[]))));
    let (_, sent) = h.pass(30, &[summary(9, Kind::Pull, State::Open, &[], 25)]);
    assert_eq!(sent[0].op, Op::Pull { number: 9, reviews: 0 }, "the linked pull request changed: read");
    let reviews = [(PERSON, Verdict::Approve), (8, Verdict::RequestChanges)];
    let mut page = pull(9, HEAD, &[Check::Passed], &reviews);
    page.more = true;
    let told = h.answer(&sent[0], Ok(Answer::Pull(page)));
    assert_eq!(
        *told,
        [
            news(5, 2, News::Review { author: PERSON, verdict: Verdict::Approve, commit: HEAD }),
            news(5, 3, News::Review { author: 8, verdict: Verdict::RequestChanges, commit: HEAD }),
        ],
        "each review is news"
    );
    assert!(h.send().is_empty(), "the inbox is full: the rest waits");
    h.step(Event::Took { item: item(5), through: 3 });
    let next = h.send_one();
    assert_eq!(next.op, Op::Pull { number: 9, reviews: 2 }, "the reviews after those told");
    let told = h.answer(&next, Ok(Answer::Pull(pull(9, HEAD, &[Check::Passed], &[(PERSON, Verdict::Comment)]))));
    assert_eq!(*told, [news(5, 4, News::Review { author: PERSON, verdict: Verdict::Comment, commit: HEAD })], "and on");
}

#[test]
fn a_full_inbox_waits_until_the_parent_takes_news() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let (_, sent) = h.pass(30, &[issue(5, &[TRACKING], 20)]);
    let page = comments(&[comment(101, PERSON), comment(102, ENGINE), comment(103, PERSON)]);
    h.answer(&sent[0], item_page(issue(5, &[TRACKING], 20), page, true));
    let read = h.send_one();
    assert_eq!(read.op, Op::Item { number: 5, after: 103 }, "the next page");
    let page = comments(&[comment(104, PERSON), comment(106, PERSON)]);
    let told = h.answer(&read, item_page(issue(5, &[TRACKING], 20), page, false));
    assert_eq!(*told, [news(5, 3, News::Comment { id: 104, author: PERSON })], "as much news as the inbox holds");
    assert!(h.send().is_empty(), "nothing more is read while it is full");
    h.step(Event::Took { item: item(5), through: 2 });
    let read = h.send_one();
    assert_eq!(read.op, Op::Item { number: 5, after: 104 }, "read on from the last comment told");
    let told = h.answer(&read, item_page(issue(5, &[TRACKING], 20), comments(&[comment(106, PERSON)]), false));
    assert_eq!(*told, [news(5, 4, News::Comment { id: 106, author: PERSON })], "news resumes");
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
            | Op::PullFor { .. }
            | Op::Statuses { .. }
            | Op::Permission { .. }
            | Op::Branch { .. }
            | Op::Pages { .. }
            | Op::Page { .. }
            | Op::CreateIssue { .. }
            | Op::Post { .. }
            | Op::EditComment { .. }
            | Op::SetLabels { .. }
            | Op::OpenPull { .. }
            | Op::Merge { .. }
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
    assert_eq!(*told, [Request::Left { item: item(5) }], "room frees");
    assert_eq!(h.model.next_deadline(), Some(at(62)), "the labels are listed again soon");
    h.at(62);
    h.fire();
    let sent = h.send();
    assert_eq!(sent[0].op, tracked_listing(1), "the tracking label first");
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
    let refused = h.step(Event::Write { owner, write: many, resumed: false });
    assert_eq!(*refused, [Request::Wrote { owner, result: Err(Failure::Invalid) }], "more labels than the limits");
    let record = Write::Record { item: item(1), payload: Token::new(2) };
    let unknown = h.step(Event::Write { owner, write: record, resumed: false });
    assert_eq!(*unknown, [Request::Wrote { owner, result: Err(Failure::Unknown) }], "a record of an item not held");
    for number in 1..=3 {
        h.step(Event::Write { owner, write: Write::Close { item: item(number) }, resumed: false });
    }
    let busy = h.step(Event::Write { owner, write: Write::Close { item: item(4) }, resumed: false });
    assert_eq!(*busy, [Request::Wrote { owner, result: Err(Failure::Busy) }], "three writes at once");
}

// Fresh reads.

#[test]
fn fresh_reads_go_out_first_are_tried_again_and_answered_once() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(4);
    h.step(Event::Write { owner: Token::new(5), write: Write::Close { item: item(1) }, resumed: false });
    h.step(Event::Read { owner, read: Read::Pull { item: item(9) } });
    let sent = h.send();
    assert_eq!(sent[0].op, Op::Pull { number: 9, reviews: 0 }, "the read before the write: {sent:?}");
    assert_eq!(sent[1].op, Op::Close { number: 1 }, "then the write");
    assert!(h.answer(&sent[0], Err(Error::Unavailable)).is_empty(), "tried again");
    h.at(2);
    h.fire();
    let again = h.send_one();
    let told = h.answer(&again, Ok(Answer::Pull(pull(9, HEAD, &[], &[]))));
    assert_eq!(*told, [Request::Read { owner, result: Ok(Answer::Pull(pull(9, HEAD, &[], &[]))) }], "answered");
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

#[test]
fn writes_about_one_item_go_one_at_a_time_in_order() {
    let mut h = Harness::started(LIMITS);
    h.step(Event::Write { owner: Token::new(1), write: set_labels(&[b"a"]), resumed: false });
    h.step(Event::Write { owner: Token::new(2), write: set_labels(&[b"b"]), resumed: false });
    h.step(Event::Write { owner: Token::new(3), write: Write::Close { item: item(2) }, resumed: false });
    let sent = h.send();
    assert_eq!(sent.len(), 2, "one per item: {sent:?}");
    assert_eq!(sent[0].op, Op::SetLabels { number: 1, labels: labels(&[b"a"]) }, "the first of its lane");
    assert_eq!(sent[1].op, Op::Close { number: 2 }, "the other item's");
    let told = h.answer(&sent[0], Err(Error::Timeout));
    assert!(told.is_empty(), "a set is written again after a backoff");
    h.at(2);
    h.fire();
    let again = h.send_one();
    assert_eq!(again.op, Op::SetLabels { number: 1, labels: labels(&[b"a"]) }, "the same set, still first");
    let told = h.answer(&again, Ok(Answer::Done));
    assert_eq!(*told, [Request::Wrote { owner: Token::new(1), result: Ok(Written::Done) }], "done");
    let next = h.send_one();
    assert_eq!(next.op, Op::SetLabels { number: 1, labels: labels(&[b"b"]) }, "then the next of its lane");
}

#[test]
fn an_issue_whose_creation_timed_out_is_found_by_its_key_before_it_is_tried_again() {
    let mut h = Harness::started(LIMITS);
    h.at(10);
    let owner = Token::new(1);
    let create = Write::CreateIssue {
        repository: 0,
        key: bytes(b"k1"),
        title: bytes(b"task"),
        body: Content::Payload(Token::new(9)),
        labels: labels(&[TRACKING]),
    };
    h.step(Event::Write { owner, write: create, resumed: false });
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
    h.at(13);
    h.fire();
    let find = h.send_one();
    let since = at(10);
    assert_eq!(find.op, Op::Items { state: None, kind: Some(Kind::Issue), label: None, since, page: 1 }, "look");
    let mut other = issue(11, &[], 11);
    other.key = Some(bytes(b"k0"));
    let mut ours = issue(12, &[TRACKING], 11);
    ours.author = ENGINE;
    ours.key = Some(bytes(b"k1"));
    let told = h.answer(&find, page(Box::new([other, ours]), false));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Created(12)) }], "found, not made twice");
    let facts = h.facts();
    assert!(facts.contains(&Fact::Found { owner }), "{facts:?}");
}

#[test]
fn a_comment_not_found_after_a_timeout_is_posted_again() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let owner = Token::new(1);
    let write = Write::Comment { item: item(5), key: bytes(b"reply"), body: Content::Text(bytes(b"hello")) };
    h.step(Event::Write { owner, write, resumed: false });
    let made = h.send_one();
    let post = Op::Post { number: 5, key: Some(bytes(b"reply")), body: Body::Text(bytes(b"hello")) };
    assert_eq!(made.op, post, "posted with its key");
    h.answer(&made, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let find = h.send_one();
    assert_eq!(find.op, Op::Item { number: 5, after: 100 }, "looked for after the last comment passed");
    let keyed = Comment { id: 102, author: PERSON, revision: 1, mark: Mark::Key(bytes(b"reply")), body: bytes(b"x") };
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
    h.step(Event::Write { owner, write: open, resumed: true });
    let find = h.send_one();
    assert_eq!(find.op, Op::PullFor { head: bytes(b"change"), base: bytes(b"main") }, "looked for first");
    let told = h.answer(&find, Ok(Answer::Pull(pull(4, HEAD, &[], &[]))));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Created(4)) }], "found open");
    let open = Write::OpenPull {
        repository: 1,
        title: bytes(b"change"),
        body: Content::Text(bytes(b"why")),
        head: bytes(b"other"),
        base: bytes(b"main"),
    };
    h.step(Event::Write { owner, write: open, resumed: false });
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
    h.step(Event::Write { owner, write: Write::Merge { item: item(4), head: HEAD }, resumed: false });
    let made = h.send_one();
    assert_eq!(made.op, Op::Merge { number: 4, head: HEAD }, "at its head");
    h.answer(&made, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let check = h.send_one();
    assert_eq!(check.op, Op::Pull { number: 4, reviews: 0 }, "checked");
    let mut merged = pull(4, HEAD, &[Check::Passed], &[]);
    merged.state = State::Closed;
    merged.merged = Some(OTHER);
    let told = h.answer(&check, Ok(Answer::Pull(merged)));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Merged(OTHER)) }], "merged");
}

#[test]
fn a_deletion_that_finds_nothing_after_a_timeout_is_done() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::DeletePage { repository: 0, name: bytes(b"n") }, resumed: false });
    let made = h.send_one();
    h.answer(&made, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let again = h.send_one();
    let told = h.answer(&again, Err(Error::Missing));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "done");
    h.step(Event::Write { owner, write: Write::DeletePage { repository: 0, name: bytes(b"m") }, resumed: false });
    let made = h.send_one();
    let told = h.answer(&made, Err(Error::Missing));
    assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Forge(Error::Missing)) }], "never there");
}

#[test]
fn a_write_failing_for_a_while_gives_up_after_its_attempts_and_the_rate_counts_none() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    h.step(Event::Write { owner, write: Write::Close { item: item(1) }, resumed: false });
    let sent = h.send_one();
    h.answer(&sent, Err(Error::RateLimited { reset: at(5) }));
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
    h.step(Event::Write { owner, write: Write::Record { item: item(9), payload }, resumed: false });
    let post = h.send_one();
    let position = Position { comment: 1, ..Position::START };
    assert_eq!(post.op, Op::Post { number: 9, key: None, body: Body::Record { payload, position } }, "posted");
    h.answer(&post, Err(Error::Timeout));
    h.at(3);
    h.fire();
    let find = h.send_one();
    assert_eq!(find.op, Op::Item { number: 9, after: 1 }, "looked for");
    let told = h.answer(&find, item_page(issue(9, &[HAND_IN], 4), comments(&[record(2, 9, position)]), false));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "found, not posted twice");
    h.step(Event::Write { owner, write: Write::Record { item: item(9), payload }, resumed: false });
    let check = h.send_one();
    assert_eq!(check.op, Op::Comment { number: 9, id: 2 }, "read afresh before it is edited");
    h.answer(&check, Ok(Answer::Comment(record(2, 9, position))));
    let edit = h.send_one();
    assert_eq!(edit.op, Op::EditComment { number: 9, id: 2, body: Body::Record { payload, position } }, "edited");
    let told = h.answer(&edit, Ok(Answer::Edited { revision: 50 }));
    assert_eq!(*told, [Request::Wrote { owner, result: Ok(Written::Done) }], "done");
    h.step(Event::Write { owner, write: Write::Record { item: item(9), payload }, resumed: false });
    let check = h.send_one();
    let mut edited = record(2, 9, position);
    edited.revision = 50;
    h.answer(&check, Ok(Answer::Comment(edited)));
    assert_eq!(
        h.send_one().op,
        Op::EditComment { number: 9, id: 2, body: Body::Record { payload, position } },
        "its own revision"
    );
}

#[test]
fn a_record_someone_else_changed_is_not_written_over() {
    let mut h = Harness::new(LIMITS);
    h.start(&[issue(5, &[TRACKING], 1)], &[]);
    let owner = Token::new(1);
    let write = Write::Record { item: item(5), payload: Token::new(3) };
    h.step(Event::Write { owner, write, resumed: false });
    let check = h.send_one();
    assert_eq!(check.op, Op::Comment { number: 5, id: 100 }, "read afresh");
    let mangled = Comment { id: 100, author: ENGINE, revision: 999, mark: Mark::Mangled, body: bytes(b"?") };
    let told = h.answer(&check, Ok(Answer::Comment(mangled)));
    let record = Record::Mangled { comment: 100, revision: 999 };
    assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Edited { record }) }], "held for a person");
    assert!(h.facts().contains(&Fact::Edited { item: item(5) }), "told as a fact");
    // The parent, released by a person, writes over the record as it now is.
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload: Token::new(3) }, resumed: false });
    let check = h.send_one();
    let mangled = Comment { id: 100, author: ENGINE, revision: 999, mark: Mark::Mangled, body: bytes(b"?") };
    h.answer(&check, Ok(Answer::Comment(mangled)));
    let edit = Op::EditComment {
        number: 5,
        id: 100,
        body: Body::Record { payload: Token::new(3), position: Position::START },
    };
    assert_eq!(h.send_one().op, edit, "edited over the record it last read");
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
    assert_eq!(sent[0].op, Op::Pull { number: 1, reviews: 0 }, "the read first");
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
    h.step(Event::Write { owner: Token::new(1), write: Write::Close { item: item(1) }, resumed: false });
    h.step(Event::Read { owner: Token::new(2), read: Read::Pull { item: item(2) } });
    let sent = h.send();
    assert_eq!(sent.len(), 2, "two at once: {sent:?}");
    assert_eq!(sent[0].op, Op::Pull { number: 2, reviews: 0 }, "the fresh read first");
    assert_eq!(sent[1].op, Op::Close { number: 1 }, "then the write");
    h.answer(&sent[0], Ok(Answer::Pull(pull(2, HEAD, &[], &[]))));
    let next = h.send_one();
    assert_eq!(next.op, changes(0, 1), "keeping up after them");
    let facts = h.facts();
    assert!(facts.contains(&Fact::Sent { priority: Priority::Fresh }), "{facts:?}");
}

#[test]
fn a_rate_limit_refusal_holds_every_call_until_its_reset() {
    let mut h = Harness::started(LIMITS);
    let owner = Token::new(1);
    h.step(Event::Read { owner, read: Read::Pull { item: item(1) } });
    let sent = h.send_one();
    h.step(Event::Write { owner, write: Write::Close { item: item(2) }, resumed: false });
    let told = h.answer(&sent, Err(Error::RateLimited { reset: at(40) }));
    assert!(told.is_empty(), "the read waits");
    assert!(h.send().is_empty(), "nothing goes out before the reset");
    h.at(30);
    h.fire();
    assert!(h.send().is_empty(), "not even keeping up");
    h.at(40);
    h.fire();
    let sent = h.send();
    assert_eq!(sent[0].op, Op::Pull { number: 1, reviews: 0 }, "the read again, first: {sent:?}");
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
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload }, resumed: false });
    let check = h.send_one();
    h.answer(&check, Ok(Answer::Comment(record(100, 5, Position::START))));
    for secs in [3, 10, 30] {
        let edit = h.send_one();
        assert_eq!(
            edit.op,
            Op::EditComment { number: 5, id: 100, body: Body::Record { payload, position: Position::START } }
        );
        let told = h.answer(&edit, Err(Error::Timeout));
        if secs == 30 {
            assert_eq!(*told, [Request::Wrote { owner, result: Err(Failure::Forge(Error::Timeout)) }], "gave up");
        } else {
            h.at(secs);
            h.fire();
        }
    }
    // The edit may have landed: the record says what it carried, at a
    // revision not known.
    h.step(Event::Write { owner, write: Write::Record { item: item(5), payload }, resumed: false });
    let check = h.sends_for(&Op::Comment { number: 5, id: 100 });
    let mut landed = record(100, 5, Position::START);
    landed.revision = 999;
    h.answer(&check, Ok(Answer::Comment(landed)));
    let edit = h.send_one();
    assert_eq!(
        edit.op,
        Op::EditComment { number: 5, id: 100, body: Body::Record { payload, position: Position::START } },
        "its own: edited, not held"
    );
}
