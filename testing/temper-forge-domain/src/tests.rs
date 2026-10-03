//! Feed the forge calls, inspect its answers, webhooks and observations.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token};

use crate::api::{
    Answer, Change, Check, Checks, Comment, Created, Cue, Error, File, Git, Head, Kind, Op, Page, PageName, Permission,
    Protection, Pull, Pushed, Read, Setup, State, Summary, Verdict, Want, What, Write,
};
use crate::{
    Branches, Config, Domain, Event, Limits, MAX_OUT, Observation, Operation, Request, Skew, fire, step, worst_case,
};

const LIMITS: Limits = Limits {
    repositories: 2,
    users: 8,
    labels: 3,
    items: 6,
    comments: 4,
    dependencies: 3,
    reviews: 6,
    branches: 4,
    commits: 32,
    files: 4,
    statuses: 8,
    contexts: 2,
    pages: 3,
    name_bytes: 16,
    title_bytes: 16,
    body_bytes: 32,
    content_bytes: 32,
    page_size: 2,
    calls: 4,
    hooks: 16,
    observations: 32,
};

/// Calls answered in a second, webhooks in two, and no faults unless a test
/// asks for them.
const CALM: Config = Config {
    limits: LIMITS,
    latency_min: Duration::from_secs(1),
    latency_max: Duration::from_secs(1),
    late: 0,
    late_min: Duration::from_secs(30),
    late_max: Duration::from_secs(30),
    unavailable: 0,
    timeouts: 0,
    landing: 0,
    land_min: Duration::from_secs(3),
    land_max: Duration::from_secs(3),
    rate_limit: 0,
    rate_window: Duration::from_secs(60),
    ci: CI,
    hook_min: Duration::from_secs(2),
    hook_max: Duration::from_secs(2),
    hooks_late: 0,
    hooks_lost: 0,
    resolution: Duration::from_secs(1),
    skew: Skew::None,
    status_updates: false,
    edit_updates: false,
};

/// The users: the engine and CI write, a person reads, a maintainer writes,
/// an admin administers, and a stranger has no permission.
const ENGINE: u64 = 1;
const PERSON: u64 = 2;
const CI: u64 = 3;
const STRANGER: u64 = 4;
const ADMIN: u64 = 5;
const MAINTAINER: u64 = 6;

const REPOSITORY: &[u8] = b"ai/temper";
const MAIN: &[u8] = b"main";

/// The first commit of the first repository set up.
const FIRST: u64 = 1;

/// A repository whose `main` holds a readme and a CI cue file, with two
/// labels, one CI context passing after five seconds, and a subscriber.
fn setup() -> Setup {
    Setup {
        name: copy_of(REPOSITORY),
        default: copy_of(MAIN),
        tree: files(&[(b"README", b"hello"), (b"ci", b"green")]),
        labels: names(&[b"temper", b"bug"]),
        checks: Checks {
            contexts: names(&[b"ci"]),
            latency_min: Duration::from_secs(5),
            latency_max: Duration::from_secs(5),
            silent: 0,
            passes: 1000,
            reruns: 0,
            cue: None,
        },
        protection: None,
        hooked: true,
    }
}

fn files(files: &[(&[u8], &[u8])]) -> Box<[File]> {
    let mut list = List::with_capacity(u32::try_from(files.len()).expect("few"));
    for &(path, content) in files {
        list.push(File { path: copy_of(path), content: copy_of(content) }).expect("room");
    }
    list.into_boxed()
}

fn names(names: &[&[u8]]) -> Box<[Box<[u8]>]> {
    let mut list = List::with_capacity(u32::try_from(names.len()).expect("few"));
    for &name in names {
        list.push(copy_of(name)).expect("room");
    }
    list.into_boxed()
}

fn write(write: Write) -> Op {
    Op::Write(write)
}

fn read(read: Read) -> Op {
    Op::Read(read)
}

fn create(title: &[u8], body: &[u8], labels: &[&[u8]]) -> Op {
    write(Write::CreateIssue { title: copy_of(title), body: copy_of(body), labels: names(labels) })
}

fn edit(id: u64, body: &[u8]) -> Op {
    write(Write::EditComment { id, body: copy_of(body) })
}

fn set_labels(number: u64, labels: &[&[u8]]) -> Op {
    write(Write::SetLabels { number, labels: names(labels) })
}

fn define(name: &[u8]) -> Op {
    write(Write::DefineLabel { name: copy_of(name) })
}

fn create_branch(branch: &[u8], commit: u64) -> Op {
    Op::Git(Git::Create { branch: copy_of(branch), commit })
}

fn delete_branch(branch: &[u8]) -> Op {
    write(Write::DeleteBranch { branch: copy_of(branch) })
}

fn fetch(want: Want) -> Op {
    Op::Git(Git::Fetch { want })
}

fn merge(number: u64, head: u64) -> Op {
    write(Write::Merge { number, head })
}

fn review(number: u64, verdict: Verdict) -> Op {
    write(Write::Review { number, verdict: Some(verdict), body: copy_of(b"looked") })
}

fn status(commit: u64, context: &[u8], state: Check) -> Op {
    write(Write::Status { commit, context: copy_of(context), state })
}

fn put(name: &[u8], content: &[u8]) -> Op {
    write(Write::PutPage { name: copy_of(name), content: copy_of(content) })
}

/// The states of the statuses on a pull request's head, in their contexts'
/// order.
fn checks(pull: &Pull) -> List<Check> {
    let mut states = List::with_capacity(LIMITS.contexts);
    for status in &pull.statuses {
        states.push(status.state).expect("no more than the contexts");
    }
    states
}

/// A webhook: what changed, the item, the branch and the commit it names.
type Heard = (Change, Option<u64>, Option<Box<[u8]>>, Option<u64>);

/// The forge, its environment, room for one step's output, and the webhooks
/// heard so far.
struct Harness {
    domain: Domain,
    env: Env<Config>,
    out: Queue<Request>,
    hooks: Queue<Heard>,
    /// The calls answered while settling.
    answered: Queue<Token>,
    calls: u64,
}

impl Harness {
    fn new(config: Config) -> Harness {
        Harness::with(config, setup())
    }

    /// A forge with the repository `setup` describes, and the users.
    fn with(config: Config, setup: Setup) -> Harness {
        Harness::seeded(config, setup, 7)
    }

    fn seeded(config: Config, setup: Setup, seed: u64) -> Harness {
        let mut domain = Domain::new(&config, seed);
        let name = copy_of(&setup.name);
        assert_eq!(crate::repository(&mut domain, &config, setup), FIRST);
        for (user, permission) in [
            (ENGINE, Permission::Write),
            (PERSON, Permission::Read),
            (CI, Permission::Write),
            (ADMIN, Permission::Admin),
            (MAINTAINER, Permission::Write),
        ] {
            crate::grant(&mut domain, &name, user, permission);
        }
        Harness {
            domain,
            env: Env { now: Time::ZERO, limits: config },
            out: Queue::with_capacity(MAX_OUT),
            hooks: Queue::with_capacity(64),
            answered: Queue::with_capacity(LIMITS.calls),
            calls: 0,
        }
    }

    /// Calls the forge as `user`, and waits for the answer, firing what falls
    /// due before it.
    fn call(&mut self, user: u64, op: Op) -> Result<Answer, Error> {
        self.call_on(REPOSITORY, user, op)
    }

    fn call_on(&mut self, repository: &[u8], user: u64, op: Op) -> Result<Answer, Error> {
        self.calls = self.calls.checked_add(1).expect("few calls");
        let token = Token::new(self.calls);
        let event = Event::Call { reply_to: ReplyTo::new(token), user, repository: copy_of(repository), op };
        step(&mut self.domain, &self.env, event, &mut self.out);
        for _ in 0_u32..256 {
            if let Some(request) = self.out.pop() {
                match request {
                    Request::Reply { to, result } => {
                        assert_eq!(to.into_token(), token, "the answer to this call");
                        self.domain.reclaim();
                        return result;
                    }
                    Request::Hook { repository: _, change, number, branch, commit } => {
                        self.hooks.push((change, number, branch, commit));
                    }
                }
                continue;
            }
            self.env.now = self.domain.next_deadline().expect("the call's timer is armed");
            fire(&mut self.domain, &self.env, &mut self.out);
        }
        unreachable!("the call is answered");
    }

    fn ok(&mut self, user: u64, op: Op) -> Answer {
        self.call(user, op).expect("the call succeeds")
    }

    /// Fires every timer, hearing every webhook.
    fn settle(&mut self) {
        for _ in 0_u32..1024 {
            let Some(at) = self.domain.next_deadline() else {
                self.domain.reclaim();
                return;
            };
            self.env.now = self.env.now.max(at);
            fire(&mut self.domain, &self.env, &mut self.out);
            if let Some(request) = self.out.pop() {
                match request {
                    Request::Hook { repository: _, change, number, branch, commit } => {
                        self.hooks.push((change, number, branch, commit));
                    }
                    Request::Reply { to, result: _ } => self.answered.push(to.into_token()),
                }
            }
        }
        unreachable!("the forge settles");
    }

    /// Fires every timer due by `at`, hearing every webhook, and moves the
    /// clock to `at`.
    fn until(&mut self, at: Time) {
        for _ in 0_u32..1024 {
            match self.domain.next_deadline() {
                Some(next) if next <= at => self.env.now = self.env.now.max(next),
                Some(_) | None => {
                    self.env.now = at;
                    self.domain.reclaim();
                    return;
                }
            }
            fire(&mut self.domain, &self.env, &mut self.out);
            if let Some(request) = self.out.pop() {
                match request {
                    Request::Hook { repository: _, change, number, branch, commit } => {
                        self.hooks.push((change, number, branch, commit));
                    }
                    Request::Reply { to, result: _ } => self.answered.push(to.into_token()),
                }
            }
        }
        unreachable!("the forge settles");
    }

    fn wait(&mut self, span: Duration) {
        self.env.now = self.env.now.saturating_add(span);
    }

    fn issue(&mut self, user: u64, title: &[u8]) -> u64 {
        let Answer::Created(number) = self.ok(user, create(title, b"body", &[])) else {
            unreachable!("an issue opened");
        };
        number
    }

    fn comment(&mut self, user: u64, number: u64, body: &[u8]) -> u64 {
        let Answer::Commented(id) = self.ok(user, write(Write::Comment { number, body: copy_of(body) })) else {
            unreachable!("a comment posted");
        };
        id
    }

    fn inspect(&self, read: &Read) -> Result<Answer, Error> {
        self.domain.inspect(&self.env.limits, REPOSITORY, read)
    }

    fn item(&self, number: u64) -> (Summary, Box<[Comment]>) {
        let Ok(Answer::Item { item, comments, more: _ }) = self.inspect(&Read::Item { number, after: 0 }) else {
            unreachable!("an item");
        };
        (item, comments)
    }

    /// The numbers on page `page` of the items in `state` carrying
    /// `labels`, and whether a later page has more.
    fn list(&mut self, user: u64, state: Option<State>, labels: &[&[u8]], page: u32) -> (List<u64>, bool) {
        let op = read(Read::Items {
            state,
            kind: None,
            labels: names(labels),
            author: None,
            since: Time::ZERO,
            page,
            limit: 0,
        });
        let Answer::Items { items, more, .. } = self.ok(user, op) else {
            unreachable!("a page of items");
        };
        let mut numbers = List::with_capacity(LIMITS.page_size);
        for item in &items {
            numbers.push(item.number).expect("a page");
        }
        (numbers, more)
    }

    /// Where `branch` is.
    fn branch(&self, branch: &[u8]) -> Option<u64> {
        let found = self.inspect(&Read::Branch { branch: copy_of(branch) });
        if found == Err(Error::Missing(What::Branch)) {
            return None;
        }
        let Ok(Answer::Commit(commit)) = found else {
            unreachable!("a branch");
        };
        Some(commit)
    }

    /// A commit on `parent` of its tree with `changes` written.
    fn commit(&mut self, parent: u64, changes: &[(&[u8], &[u8])]) -> u64 {
        let mut tree = crate::git::copy_tree(&LIMITS, &self.domain.object(parent).expect("a parent").tree);
        for &(path, content) in changes {
            tree.insert(copy_of(path), copy_of(content)).expect("a tree within the limits");
        }
        let mut list = List::with_capacity(tree.len());
        for (path, content) in &tree {
            list.push(File { path: copy_of(path), content: copy_of(content) }).expect("room");
        }
        crate::commit(&mut self.domain, &self.env.limits, parent, list.into_boxed()).expect("room").expect("a change")
    }

    fn push(&mut self, user: u64, branch: &[u8], commit: u64) -> Result<Answer, Error> {
        self.call(user, Op::Git(Git::Push { branch: copy_of(branch), commit }))
    }

    fn pull(&self, number: u64) -> Pull {
        let Ok(Answer::Pull(pull)) = self.inspect(&Read::Pull { number }) else {
            unreachable!("a pull request");
        };
        pull
    }

    /// Opens a pull request of `head` into `main`, as the engine.
    fn open(&mut self, head: &[u8]) -> Result<Answer, Error> {
        let op = open_op(head);
        self.call(ENGINE, op)
    }

    /// Takes what the forge emitted, noting each answer's token, which must
    /// be new.
    fn drain(&mut self, answered: &mut temper_lib::Set<u64>) {
        while let Some(request) = self.out.pop() {
            match request {
                Request::Reply { to, result: _ } => {
                    let fresh = answered.insert(to.into_token().raw()).expect("room");
                    assert!(fresh, "a call is answered once");
                }
                Request::Hook { .. } => {}
            }
        }
    }

    /// The observations not drained yet.
    fn observations(&mut self) -> List<Observation> {
        let mut observations = List::with_capacity(LIMITS.observations);
        while let Some(observation) = self.domain.pop_observation() {
            observations.push(observation).expect("room");
        }
        observations
    }

    /// The webhooks heard so far: what changed, and about which item.
    fn heard(&mut self) -> List<(Change, Option<u64>)> {
        let mut hooks = List::with_capacity(64);
        while let Some((change, number, _, _)) = self.hooks.pop() {
            hooks.push((change, number)).expect("room");
        }
        hooks
    }

    /// The webhooks heard so far, all they say.
    fn heard_all(&mut self) -> List<Heard> {
        let mut hooks = List::with_capacity(64);
        while let Some(hook) = self.hooks.pop() {
            hooks.push(hook).expect("room");
        }
        hooks
    }
}

fn repository() -> Box<[u8]> {
    copy_of(REPOSITORY)
}

/// CI reported a commit pending.
fn pending(commit: u64) -> Observation {
    Observation::Reported { repository: repository(), commit, context: copy_of(b"ci"), state: Check::Pending, by: CI }
}

/// The engine moved a branch.
fn moved(branch: &[u8], from: Option<u64>, to: u64) -> Observation {
    Observation::Moved { repository: repository(), branch: copy_of(branch), from, to, by: ENGINE }
}

fn numbers(list: &List<u64>) -> &[u64] {
    list.as_slice()
}

// Store and reads.

#[test]
fn an_issue_is_opened_with_labels_and_read_back() {
    let mut h = Harness::new(CALM);
    assert_eq!(h.ok(ENGINE, create(b"crash", b"it crashes", &[b"bug", b"temper"])), Answer::Created(1));
    assert_eq!(h.issue(PERSON, b"question"), 2, "issues share one numbering");
    let (item, comments) = h.item(1);
    assert_eq!(&*item.title, b"crash");
    assert_eq!(&*item.body, b"it crashes");
    assert_eq!((item.kind, item.state, item.author), (Kind::Issue, State::Open, ENGINE));
    assert_eq!(item.labels, names(&[b"bug", b"temper"]), "labels in their order");
    assert_eq!(item.created, Time::ZERO);
    assert!(comments.is_empty());
    assert_eq!(h.ok(PERSON, read(Read::Permission { user: STRANGER })), Answer::Permission(Permission::None));
    assert_eq!(h.ok(PERSON, read(Read::Permission { user: ADMIN })), Answer::Permission(Permission::Admin));
    assert_eq!(h.call(STRANGER, read(Read::Item { number: 1, after: 0 })), Err(Error::Forbidden), "no permission");
}

#[test]
fn listings_page_least_recently_updated_first_by_page_number() {
    let mut h = Harness::new(CALM);
    for title in [b"one", b"two", b"six"] {
        h.issue(ENGINE, title);
    }
    h.comment(PERSON, 1, b"bump");
    let (page, more) = h.list(ENGINE, None, &[], 1);
    assert_eq!((numbers(&page), more), (&[2, 3][..], true));
    let (page, more) = h.list(ENGINE, None, &[], 2);
    assert_eq!((numbers(&page), more), (&[1][..], false), "the comment moved the first last");
    assert_eq!(h.list(ENGINE, None, &[], 0).0.as_slice(), [2, 3], "a page before the first is the first");
    assert!(h.list(ENGINE, None, &[], 3).0.is_empty());
    let one = read(Read::Items {
        state: None,
        kind: None,
        labels: names(&[]),
        author: None,
        since: Time::ZERO,
        page: 3,
        limit: 1,
    });
    let Answer::Items { items, more: false, .. } = h.ok(ENGINE, one) else {
        unreachable!("the last page");
    };
    assert_eq!(items[0].number, 1, "a limit below the most a page holds");
    // An item that changes while a client pages moves to the end, and the
    // one after it falls on the page already read, as on Forgejo.
    let (page, _) = h.list(ENGINE, None, &[], 1);
    assert_eq!(numbers(&page), [2, 3]);
    let changed = h.env.now;
    h.comment(PERSON, 2, b"again");
    let (page, more) = h.list(ENGINE, None, &[], 2);
    assert_eq!((numbers(&page), more), (&[2][..], false), "the first is missed this pass");
    let (item, _) = h.item(2);
    assert!(item.updated >= changed, "the pass sees it moved under it");
    h.issue(ENGINE, b"four");
    let (page, more) = h.list(ENGINE, None, &[], 2);
    assert_eq!((numbers(&page), more), (&[2, 4][..], false), "an exactly full last page has no more");
}

#[test]
fn listings_are_at_the_forges_resolution_with_since_inclusive_and_ties_by_number() {
    let mut h = Harness::new(Config {
        latency_min: Duration::from_millis(100),
        latency_max: Duration::from_millis(100),
        ..CALM
    });
    h.wait(Duration::from_millis(1500));
    for title in [b"one", b"two", b"six"] {
        h.issue(ENGINE, title);
    }
    h.comment(PERSON, 3, b"first");
    h.comment(PERSON, 1, b"then");
    let (item, _) = h.item(3);
    assert_eq!(item.updated, Time::ZERO.saturating_add(Duration::from_secs(1)), "kept in seconds");
    assert_eq!(numbers(&h.list(ENGINE, None, &[], 1).0), [1, 2], "the same second, by number");
    h.wait(Duration::from_secs(1));
    h.comment(PERSON, 2, b"later");
    let since = Time::ZERO.saturating_add(Duration::from_millis(2900));
    let recent =
        read(Read::Items { state: None, kind: None, labels: names(&[]), author: None, since, page: 1, limit: 0 });
    let Answer::Items { items, more: false, .. } = h.ok(ENGINE, recent) else {
        unreachable!("one page");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].number, 2, "since is taken at the resolution, inclusive");
    let mut h = Harness::new(Config { resolution: Duration::ZERO, ..CALM });
    h.wait(Duration::from_millis(1500));
    h.issue(ENGINE, b"one");
    assert_eq!(h.item(1).0.created, Time::ZERO.saturating_add(Duration::from_millis(1500)), "the clock's own");
}

#[test]
fn statuses_and_comment_edits_move_updated_times_only_where_configured() {
    let mut h = Harness::new(Config { status_updates: true, edit_updates: true, ..CALM });
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.settle();
    assert_eq!(h.item(1).0.updated, h.pull(1).statuses[0].at, "the verdict updated the pull request");
    let id = h.comment(PERSON, 1, b"typo");
    h.wait(Duration::from_secs(10));
    h.ok(PERSON, edit(id, b"fixed"));
    let (item, comments) = h.item(1);
    assert_eq!(comments[0].edited, Some(item.updated), "the edit updated the item");
    h.wait(Duration::from_secs(10));
    let deleted = h.env.now;
    h.ok(PERSON, write(Write::DeleteComment { id }));
    assert_eq!(h.item(1).0.updated, deleted, "so did the deletion");
}

#[test]
fn listings_filter_by_state_kind_labels_and_time() {
    let mut h = Harness::new(CALM);
    h.issue(ENGINE, b"one");
    h.ok(ENGINE, create(b"two", b"", &[b"temper"]));
    h.ok(ENGINE, write(Write::Close { number: 1 }));
    let (page, _) = h.list(ENGINE, Some(State::Open), &[], 1);
    assert_eq!(numbers(&page), [2]);
    let (page, _) = h.list(ENGINE, Some(State::Closed), &[], 1);
    assert_eq!(numbers(&page), [1]);
    let (page, _) = h.list(ENGINE, None, &[b"temper"], 1);
    assert_eq!(numbers(&page), [2]);
    let (page, _) = h.list(ENGINE, None, &[b"temper", b"bug"], 1);
    assert!(page.is_empty(), "every label must be there");
    let since = h.env.now;
    let pulls = read(Read::Items {
        state: None,
        kind: Some(Kind::Pull),
        labels: names(&[]),
        author: None,
        since: Time::ZERO,
        page: 1,
        limit: 0,
    });
    let Answer::Items { items, more: false, .. } = h.ok(ENGINE, pulls) else {
        unreachable!("one page");
    };
    assert!(items.is_empty(), "no pull requests");
    h.comment(PERSON, 2, b"later");
    let recent = read(Read::Items {
        state: None,
        kind: Some(Kind::Issue),
        labels: names(&[]),
        author: None,
        since,
        page: 1,
        limit: 0,
    });
    let Answer::Items { items, more: false, .. } = h.ok(ENGINE, recent) else {
        unreachable!("one page");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].number, 2, "only what changed since");
}

#[test]
fn an_items_comments_come_a_page_at_a_time_after_an_id() {
    let mut h = Harness::new(CALM);
    let number = h.issue(ENGINE, b"talk");
    let other = h.issue(ENGINE, b"other");
    let first = h.comment(PERSON, number, b"one");
    let elsewhere = h.comment(PERSON, other, b"elsewhere");
    let second = h.comment(ENGINE, number, b"two");
    let third = h.comment(PERSON, number, b"three");
    assert!(first < elsewhere && elsewhere < second && second < third, "ids only grow, across items");
    let Answer::Item { comments, more: true, .. } = h.ok(ENGINE, read(Read::Item { number, after: 0 })) else {
        unreachable!("a first page, and more");
    };
    assert_eq!([comments[0].id, comments[1].id], [first, second]);
    assert_eq!(&*comments[1].body, b"two");
    assert_eq!(comments[1].author, ENGINE);
    let Answer::Item { comments, more: false, .. } = h.ok(ENGINE, read(Read::Item { number, after: second })) else {
        unreachable!("the last page");
    };
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, third);
}

// Writes and refusals.

#[test]
fn a_comment_is_edited_or_deleted_by_its_author_or_a_writer() {
    let mut h = Harness::new(CALM);
    let number = h.issue(ENGINE, b"talk");
    let id = h.comment(PERSON, number, b"typo");
    let record = h.comment(ENGINE, number, b"the record");
    assert_eq!(h.call(PERSON, edit(record, b"mangled")), Err(Error::Forbidden), "a reader edits only theirs");
    assert_eq!(h.call(PERSON, edit(id, b"")), Err(Error::Empty));
    h.wait(Duration::from_secs(10));
    assert_eq!(h.ok(PERSON, edit(id, b"fixed")), Answer::Done);
    let (item, comments) = h.item(number);
    assert_eq!(&*comments[0].body, b"fixed");
    assert!(item.updated < comments[0].edited.expect("edited"), "an edit leaves the item's updated time");
    assert_eq!(h.ok(MAINTAINER, edit(record, b"mangled")), Answer::Done, "a writer mangles the engine's record");
    assert_eq!(&*h.item(number).1[1].body, b"mangled");
    assert_eq!(h.call(PERSON, write(Write::DeleteComment { id: record })), Err(Error::Forbidden));
    assert_eq!(h.ok(MAINTAINER, write(Write::DeleteComment { id })), Answer::Done);
    assert_eq!(h.call(PERSON, write(Write::DeleteComment { id })), Err(Error::Missing(What::Comment)));
    assert_eq!(h.call(PERSON, edit(id, b"gone")), Err(Error::Missing(What::Comment)));
    assert_eq!(h.item(number).1.len(), 1);
    assert_eq!(h.call(PERSON, write(Write::Comment { number, body: copy_of(b"") })), Err(Error::Empty));
}

#[test]
fn labels_are_defined_before_they_are_set_and_set_as_a_whole() {
    let mut h = Harness::new(CALM);
    let number = h.issue(PERSON, b"triage");
    assert_eq!(h.call(ENGINE, set_labels(number, &[b"later"])), Err(Error::Missing(What::Label)));
    assert_eq!(h.call(PERSON, set_labels(number, &[b"bug"])), Err(Error::Forbidden), "labelling needs write");
    assert_eq!(h.call(PERSON, define(b"later")), Err(Error::Forbidden));
    assert_eq!(h.ok(ENGINE, define(b"later")), Answer::Done);
    assert_eq!(h.call(ENGINE, define(b"later")), Err(Error::Exists));
    assert_eq!(h.call(ENGINE, define(b"more")), Err(Error::Full));
    assert_eq!(h.ok(ENGINE, set_labels(number, &[b"later", b"bug", b"later"])), Answer::Done);
    assert_eq!(h.item(number).0.labels, names(&[b"bug", b"later"]));
    assert_eq!(h.ok(ENGINE, set_labels(number, &[b"temper"])), Answer::Done);
    assert_eq!(h.item(number).0.labels, names(&[b"temper"]), "a set replaces");
    assert_eq!(h.call(ENGINE, set_labels(number, &[b"a", b"b", b"c", b"d"])), Err(Error::TooLarge));
    assert_eq!(h.ok(PERSON, create(b"t", b"", &[b"bug"])), Answer::Created(2));
    assert!(h.item(2).0.labels.is_empty(), "the labels of a user who may not label are dropped");
    assert_eq!(h.call(ENGINE, create(b"", b"body", &[])), Err(Error::Empty), "an issue has a title");
}

#[test]
fn labels_are_added_and_removed_leaving_the_others() {
    let mut h = Harness::new(CALM);
    let number = h.issue(PERSON, b"triage");
    let add = |labels: &[&[u8]]| write(Write::AddLabels { number, labels: names(labels) });
    let remove = |labels: &[&[u8]]| write(Write::RemoveLabels { number, labels: names(labels) });
    assert_eq!(h.call(PERSON, add(&[b"bug"])), Err(Error::Forbidden), "labelling needs write");
    assert_eq!(h.call(ENGINE, add(&[b"later"])), Err(Error::Missing(What::Label)));
    assert_eq!(h.ok(MAINTAINER, set_labels(number, &[b"bug"])), Answer::Done);
    h.observations();
    assert_eq!(h.ok(ENGINE, add(&[b"temper"])), Answer::Done);
    assert_eq!(h.item(number).0.labels, names(&[b"bug", b"temper"]), "added beside the others");
    let labelled =
        Observation::Labelled { repository: repository(), number, labels: names(&[b"bug", b"temper"]), by: ENGINE };
    assert_eq!(h.observations().as_slice(), [labelled], "observed");
    let updated = h.item(number).0.updated;
    h.wait(Duration::from_secs(5));
    assert_eq!(h.ok(ENGINE, add(&[b"temper"])), Answer::Done);
    assert_eq!(h.ok(ENGINE, remove(&[b"later"])), Answer::Done, "a label it does not carry");
    assert_eq!(h.item(number).0.updated, updated, "what changes nothing moves nothing");
    assert!(h.observations().is_empty(), "nor is observed");
    assert_eq!(h.ok(ENGINE, remove(&[b"temper"])), Answer::Done);
    assert_eq!(h.item(number).0.labels, names(&[b"bug"]), "removed, the others left");
}

#[test]
fn listings_filter_by_who_opened_the_items() {
    let mut h = Harness::new(CALM);
    h.issue(ENGINE, b"ours");
    h.issue(PERSON, b"theirs");
    let by = |author: u64| {
        read(Read::Items {
            state: None,
            kind: None,
            labels: names(&[]),
            author: Some(author),
            since: Time::ZERO,
            page: 1,
            limit: 0,
        })
    };
    let Answer::Items { items, more: false, .. } = h.ok(PERSON, by(ENGINE)) else {
        unreachable!("one page");
    };
    assert_eq!(items.len(), 1, "the engine's only");
    assert_eq!(items[0].number, 1);
    let Answer::Items { items, more: false, .. } = h.ok(PERSON, by(PERSON)) else {
        unreachable!("one page");
    };
    assert_eq!(items[0].number, 2, "the person's");
}

#[test]
fn an_item_is_closed_and_reopened_by_its_author_or_a_writer() {
    let mut h = Harness::new(CALM);
    let number = h.issue(PERSON, b"mine");
    let theirs = h.issue(ENGINE, b"theirs");
    assert_eq!(h.call(PERSON, write(Write::Close { number: theirs })), Err(Error::Forbidden));
    assert_eq!(h.ok(PERSON, write(Write::Close { number })), Answer::Done);
    assert_eq!(h.ok(PERSON, write(Write::Close { number })), Answer::Done, "closing again changes nothing");
    assert_eq!(h.item(number).0.state, State::Closed);
    assert_eq!(h.ok(ENGINE, write(Write::Reopen { number })), Answer::Done);
    assert_eq!(h.item(number).0.state, State::Open);
    assert_eq!(h.ok(ENGINE, write(Write::Close { number: theirs })), Answer::Done);
    h.comment(PERSON, theirs, b"still talking");
    assert_eq!(h.item(theirs).1.len(), 1, "a closed item takes comments");
}

#[test]
fn what_is_past_the_limits_is_refused() {
    let mut h = Harness::new(CALM);
    let long = [b'x'; 40];
    assert_eq!(h.call(ENGINE, create(&long, b"", &[])), Err(Error::TooLarge));
    assert_eq!(h.call(ENGINE, create(b"", &long, &[])), Err(Error::TooLarge));
    let number = h.issue(ENGINE, b"one");
    assert_eq!(h.call(ENGINE, write(Write::Comment { number, body: copy_of(&long) })), Err(Error::TooLarge));
    for _ in 0..LIMITS.comments {
        h.comment(ENGINE, number, b"c");
    }
    assert_eq!(h.call(ENGINE, write(Write::Comment { number, body: copy_of(b"c") })), Err(Error::Full));
    for _ in 1..LIMITS.items {
        h.issue(ENGINE, b"more");
    }
    assert_eq!(h.call(ENGINE, create(b"t", b"", &[])), Err(Error::Full));
}

#[test]
fn what_is_missing_is_refused() {
    let mut h = Harness::new(CALM);
    h.issue(ENGINE, b"one");
    assert_eq!(
        h.call_on(b"ai/nowhere", ENGINE, read(Read::Item { number: 1, after: 0 })),
        Err(Error::Missing(What::Repository))
    );
    assert_eq!(h.call(ENGINE, read(Read::Item { number: 9, after: 0 })), Err(Error::Missing(What::Item)));
    assert_eq!(
        h.call(ENGINE, write(Write::Comment { number: 9, body: copy_of(b"hello") })),
        Err(Error::Missing(What::Item))
    );
    assert_eq!(h.call(ENGINE, read(Read::Pull { number: 1 })), Err(Error::Missing(What::Pull)), "an issue");
    assert_eq!(h.call(ENGINE, read(Read::Branch { branch: copy_of(b"gone") })), Err(Error::Missing(What::Branch)));
    assert_eq!(h.call(ENGINE, read(Read::Tree { commit: 99 })), Err(Error::Missing(What::Commit)));
    let file = read(Read::File { commit: FIRST, path: copy_of(b"nothing") });
    assert_eq!(h.call(ENGINE, file), Err(Error::Missing(What::File)));
    let readme = read(Read::File { commit: FIRST, path: copy_of(b"README") });
    assert_eq!(h.ok(PERSON, readme), Answer::File(copy_of(b"hello")));
    assert_eq!(h.call(ENGINE, read(Read::Page { name: copy_of(b"home") })), Err(Error::Missing(What::Page)));
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the test limits fit");
    let stored = u64::from(LIMITS.commits * LIMITS.files * LIMITS.content_bytes);
    assert!(bound > stored, "it counts the store");
    let more = worst_case(&Limits { items: LIMITS.items + 1, ..LIMITS }).expect("fits");
    assert!(more > bound, "an item more is more");
    assert_eq!(worst_case(&Limits { content_bytes: u32::MAX, commits: u32::MAX, files: u32::MAX, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { items: u32::MAX, comments: u32::MAX, ..LIMITS }), None);
}

// Git.

#[test]
fn a_push_fast_forwards_or_creates_and_is_otherwise_rejected() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    let more = h.commit(work, &[(b"src", b"two")]);
    let aside = h.commit(FIRST, &[(b"src", b"else")]);
    assert_eq!(h.push(ENGINE, b"work", work), Ok(Answer::Pushed(Pushed::Pushed)), "created");
    assert_eq!(h.push(ENGINE, b"work", more), Ok(Answer::Pushed(Pushed::Pushed)), "fast-forwarded");
    assert_eq!(h.push(ENGINE, b"work", more), Ok(Answer::Pushed(Pushed::Pushed)), "already there");
    assert_eq!(h.push(ENGINE, b"work", aside), Ok(Answer::Pushed(Pushed::Rejected)), "not a fast-forward");
    assert_eq!(h.push(ENGINE, b"work", work), Ok(Answer::Pushed(Pushed::Rejected)), "never backwards");
    assert_eq!(h.branch(b"work"), Some(more));
    assert_eq!(h.push(PERSON, b"work", more), Err(Error::Forbidden), "pushing needs write");
    assert_eq!(h.push(ENGINE, b"work", 99), Err(Error::Missing(What::Commit)));
    let tree = read(Read::Tree { commit: work });
    assert!(h.call(PERSON, tree).is_ok(), "a push brings the commits before the one pushed");
    let tree = read(Read::Tree { commit: aside });
    assert_eq!(h.call(PERSON, tree), Err(Error::Missing(What::Commit)), "a commit rejected is not had");
    for branch in [b"b3", b"b4"] {
        assert_eq!(h.push(ENGINE, branch, work), Ok(Answer::Pushed(Pushed::Pushed)));
    }
    assert_eq!(h.push(ENGINE, b"b5", work), Err(Error::Full));
}

#[test]
fn another_party_advancing_a_branch_rejects_a_push_from_where_it_was() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    assert_eq!(h.push(ENGINE, b"work", work), Ok(Answer::Pushed(Pushed::Pushed)));
    let theirs =
        crate::advance(&mut h.domain, &h.env, REPOSITORY, b"work", b"src", b"theirs", PERSON).expect("advanced");
    let ours = h.commit(work, &[(b"src", b"two")]);
    assert_eq!(h.push(ENGINE, b"work", ours), Ok(Answer::Pushed(Pushed::Rejected)));
    assert_eq!(h.branch(b"work"), Some(theirs));
    assert!(h.domain.is_ancestor(work, theirs));
    let on = h.commit(theirs, &[(b"src", b"rebased")]);
    assert_eq!(h.push(ENGINE, b"work", on), Ok(Answer::Pushed(Pushed::Pushed)), "from where it is now");
}

#[test]
fn a_branch_is_created_only_where_none_is() {
    let mut h = Harness::new(CALM);
    assert_eq!(h.ok(ENGINE, create_branch(b"base", FIRST)), Answer::Branch(Created::Created));
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    assert_eq!(h.call(ENGINE, create_branch(b"other", work)), Err(Error::Missing(What::Commit)), "not pushed");
    assert_eq!(h.push(ENGINE, b"work", work), Ok(Answer::Pushed(Pushed::Pushed)));
    assert_eq!(h.ok(ENGINE, create_branch(b"base", work)), Answer::Branch(Created::Exists));
    assert_eq!(h.branch(b"base"), Some(FIRST), "left where it is");
    assert_eq!(h.call(PERSON, create_branch(b"mine", FIRST)), Err(Error::Forbidden));
}

#[test]
fn a_clone_has_every_branch_and_a_fetch_what_it_names() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    let cloned = h.ok(PERSON, Op::Git(Git::Clone));
    let branches = [Head { branch: copy_of(MAIN), commit: FIRST }, Head { branch: copy_of(b"work"), commit: work }];
    assert_eq!(cloned, Answer::Cloned { default: copy_of(MAIN), branches: Box::new(branches) });
    assert_eq!(h.ok(PERSON, fetch(Want::Default)), Answer::Commit(FIRST));
    assert_eq!(h.ok(PERSON, fetch(Want::Branch(copy_of(b"work")))), Answer::Commit(work));
    assert_eq!(h.ok(PERSON, fetch(Want::Commit(work))), Answer::Commit(work));
    assert_eq!(h.call(PERSON, fetch(Want::Branch(copy_of(b"gone")))), Err(Error::Missing(What::Branch)));
    let stray = h.commit(FIRST, &[(b"src", b"stray")]);
    assert_eq!(h.call(PERSON, fetch(Want::Commit(stray))), Err(Error::Missing(What::Commit)));
    assert_eq!(h.call(STRANGER, Op::Git(Git::Clone)), Err(Error::Forbidden));
}

#[test]
fn unreachable_and_refusing_repositories_fail_as_git_would() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    crate::set_refusing(&mut h.domain, REPOSITORY, true);
    assert_eq!(h.push(ENGINE, b"work", work), Err(Error::Refused));
    assert_eq!(h.call(ENGINE, create_branch(b"base", FIRST)), Err(Error::Refused));
    assert_eq!(h.ok(PERSON, fetch(Want::Default)), Answer::Commit(FIRST), "it still serves");
    crate::set_reachable(&mut h.domain, REPOSITORY, false);
    assert_eq!(h.call(PERSON, Op::Git(Git::Clone)), Err(Error::Unreachable));
    assert_eq!(h.call(PERSON, fetch(Want::Default)), Err(Error::Unreachable));
    crate::set_reachable(&mut h.domain, REPOSITORY, true);
    crate::set_refusing(&mut h.domain, REPOSITORY, false);
    assert_eq!(h.push(ENGINE, b"work", work), Ok(Answer::Pushed(Pushed::Pushed)));
}

#[test]
fn a_working_tree_commits_into_the_one_store() {
    let mut h = Harness::new(CALM);
    let same = files(&[(b"README", b"hello"), (b"ci", b"green")]);
    assert_eq!(crate::commit(&mut h.domain, &h.env.limits, FIRST, same), Ok(None), "nothing changed");
    let next = crate::commit(&mut h.domain, &h.env.limits, FIRST, files(&[(b"README", b"bye")])).expect("room");
    assert_eq!(next, Some(2), "named by a count");
    let object = h.domain.object(2).expect("stored");
    assert_eq!(object.parent, Some(FIRST));
    assert_eq!(object.tree.len(), 1, "a tree is the whole tree");
    assert_eq!(crate::commit(&mut h.domain, &h.env.limits, 99, files(&[])), Err(Error::Missing(What::Commit)));
    let wide = files(&[(b"a", b""), (b"b", b""), (b"c", b""), (b"d", b""), (b"e", b"")]);
    assert_eq!(crate::commit(&mut h.domain, &h.env.limits, FIRST, wide), Err(Error::TooLarge));
}

#[test]
fn a_branch_is_deleted_unless_it_is_the_default_or_protected() {
    let mut setup = setup();
    setup.protection =
        Some(Protection { branch: copy_of(b"release"), contexts: names(&[]), approvals: 0, dismiss_stale: false });
    let mut h = Harness::with(CALM, setup);
    h.ok(ENGINE, create_branch(b"work", FIRST));
    assert_eq!(h.call(ENGINE, delete_branch(MAIN)), Err(Error::Protected), "the default");
    let release = h.commit(FIRST, &[(b"src", b"x")]);
    assert_eq!(h.push(ENGINE, b"release", release), Err(Error::Protected));
    assert_eq!(h.call(PERSON, delete_branch(b"work")), Err(Error::Forbidden));
    assert_eq!(h.ok(ENGINE, delete_branch(b"work")), Answer::Done);
    assert_eq!(h.branch(b"work"), None);
    assert_eq!(h.call(ENGINE, delete_branch(b"work")), Err(Error::Missing(What::Branch)));
}

#[test]
fn deleting_a_branch_closes_the_pull_requests_from_or_into_it() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    let next = h.commit(work, &[(b"src", b"two")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.push(ENGINE, b"next", next).expect("pushed");
    h.open(b"work").expect("opened");
    let op = write(Write::OpenPull {
        title: copy_of(b"t"),
        body: copy_of(b""),
        head: copy_of(b"next"),
        base: copy_of(b"work"),
    });
    assert_eq!(h.ok(ENGINE, op), Answer::Created(2));
    h.observations();
    h.ok(MAINTAINER, delete_branch(b"work"));
    assert_eq!((h.pull(1).state, h.pull(2).state), (State::Closed, State::Closed), "its head's, and its base's");
    let closed = [
        Observation::Deleted { repository: repository(), branch: copy_of(b"work"), at: work, by: MAINTAINER },
        Observation::Closed { repository: repository(), number: 1, by: MAINTAINER },
        Observation::Closed { repository: repository(), number: 2, by: MAINTAINER },
    ];
    assert_eq!(h.observations().as_slice(), closed);
    h.push(ENGINE, b"work", next).expect("pushed again");
    assert_eq!(h.pull(1).state, State::Closed, "a new branch of the name revives nothing");
    assert_eq!(h.pull(1).commit, work);
}

#[test]
fn a_protected_branch_is_not_created() {
    let mut setup = setup();
    setup.protection =
        Some(Protection { branch: copy_of(b"release"), contexts: names(&[]), approvals: 0, dismiss_stale: false });
    let mut h = Harness::with(CALM, setup);
    assert_eq!(h.call(ENGINE, create_branch(b"release", FIRST)), Err(Error::Protected));
    assert_eq!(h.push(ENGINE, b"release", FIRST), Err(Error::Protected));
    assert_eq!(h.branch(b"release"), None);
}

// Pull requests, CI, merges and protection.

#[test]
fn a_pull_request_follows_its_head_branch_while_it_is_open() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    assert_eq!(h.open(b"work"), Ok(Answer::Created(1)));
    let pull = h.pull(1);
    assert_eq!((&*pull.head, &*pull.base, pull.commit, pull.base_commit), (&b"work"[..], MAIN, work, Some(FIRST)));
    assert_eq!((pull.state, pull.merged, pull.mergeable), (State::Open, None, true));
    let more = h.commit(work, &[(b"src", b"two")]);
    let pushed = h.env.now;
    h.push(ENGINE, b"work", more).expect("pushed");
    assert_eq!(h.pull(1).commit, more);
    assert_eq!(h.item(1).0.updated, pushed, "a new head updates it, when the push arrives");
    h.ok(ENGINE, write(Write::Close { number: 1 }));
    let after = h.commit(more, &[(b"src", b"three")]);
    h.push(ENGINE, b"work", after).expect("pushed");
    assert_eq!(h.pull(1).commit, more, "a closed pull request stays where it was");
    assert!(!h.pull(1).mergeable);
    h.ok(ENGINE, write(Write::Reopen { number: 1 }));
    assert_eq!(h.pull(1).commit, after, "reopened, it follows its branch again");
}

#[test]
fn opening_a_pull_request_refuses_what_forgejo_refuses() {
    let mut h = Harness::new(CALM);
    assert_eq!(h.issue(PERSON, b"first"), 1);
    assert_eq!(h.open(b"work"), Err(Error::Missing(What::Branch)));
    assert_eq!(h.open(MAIN), Err(Error::NothingToMerge));
    h.ok(ENGINE, create_branch(b"same", FIRST));
    assert_eq!(h.open(b"same"), Err(Error::NothingToMerge), "nothing the base lacks");
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    assert_eq!(h.open(b"work"), Ok(Answer::Created(2)), "one numbering with issues");
    assert_eq!(h.open(b"work"), Err(Error::Exists));
    h.ok(ENGINE, write(Write::Close { number: 2 }));
    assert_eq!(h.open(b"work"), Ok(Answer::Created(3)), "once the first is closed");
    assert_eq!(h.call(ENGINE, write(Write::Reopen { number: 2 })), Err(Error::Exists));
    let op = write(Write::OpenPull {
        title: copy_of(b"t"),
        body: copy_of(b""),
        head: copy_of(b"work"),
        base: copy_of(MAIN),
    });
    assert_eq!(h.call(STRANGER, op), Err(Error::Forbidden));
    h.ok(ENGINE, delete_branch(b"work"));
    h.ok(ENGINE, write(Write::Close { number: 3 }));
    assert_eq!(h.call(ENGINE, write(Write::Reopen { number: 3 })), Err(Error::Missing(What::Branch)));
}

#[test]
fn a_merge_squashes_the_head_onto_the_base() {
    let mut h = Harness::new(CALM);
    let tree = files(&[(b"README", b"hello"), (b"src", b"one")]);
    let work = crate::commit(&mut h.domain, &h.env.limits, FIRST, tree).expect("room").expect("a change");
    let more = h.commit(work, &[(b"src", b"two")]);
    h.push(ENGINE, b"work", more).expect("pushed");
    h.open(b"work").expect("opened");
    let theirs = crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"other", b"x", MAINTAINER).expect("advanced");
    assert_eq!(h.call(PERSON, merge(1, more)), Err(Error::Forbidden), "merging needs write");
    let Answer::Merged(commit) = h.ok(ENGINE, merge(1, more)) else {
        unreachable!("merged");
    };
    let object = h.domain.object(commit).expect("stored");
    assert_eq!(object.parent, Some(theirs), "one commit on the base's tip");
    let expected = files(&[(b"README", b"hello"), (b"other", b"x"), (b"src", b"two")]);
    assert_eq!(h.ok(PERSON, read(Read::Tree { commit })), Answer::Tree(expected), "the deletion merged too");
    assert_eq!(h.branch(MAIN), Some(commit));
    assert_eq!(h.ok(PERSON, fetch(Want::Default)), Answer::Commit(commit), "the next fetch sees it");
    let pull = h.pull(1);
    assert_eq!((pull.state, pull.merged), (State::Closed, Some(commit)));
    assert_eq!(h.call(ENGINE, merge(1, more)), Err(Error::Closed));
    assert_eq!(h.call(ENGINE, write(Write::Reopen { number: 1 })), Err(Error::Closed));
}

#[test]
fn a_merge_conflicts_where_head_and_base_changed_a_path_differently() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"README", b"ours"), (b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"src", b"one", MAINTAINER).expect("advanced");
    assert!(h.pull(1).mergeable, "the same change on both sides merges");
    crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"README", b"theirs", MAINTAINER).expect("advanced");
    assert!(!h.pull(1).mergeable);
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::Conflict));
    assert_eq!(h.pull(1).state, State::Open, "nothing merged");
}

#[test]
fn a_merge_at_a_head_that_moved_is_stale() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    let more = h.commit(work, &[(b"src", b"two")]);
    h.push(ENGINE, b"work", more).expect("pushed");
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::Stale));
    h.ok(ENGINE, delete_branch(b"work"));
    assert_eq!(h.call(ENGINE, merge(1, more)), Err(Error::Closed), "deleting its head closed it");
    assert_eq!(h.call(ENGINE, merge(9, more)), Err(Error::Missing(What::Item)));
}

#[test]
fn a_merge_of_what_the_base_has_already_is_refused() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.push(MAINTAINER, MAIN, work).expect("pushed to the base directly");
    assert!(!h.pull(1).mergeable);
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::NothingToMerge));
}

#[test]
fn protection_wants_green_ci_and_approvals_of_the_exact_head_where_stale_ones_are_dismissed() {
    let mut setup = setup();
    setup.protection =
        Some(Protection { branch: copy_of(MAIN), contexts: names(&[b"ci"]), approvals: 1, dismiss_stale: true });
    let mut h = Harness::with(CALM, setup);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    assert_eq!(h.push(ENGINE, MAIN, work), Err(Error::Protected), "nothing is pushed to it");
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::Protected), "CI is pending");
    h.settle();
    assert_eq!(checks(&h.pull(1)).as_slice(), [Check::Passed]);
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::Protected), "no approval");
    assert_eq!(h.call(ENGINE, review(1, Verdict::Approve)), Err(Error::Forbidden), "not its author's");
    h.ok(PERSON, review(1, Verdict::Approve));
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::Protected), "a reader's approval does not count");
    h.ok(MAINTAINER, review(1, Verdict::Approve));
    let more = h.commit(work, &[(b"src", b"two")]);
    h.push(ENGINE, b"work", more).expect("pushed");
    h.settle();
    assert_eq!(h.call(ENGINE, merge(1, more)), Err(Error::Protected), "the approval was of another head");
    h.ok(MAINTAINER, review(1, Verdict::Approve));
    h.ok(MAINTAINER, review(1, Verdict::RequestChanges));
    h.ok(MAINTAINER, review(1, Verdict::Comment));
    assert_eq!(h.call(ENGINE, merge(1, more)), Err(Error::Protected), "the last verdict counts");
    h.ok(MAINTAINER, review(1, Verdict::Approve));
    assert!(matches_merged(&h.call(ENGINE, merge(1, more))));
}

#[test]
fn protection_counts_official_approvals_of_any_head_by_default() {
    let mut setup = setup();
    setup.protection =
        Some(Protection { branch: copy_of(MAIN), contexts: names(&[]), approvals: 2, dismiss_stale: false });
    let mut h = Harness::with(CALM, setup);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.ok(MAINTAINER, review(1, Verdict::Approve));
    h.ok(PERSON, review(1, Verdict::Approve));
    crate::grant(&mut h.domain, REPOSITORY, PERSON, Permission::Write);
    let more = h.commit(work, &[(b"src", b"two")]);
    h.push(ENGINE, b"work", more).expect("pushed");
    assert_eq!(h.call(ENGINE, merge(1, more)), Err(Error::Protected), "a reader's approval was not official");
    assert!(!h.pull(1).reviews[1].official);
    h.ok(ADMIN, review(1, Verdict::Approve));
    crate::grant(&mut h.domain, REPOSITORY, ADMIN, Permission::Read);
    assert!(matches_merged(&h.call(ENGINE, merge(1, more))), "an approval of the older head counts, as on Forgejo");
}

fn matches_merged(result: &Result<Answer, Error>) -> bool {
    let Ok(Answer::Merged(_)) = result else {
        return false;
    };
    true
}

#[test]
fn reviews_are_kept_in_order_and_refused_where_forgejo_refuses() {
    let mut h = Harness::new(CALM);
    h.issue(PERSON, b"question");
    assert_eq!(h.call(MAINTAINER, review(1, Verdict::Approve)), Err(Error::Missing(What::Pull)));
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.ok(ENGINE, review(2, Verdict::Comment));
    h.ok(PERSON, review(2, Verdict::RequestChanges));
    let reviews = h.pull(2).reviews;
    assert_eq!(reviews.len(), 2);
    assert_eq!((reviews[0].author, reviews[0].verdict, reviews[0].commit), (ENGINE, Verdict::Comment, work));
    assert_eq!((reviews[1].author, reviews[1].verdict), (PERSON, Verdict::RequestChanges));
    assert_eq!(h.call(STRANGER, review(2, Verdict::Comment)), Err(Error::Forbidden));
    h.ok(ENGINE, write(Write::Close { number: 2 }));
    assert_eq!(h.call(PERSON, review(2, Verdict::Approve)), Err(Error::Closed));
}

#[test]
fn ci_reports_pending_then_a_drawn_verdict_or_never() {
    let mut setup = setup();
    setup.checks.contexts = names(&[b"ci", b"lint"]);
    setup.checks.passes = 0;
    let mut h = Harness::with(CALM, setup);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    assert_eq!(checks(&h.pull(1)).as_slice(), [Check::Pending, Check::Pending], "pending at once");
    h.settle();
    let pull = h.pull(1);
    assert_eq!(checks(&pull).as_slice(), [Check::Failed, Check::Failed]);
    assert_eq!(pull.statuses[0].author, CI);
    assert_eq!(h.domain.tally().verdicts, 2);
    let mut setup = crate::tests::setup();
    setup.checks.silent = 1000;
    let mut h = Harness::with(CALM, setup);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.settle();
    assert_eq!(checks(&h.pull(1)).as_slice(), [Check::Pending], "never reported");
    assert_eq!(h.domain.tally().silent, 1);
}

#[test]
fn ci_runs_a_context_again_after_it_settled_once() {
    let mut setup = setup();
    setup.checks.contexts = names(&[b"ci", b"lint"]);
    setup.checks.passes = 0;
    setup.checks.reruns = 1000;
    let mut h = Harness::with(CALM, setup);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    // Each context reports, five seconds in, then is run again: pending at
    // once, and a verdict five seconds after.
    let at = h.env.now.saturating_add(Duration::from_secs(5));
    h.until(at);
    assert_eq!(checks(&h.pull(1)).as_slice(), [Check::Failed, Check::Failed], "settled");
    h.observations();
    h.until(at.saturating_add(Duration::from_secs(5)));
    assert_eq!(checks(&h.pull(1)).as_slice(), [Check::Pending, Check::Pending], "run again");
    let rerun = h.observations();
    assert_eq!(rerun.len(), 2, "each context reported pending again: {rerun:?}");
    h.settle();
    assert_eq!(checks(&h.pull(1)).as_slice(), [Check::Failed, Check::Failed], "a verdict again");
    assert_eq!(h.domain.tally().verdicts, 4, "each context ran twice, and no more");
}

#[test]
fn a_pending_review_is_hidden_until_it_is_submitted_and_keeps_its_place() {
    let mut setup = setup();
    setup.protection =
        Some(Protection { branch: copy_of(MAIN), contexts: Box::new([]), approvals: 1, dismiss_stale: false });
    let mut h = Harness::with(CALM, setup);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.observations();
    let started = write(Write::Review { number: 1, verdict: None, body: copy_of(b"thinking") });
    assert_eq!(h.ok(MAINTAINER, started), Answer::Reviewed(1), "started");
    assert!(h.pull(1).reviews.is_empty(), "no read shows it");
    assert!(h.observations().is_empty(), "nor is it observed");
    let updated = h.item(1).0.updated;
    h.wait(Duration::from_secs(5));
    assert_eq!(h.ok(PERSON, review(1, Verdict::Comment)), Answer::Reviewed(2), "another, meanwhile");
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::Protected), "a pending approval counts for nothing");
    let submit = |verdict| write(Write::Submit { number: 1, review: 1, verdict });
    assert_eq!(h.call(PERSON, submit(Verdict::Approve)), Err(Error::Forbidden), "only its author submits it");
    h.wait(Duration::from_secs(5));
    assert_eq!(h.ok(MAINTAINER, submit(Verdict::Approve)), Answer::Done);
    let reviews = h.pull(1).reviews;
    assert_eq!((reviews[0].id, reviews[0].verdict), (1, Verdict::Approve), "shown before the one after it");
    assert_eq!(reviews[1].id, 2);
    assert!(h.item(1).0.updated > updated, "the pull request updated as it was submitted");
    let observed = h.observations();
    let submitted = Observation::Reviewed {
        repository: repository(),
        number: 1,
        id: 1,
        commit: work,
        verdict: Verdict::Approve,
        body: copy_of(b"thinking"),
        by: MAINTAINER,
    };
    assert!(observed.as_slice().contains(&submitted), "observed as submitted: {observed:?}");
    assert_eq!(h.call(MAINTAINER, submit(Verdict::Approve)), Err(Error::Missing(What::Review)), "once");
    assert!(matches_merged(&h.call(ENGINE, merge(1, work))), "the approval counts once submitted");
}

#[test]
fn ci_follows_content_where_a_cue_is_configured() {
    let mut setup = setup();
    setup.checks.passes = 0;
    setup.checks.cue = Some(Cue { path: copy_of(b"ci"), green: copy_of(b"green") });
    let mut h = Harness::with(CALM, setup);
    let red = h.commit(FIRST, &[(b"ci", b"red")]);
    h.push(ENGINE, b"work", red).expect("pushed");
    h.open(b"work").expect("opened");
    h.settle();
    assert_eq!(checks(&h.pull(1)).as_slice(), [Check::Failed]);
    let repaired = h.commit(red, &[(b"ci", b"all green now")]);
    h.push(ENGINE, b"work", repaired).expect("pushed");
    h.settle();
    assert_eq!(checks(&h.pull(1)).as_slice(), [Check::Passed], "a repair turns it green");
}

#[test]
fn ci_runs_once_per_commit_and_a_status_updates_the_pull_requests_on_it() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.ok(ENGINE, create_branch(b"copy", work));
    h.settle();
    assert_eq!(h.domain.tally().verdicts, 1, "once for the commit");
    assert!(h.item(1).0.updated < h.pull(1).statuses[0].at, "a verdict leaves the pull request's updated time");
    assert_eq!(h.call(PERSON, status(work, b"review", Check::Passed)), Err(Error::Forbidden));
    h.ok(MAINTAINER, status(work, b"review", Check::Passed));
    let pull = h.pull(1);
    assert_eq!(checks(&pull).as_slice(), [Check::Passed, Check::Passed]);
    assert_eq!((&*pull.statuses[1].context, pull.statuses[1].author), (&b"review"[..], MAINTAINER));
    assert_eq!(h.call(MAINTAINER, status(99, b"ci", Check::Failed)), Err(Error::Missing(What::Commit)));
    assert_eq!(
        h.call(MAINTAINER, status(work, b"third", Check::Failed)),
        Err(Error::Full),
        "no more contexts than the limits"
    );
}

// Wikis.

#[test]
fn wiki_pages_are_put_listed_read_and_deleted() {
    let mut h = Harness::new(CALM);
    assert_eq!(h.ok(ENGINE, put(b"b", b"second")), Answer::Revision(1));
    assert_eq!(h.ok(MAINTAINER, put(b"a", b"first")), Answer::Revision(2));
    assert_eq!(h.ok(ENGINE, put(b"a", b"first, again")), Answer::Revision(3));
    let page = Page { name: copy_of(b"a"), content: copy_of(b"first, again"), author: ENGINE, revision: 3 };
    assert_eq!(h.ok(PERSON, read(Read::Page { name: copy_of(b"a") })), Answer::Page(page));
    h.ok(ENGINE, put(b"c", b"third"));
    assert_eq!(h.call(ENGINE, put(b"d", b"fourth")), Err(Error::Full));
    let pages = [PageName { name: copy_of(b"a"), revision: 3 }, PageName { name: copy_of(b"b"), revision: 1 }];
    let listed = Answer::Pages { pages: Box::new(pages), next: Some(copy_of(b"b")) };
    assert_eq!(h.ok(PERSON, read(Read::Pages { after: None })), listed);
    let listed = Answer::Pages { pages: Box::new([PageName { name: copy_of(b"c"), revision: 4 }]), next: None };
    assert_eq!(h.ok(PERSON, read(Read::Pages { after: Some(copy_of(b"b")) })), listed);
    assert_eq!(h.call(PERSON, put(b"e", b"")), Err(Error::Forbidden), "writing needs write");
    assert_eq!(h.call(ENGINE, put(b"a", &[b'x'; 40])), Err(Error::TooLarge));
    assert_eq!(h.ok(ENGINE, write(Write::DeletePage { name: copy_of(b"b") })), Answer::Done);
    assert_eq!(h.call(ENGINE, write(Write::DeletePage { name: copy_of(b"b") })), Err(Error::Missing(What::Page)));
    assert_eq!(h.ok(ENGINE, put(b"d", b"fourth")), Answer::Revision(6), "revisions only grow");
}

// Webhooks and faults.

#[test]
fn a_subscriber_hears_of_each_change_by_webhook() {
    let mut h = Harness::new(CALM);
    h.issue(PERSON, b"one");
    let id = h.comment(PERSON, 1, b"hi");
    h.ok(ENGINE, set_labels(1, &[b"bug"]));
    h.ok(PERSON, edit(id, b"hello"));
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.ok(MAINTAINER, review(2, Verdict::Approve));
    h.ok(ENGINE, put(b"home", b"notes"));
    h.settle();
    let heard = h.heard();
    let expected = [
        ((Change::Issue, Some(1)), 2),
        ((Change::Comment, Some(1)), 2),
        ((Change::Push, None), 1),
        ((Change::Status, None), 2),
        ((Change::Pull, Some(2)), 1),
        ((Change::Review, Some(2)), 1),
        ((Change::Wiki, None), 1),
    ];
    for (hook, times) in expected {
        let mut count: u32 = 0;
        for &other in &heard {
            if other == hook {
                count = count.checked_add(1).expect("few");
            }
        }
        assert_eq!(count, times, "each change heard once");
    }
    assert_eq!(heard.len(), 10);
    assert_eq!(h.domain.tally().hooks, 10);
    let mut setup = setup();
    setup.hooked = false;
    let mut h = Harness::with(CALM, setup);
    h.issue(PERSON, b"unheard");
    h.settle();
    assert!(h.heard().is_empty(), "no subscriber");
}

#[test]
fn webhooks_come_late_or_never() {
    let mut h = Harness::new(Config { hooks_late: 1000, ..CALM });
    h.issue(PERSON, b"one");
    let opened = h.env.now;
    h.settle();
    assert_eq!(h.heard().as_slice(), [(Change::Issue, Some(1))]);
    assert_eq!(h.env.now, opened.saturating_add(Duration::from_secs(29)), "late: thirty seconds from the change");
    assert_eq!(h.domain.tally().hooks_late, 1);
    let mut h = Harness::new(Config { hooks_lost: 1000, ..CALM });
    h.issue(PERSON, b"one");
    h.settle();
    assert!(h.heard().is_empty());
    assert_eq!(h.domain.tally().hooks_lost, 1);
    let mut h = Harness::new(Config { limits: Limits { hooks: 1, ..LIMITS }, ..CALM });
    h.issue(PERSON, b"one");
    h.comment(PERSON, 1, b"in flight");
    h.settle();
    assert_eq!(h.heard().as_slice(), [(Change::Issue, Some(1))]);
    assert_eq!(h.domain.tally().hooks_dropped, 1, "no room for the second");
}

#[test]
fn a_call_fails_before_it_is_made_or_times_out_after() {
    let mut h = Harness::new(Config { unavailable: 1000, ..CALM });
    assert_eq!(h.call(PERSON, create(b"lost", b"", &[])), Err(Error::Unavailable));
    assert_eq!(h.inspect(&Read::Item { number: 1, after: 0 }), Err(Error::Missing(What::Item)), "nothing was done");
    assert_eq!(h.domain.tally().unavailable, 1);
    let mut h = Harness::new(Config { timeouts: 1000, ..CALM });
    assert_eq!(h.call(PERSON, create(b"made", b"", &[])), Err(Error::Timeout));
    assert_eq!(&*h.item(1).0.title, b"made", "it was done all the same");
    assert_eq!(h.domain.tally().timeouts, 1);
}

#[test]
fn a_call_may_land_after_it_was_answered_as_timed_out() {
    let mut h = Harness::new(Config { landing: 1000, ..CALM });
    assert_eq!(h.call(PERSON, create(b"later", b"", &[])), Err(Error::Timeout));
    assert_eq!(h.inspect(&Read::Item { number: 1, after: 0 }), Err(Error::Missing(What::Item)), "not made yet");
    assert_eq!(h.domain.calls(), 1, "the call is held until it lands");
    h.observations();
    h.settle();
    assert_eq!(&*h.item(1).0.title, b"later", "made after its answer");
    assert_eq!(h.observations().len(), 1, "and observed then");
    assert_eq!(h.domain.tally().landed, 1);
    assert_eq!(h.domain.calls(), 0);
    let permission = h.call(PERSON, read(Read::Permission { user: PERSON }));
    assert_eq!(permission, Ok(Answer::Permission(Permission::Read)), "a read is never late to land");
}

#[test]
fn the_forges_clock_is_its_own() {
    let skew = Skew::Behind(Duration::from_secs(100));
    let mut h = Harness::new(Config { skew, rate_limit: 1, ..CALM });
    h.wait(Duration::from_secs(1_000));
    h.issue(PERSON, b"one");
    let (item, _) = h.item(1);
    assert_eq!(item.updated, Time::ZERO.saturating_add(Duration::from_secs(900)), "the forge's time, behind");
    assert_eq!(crate::time(&h.env.limits, h.env.now), Time::ZERO.saturating_add(Duration::from_secs(901)));
    let op = read(Read::Items {
        state: None,
        kind: None,
        labels: names(&[]),
        author: None,
        since: Time::ZERO,
        page: 1,
        limit: 0,
    });
    let reset = Time::ZERO.saturating_add(Duration::from_secs(960));
    assert_eq!(h.call(PERSON, op.clone()), Err(Error::RateLimited { reset }), "a reset in the forge's time");
    let Ok(Answer::Items { now, .. }) = h.call(ENGINE, op) else {
        unreachable!("a page");
    };
    assert_eq!(now, Time::ZERO.saturating_add(Duration::from_secs(902)), "a page says when it was made");
}

#[test]
fn a_late_answer_comes_after_the_late_latency() {
    let mut h = Harness::new(Config { late: 1000, ..CALM });
    h.issue(PERSON, b"slow");
    assert_eq!(h.env.now, Time::ZERO.saturating_add(Duration::from_secs(30)));
    assert_eq!(h.domain.tally().late, 1);
}

#[test]
fn a_rate_limit_refuses_with_when_it_resets() {
    let mut h = Harness::new(Config { rate_limit: 2, ..CALM });
    h.issue(PERSON, b"one");
    h.issue(PERSON, b"two");
    let reset = Time::ZERO.saturating_add(Duration::from_secs(60));
    assert_eq!(h.call(PERSON, create(b"three", b"", &[])), Err(Error::RateLimited { reset }));
    assert_eq!(h.issue(ENGINE, b"theirs"), 3, "a window per user");
    h.env.now = reset;
    assert_eq!(h.issue(PERSON, b"three"), 4, "a new window");
    assert_eq!(h.domain.tally().limited, 1);
}

#[test]
fn a_forge_full_of_calls_answers_at_once_as_unavailable() {
    let mut h = Harness::new(CALM);
    for token in 0..=u64::from(LIMITS.calls) {
        let op = read(Read::Permission { user: PERSON });
        let event = Event::Call {
            reply_to: ReplyTo::new(Token::new(token)),
            user: PERSON,
            repository: copy_of(REPOSITORY),
            op,
        };
        step(&mut h.domain, &h.env, event, &mut h.out);
    }
    let Some(Request::Reply { to, result }) = h.out.pop() else {
        unreachable!("an answer at once");
    };
    assert_eq!((to.into_token(), result), (Token::new(u64::from(LIMITS.calls)), Err(Error::Unavailable)));
    assert_eq!(h.domain.calls(), LIMITS.calls);
    h.settle();
    assert_eq!(h.domain.calls(), 0, "every call answered and reclaimed");
    assert_eq!(h.domain.tally().busy, 1);
    assert_eq!(h.domain.tally().answered, LIMITS.calls + 1);
}

#[test]
fn a_seed_replays_to_the_same_run() {
    let first = run(11);
    assert_eq!(first, run(11));
    assert_ne!(first, run(12), "another seed, another run");
}

type Run = (Box<[(Time, Result<Answer, Error>)]>, Box<[Observation]>, Box<[(Change, Option<u64>)]>);

/// What a run of calls under every fault answers and when, what it observes,
/// and the webhooks it sends, in order.
fn run(seed: u64) -> Run {
    let config = Config {
        latency_min: Duration::from_millis(100),
        latency_max: Duration::from_secs(2),
        late: 200,
        unavailable: 150,
        timeouts: 150,
        rate_limit: 12,
        hook_min: Duration::from_millis(100),
        hook_max: Duration::from_secs(3),
        hooks_late: 200,
        hooks_lost: 200,
        ..CALM
    };
    let mut setup = setup();
    setup.checks.passes = 500;
    setup.checks.silent = 200;
    setup.checks.latency_min = Duration::from_secs(1);
    setup.checks.latency_max = Duration::from_secs(9);
    let mut h = Harness::seeded(config, setup, seed);
    let mut answers = List::with_capacity(32);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    for op in [
        create(b"one", b"body", &[]),
        write(Write::Comment { number: 1, body: copy_of(b"hi") }),
        Op::Git(Git::Push { branch: copy_of(b"work"), commit: work }),
        write(Write::OpenPull {
            title: copy_of(b"t"),
            body: copy_of(b""),
            head: copy_of(b"work"),
            base: copy_of(MAIN),
        }),
        read(Read::Items {
            state: None,
            kind: None,
            labels: names(&[]),
            author: None,
            since: Time::ZERO,
            page: 1,
            limit: 0,
        }),
        put(b"home", b"notes"),
        read(Read::Pull { number: 2 }),
    ] {
        for user in [ENGINE, MAINTAINER] {
            let answer = h.call(user, op.clone());
            answers.push((h.env.now, answer)).expect("room");
        }
    }
    h.settle();
    (answers.into_boxed(), h.observations().into_boxed(), h.heard().into_boxed())
}

// Observations.

#[test]
fn observations_say_what_happened_content_and_all() {
    let mut h = Harness::new(CALM);
    h.issue(PERSON, b"crash");
    let id = h.comment(ENGINE, 1, b"looking");
    h.ok(ENGINE, edit(id, b"fixing"));
    h.ok(ENGINE, set_labels(1, &[b"bug"]));
    h.ok(ENGINE, write(Write::Close { number: 1 }));
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.settle();
    let expected = [
        Observation::Opened {
            repository: repository(),
            number: 1,
            kind: Kind::Issue,
            title: copy_of(b"crash"),
            body: copy_of(b"body"),
            labels: names(&[]),
            branches: None,
            by: PERSON,
        },
        Observation::Commented { repository: repository(), number: 1, id, body: copy_of(b"looking"), by: ENGINE },
        Observation::Edited { repository: repository(), number: 1, id, body: copy_of(b"fixing"), by: ENGINE },
        Observation::Labelled { repository: repository(), number: 1, labels: names(&[b"bug"]), by: ENGINE },
        Observation::Closed { repository: repository(), number: 1, by: ENGINE },
        Observation::Moved { repository: repository(), branch: copy_of(b"work"), from: None, to: work, by: ENGINE },
        Observation::Reported {
            repository: repository(),
            commit: work,
            context: copy_of(b"ci"),
            state: Check::Pending,
            by: CI,
        },
        Observation::Reported {
            repository: repository(),
            commit: work,
            context: copy_of(b"ci"),
            state: Check::Passed,
            by: CI,
        },
    ];
    assert_eq!(h.observations().as_slice(), expected);
    assert_eq!(h.domain.observations_lost(), 0);
}

#[test]
fn observations_beyond_the_queue_are_dropped_and_counted() {
    let mut h = Harness::new(Config { limits: Limits { observations: 2, ..LIMITS }, ..CALM });
    for title in [b"one", b"two", b"six"] {
        h.issue(PERSON, title);
    }
    assert_eq!(h.observations().len(), 2);
    assert_eq!(h.domain.observations_lost(), 1);
    h.issue(PERSON, b"more");
    assert_eq!(h.observations().len(), 1, "room again once drained");
}

#[test]
fn a_pull_request_is_found_by_its_branches_and_statuses_by_their_commit() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    let find = read(Read::PullFor { head: copy_of(b"work"), base: copy_of(MAIN) });
    assert_eq!(h.call(PERSON, find.clone()), Err(Error::Missing(What::Pull)));
    h.open(b"work").expect("opened");
    h.ok(ENGINE, write(Write::Close { number: 1 }));
    h.open(b"work").expect("opened again");
    let Answer::Pull(pull) = h.ok(PERSON, find.clone()) else {
        unreachable!("a pull request");
    };
    assert_eq!((pull.number, pull.state), (2, State::Open), "the newest");
    h.ok(ENGINE, write(Write::Close { number: 2 }));
    let Answer::Pull(pull) = h.ok(PERSON, find) else {
        unreachable!("a pull request");
    };
    assert_eq!((pull.number, pull.state), (2, State::Closed), "open or not");
    h.settle();
    let Answer::Statuses(statuses) = h.ok(PERSON, read(Read::Statuses { commit: work })) else {
        unreachable!("statuses");
    };
    assert_eq!(statuses.len(), 1);
    assert_eq!((&*statuses[0].context, statuses[0].state), (&b"ci"[..], Check::Passed));
    assert_eq!(h.ok(PERSON, read(Read::Statuses { commit: FIRST })), Answer::Statuses(Box::new([])), "never a head");
    assert_eq!(h.call(PERSON, read(Read::Statuses { commit: 99 })), Err(Error::Missing(What::Commit)));
}

#[test]
fn observations_and_webhooks_follow_branches_pull_requests_and_the_wiki() {
    let mut setup = setup();
    setup.checks.silent = 1000;
    let mut h = Harness::with(CALM, setup);
    h.ok(ENGINE, define(b"later"));
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    let more = h.commit(work, &[(b"src", b"two")]);
    h.push(ENGINE, b"work", more).expect("pushed");
    h.ok(MAINTAINER, review(1, Verdict::Approve));
    let Answer::Merged(merged) = h.ok(ENGINE, merge(1, more)) else {
        unreachable!("merged");
    };
    h.ok(ENGINE, delete_branch(b"work"));
    h.ok(ENGINE, put(b"home", b"notes"));
    h.ok(ENGINE, write(Write::DeletePage { name: copy_of(b"home") }));
    let expected = [
        Observation::Defined { repository: repository(), label: copy_of(b"later"), by: ENGINE },
        moved(b"work", None, work),
        pending(work),
        Observation::Opened {
            repository: repository(),
            number: 1,
            kind: Kind::Pull,
            title: copy_of(b"change"),
            body: copy_of(b"body"),
            labels: names(&[]),
            branches: Some(Branches { head: copy_of(b"work"), base: copy_of(MAIN), commit: work }),
            by: ENGINE,
        },
        moved(b"work", Some(work), more),
        pending(more),
        Observation::Reviewed {
            repository: repository(),
            number: 1,
            id: 1,
            commit: more,
            verdict: Verdict::Approve,
            body: copy_of(b"looked"),
            by: MAINTAINER,
        },
        Observation::Merged {
            repository: repository(),
            number: 1,
            base: copy_of(MAIN),
            head: more,
            commit: merged,
            by: ENGINE,
        },
        moved(MAIN, Some(FIRST), merged),
        pending(merged),
        Observation::Deleted { repository: repository(), branch: copy_of(b"work"), at: more, by: ENGINE },
        Observation::Wiki {
            repository: repository(),
            name: copy_of(b"home"),
            content: Some(copy_of(b"notes")),
            revision: 1,
            by: ENGINE,
        },
        Observation::Wiki { repository: repository(), name: copy_of(b"home"), content: None, revision: 2, by: ENGINE },
    ];
    assert_eq!(h.observations().as_slice(), expected);
    let number = h.issue(PERSON, b"talk");
    let id = h.comment(PERSON, number, b"oops");
    h.ok(PERSON, write(Write::DeleteComment { id }));
    h.ok(PERSON, write(Write::Close { number }));
    h.ok(PERSON, write(Write::Reopen { number }));
    let observed = h.observations();
    let tail = observed.as_slice().get(2..).expect("the comment and its state");
    let expected = [
        Observation::Removed { repository: repository(), number, id, by: PERSON },
        Observation::Closed { repository: repository(), number, by: PERSON },
        Observation::Reopened { repository: repository(), number, by: PERSON },
    ];
    assert_eq!(tail, expected);
    h.settle();
    let heard = h.heard();
    for (hook, times) in [
        ((Change::Push, None), 4),
        ((Change::Status, None), 3),
        ((Change::Pull, Some(1)), 3),
        ((Change::Review, Some(1)), 1),
        ((Change::Wiki, None), 2),
        ((Change::Issue, Some(number)), 3),
        ((Change::Comment, Some(number)), 2),
    ] {
        let mut count: u32 = 0;
        for &other in &heard {
            if other == hook {
                count = count.checked_add(1).expect("few");
            }
        }
        assert_eq!(count, times, "each change heard once, a pull request's new head too");
    }
}

#[test]
fn webhooks_name_the_branch_and_commit_a_push_or_status_is_about() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.settle();
    h.ok(ENGINE, delete_branch(b"work"));
    h.settle();
    let heard = h.heard_all();
    let expected = [
        (Change::Push, None, Some(copy_of(b"work")), Some(work)),
        (Change::Status, None, None, Some(work)),
        (Change::Status, None, None, Some(work)),
        (Change::Push, None, Some(copy_of(b"work")), None),
    ];
    assert_eq!(heard.as_slice(), expected);
}

#[test]
fn refused_writes_and_rejected_pushes_are_observed() {
    let mut setup = setup();
    setup.protection =
        Some(Protection { branch: copy_of(MAIN), contexts: names(&[b"ci"]), approvals: 0, dismiss_stale: false });
    setup.checks.silent = 1000;
    let mut h = Harness::with(CALM, setup);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    let aside = h.commit(FIRST, &[(b"src", b"else")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    h.observations();
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::Protected));
    assert_eq!(h.push(ENGINE, b"work", aside), Ok(Answer::Pushed(Pushed::Rejected)));
    assert_eq!(h.call(PERSON, set_labels(1, &[b"bug"])), Err(Error::Forbidden));
    h.call(PERSON, read(Read::Item { number: 9, after: 0 })).expect_err("missing");
    h.call_on(b"ai/nowhere", ENGINE, delete_branch(b"work")).expect_err("no such repository");
    let expected = [
        Observation::Refused {
            repository: repository(),
            what: Operation::Merge,
            number: Some(1),
            commit: Some(work),
            error: Error::Protected,
            by: ENGINE,
        },
        Observation::Rejected { repository: repository(), branch: copy_of(b"work"), commit: aside, by: ENGINE },
        Observation::Refused {
            repository: repository(),
            what: Operation::SetLabels,
            number: Some(1),
            commit: None,
            error: Error::Forbidden,
            by: PERSON,
        },
    ];
    assert_eq!(h.observations().as_slice(), expected, "reads and unknown repositories aside");
}

// Edits, dependencies, reviewers, and the reads of one comment and the labels.

/// Opens a pull request of `head` into `main`.
fn open_op(head: &[u8]) -> Op {
    write(Write::OpenPull {
        title: copy_of(b"change"),
        body: copy_of(b"body"),
        head: copy_of(head),
        base: copy_of(MAIN),
    })
}

fn retitle(number: u64, title: &[u8]) -> Op {
    write(Write::EditItem { number, title: Some(copy_of(title)), body: None })
}

fn depend(number: u64, dependencies: &[u64]) -> Op {
    write(Write::SetDependencies { number, dependencies: Box::from(dependencies) })
}

fn request(number: u64, reviewers: &[u64]) -> Op {
    write(Write::SetReviewers { number, reviewers: Box::from(reviewers) })
}

#[test]
fn an_items_title_and_body_are_edited_by_its_author_or_a_writer() {
    let mut h = Harness::new(CALM);
    let number = h.issue(PERSON, b"tpyo");
    let theirs = h.issue(ENGINE, b"theirs");
    h.wait(Duration::from_secs(10));
    let edited = h.env.now;
    assert_eq!(h.ok(PERSON, retitle(number, b"typo")), Answer::Done);
    let (item, _) = h.item(number);
    assert_eq!((&*item.title, &*item.body), (&b"typo"[..], &b"body"[..]));
    assert_eq!(item.updated, edited, "an edit of the item updates it");
    let rebody = write(Write::EditItem { number, title: None, body: Some(copy_of(b"rewritten")) });
    assert_eq!(h.ok(MAINTAINER, rebody), Answer::Done, "a writer edits anyone's");
    assert_eq!(&*h.item(number).0.body, b"rewritten");
    assert_eq!(h.call(PERSON, retitle(number, b"")), Err(Error::Empty));
    assert_eq!(h.call(PERSON, retitle(theirs, b"mine")), Err(Error::Forbidden));
    let observed = h.observations();
    let revised = Observation::Revised {
        repository: repository(),
        number,
        title: copy_of(b"typo"),
        body: copy_of(b"rewritten"),
        by: MAINTAINER,
    };
    assert!(observed.as_slice().contains(&revised), "observed with what it now says");
}

#[test]
fn dependencies_are_set_as_a_whole_within_a_repository_without_cycles() {
    let mut h = Harness::new(CALM);
    for title in [b"one", b"two", b"six", b"ten"] {
        h.issue(ENGINE, title);
    }
    assert_eq!(h.ok(ENGINE, depend(1, &[3, 2, 3])), Answer::Done);
    assert_eq!(h.ok(PERSON, read(Read::Dependencies { number: 1 })), Answer::Dependencies(Box::new([2, 3])));
    assert_eq!(h.call(ENGINE, depend(2, &[1])), Err(Error::Circular), "two that depend on each other");
    assert_eq!(h.call(ENGINE, depend(4, &[4])), Err(Error::Circular), "on itself");
    assert_eq!(h.call(ENGINE, depend(4, &[9])), Err(Error::Missing(What::Item)));
    assert_eq!(h.call(ENGINE, depend(4, &[1, 2, 3, 1])), Err(Error::TooLarge));
    assert_eq!(h.call(PERSON, depend(4, &[1])), Err(Error::Forbidden), "needs write");
    assert_eq!(h.ok(ENGINE, depend(1, &[4])), Answer::Done);
    assert_eq!(h.ok(PERSON, read(Read::Dependencies { number: 1 })), Answer::Dependencies(Box::new([4])), "a set");
    assert_eq!(h.call(PERSON, read(Read::Dependencies { number: 9 })), Err(Error::Missing(What::Item)));
}

#[test]
fn reviewers_are_requested_as_a_set_until_they_review() {
    let mut h = Harness::new(CALM);
    h.issue(PERSON, b"question");
    assert_eq!(h.call(ENGINE, request(1, &[MAINTAINER])), Err(Error::Missing(What::Pull)));
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    assert_eq!(h.ok(ENGINE, request(2, &[MAINTAINER, PERSON])), Answer::Done);
    assert_eq!(&*h.pull(2).reviewers, [PERSON, MAINTAINER]);
    assert_eq!(h.call(ENGINE, request(2, &[ENGINE])), Err(Error::Forbidden), "not its author");
    assert_eq!(h.call(ENGINE, request(2, &[STRANGER])), Err(Error::Forbidden), "not who may not read");
    assert_eq!(h.call(PERSON, request(2, &[MAINTAINER])), Err(Error::Forbidden), "asking needs write");
    h.ok(MAINTAINER, review(2, Verdict::Comment));
    assert_eq!(&*h.pull(2).reviewers, [PERSON], "a review answers the request");
    h.ok(ENGINE, write(Write::Close { number: 2 }));
    assert_eq!(h.call(ENGINE, request(2, &[MAINTAINER])), Err(Error::Closed));
}

#[test]
fn one_comment_and_the_defined_labels_are_read() {
    let mut h = Harness::new(CALM);
    let number = h.issue(ENGINE, b"talk");
    let id = h.comment(PERSON, number, b"hello");
    let Answer::Comment { number: on, comment } = h.ok(ENGINE, read(Read::Comment { id })) else {
        unreachable!("a comment");
    };
    assert_eq!((on, comment.id, comment.author, &*comment.body), (number, id, PERSON, &b"hello"[..]));
    assert_eq!(h.call(ENGINE, read(Read::Comment { id: 99 })), Err(Error::Missing(What::Comment)));
    assert_eq!(h.ok(PERSON, read(Read::Labels)), Answer::Labels(names(&[b"bug", b"temper"])));
}

#[test]
fn a_repository_has_what_its_branches_reach_and_what_was_pushed() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    let more = h.commit(work, &[(b"src", b"two")]);
    let stray = h.commit(FIRST, &[(b"src", b"stray")]);
    h.push(ENGINE, b"work", more).expect("pushed");
    let has = h.domain.has(REPOSITORY);
    assert!(has.contains(&FIRST) && has.contains(&work) && has.contains(&more));
    assert!(!has.contains(&stray), "a commit no one pushed");
}

// Bounds: rate windows, a full store, and one answer per call under every fault.

#[test]
fn more_callers_than_rate_windows_wait_for_the_first_to_end() {
    let mut h = Harness::new(Config { rate_limit: 5, ..CALM });
    let started = h.env.now;
    for user in 10..10 + u64::from(LIMITS.users) {
        assert_eq!(h.call(user, read(Read::Labels)), Err(Error::Forbidden), "taken, then refused for permission");
    }
    let reset = started.saturating_add(Duration::from_secs(60));
    assert_eq!(h.call(99, read(Read::Labels)), Err(Error::RateLimited { reset }), "every window is running");
    assert_eq!(h.domain.tally().crowded, 1);
    assert_eq!(h.domain.tally().limited, 0, "counted apart");
    h.env.now = reset;
    assert_eq!(h.call(99, read(Read::Labels)), Err(Error::Forbidden), "an ended window is reused");
    assert_eq!(h.ok(PERSON, read(Read::Labels)), Answer::Labels(names(&[b"bug", b"temper"])));
}

#[test]
fn a_full_store_refuses_counted_and_says_it_has_no_room() {
    let mut h = Harness::new(Config { limits: Limits { commits: 3, ..LIMITS }, ..CALM });
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"other", b"x", MAINTAINER).expect("the last room");
    assert_eq!(h.domain.room().commits, 0);
    assert_eq!(crate::commit(&mut h.domain, &h.env.limits, FIRST, files(&[])), Err(Error::Full));
    h.push(ENGINE, b"work", work).expect("pushed");
    h.open(b"work").expect("opened");
    assert_eq!(h.call(ENGINE, merge(1, work)), Err(Error::Full));
    assert_eq!(h.domain.tally().full, 1);
}

#[test]
fn statuses_of_commits_no_head_shows_are_forgotten_to_make_room() {
    let mut h = Harness::new(Config { limits: Limits { statuses: 3, ..LIMITS }, ..CALM });
    let one = h.commit(FIRST, &[(b"src", b"one")]);
    let two = h.commit(one, &[(b"src", b"two")]);
    let other = h.commit(FIRST, &[(b"lib", b"one")]);
    h.push(ENGINE, b"work", one).expect("pushed");
    h.push(ENGINE, b"other", other).expect("pushed");
    h.ok(MAINTAINER, status(FIRST, b"review", Check::Passed));
    assert_eq!(h.domain.room().statuses, 0);
    h.push(ENGINE, b"work", two).expect("pushed before the first's CI reported");
    assert_eq!(h.domain.tally().forgotten, 1, "the first is no head, and its checks are cancelled");
    h.settle();
    assert_eq!(h.domain.tally().verdicts, 2, "the other's and the second's");
    assert_eq!(h.ok(PERSON, read(Read::Statuses { commit: one })), Answer::Statuses(Box::new([])));
    let more = h.commit(two, &[(b"src", b"three")]);
    h.ok(ENGINE, create_branch(b"keep", two));
    h.push(ENGINE, b"work", more).expect("pushed");
    assert_eq!(h.domain.tally().unreported, 1, "every commit with statuses is a head");
    let busy = status(more, b"review", Check::Passed);
    assert_eq!(h.call(MAINTAINER, busy), Err(Error::Full));
}

#[test]
fn every_call_is_answered_once_under_every_fault() {
    let mut faults = [0_u32; 5];
    for seed in 0..32_u64 {
        let config = Config {
            latency_min: Duration::from_millis(10),
            latency_max: Duration::from_secs(2),
            late: 100,
            unavailable: 100,
            timeouts: 100,
            rate_limit: 6,
            rate_window: Duration::from_secs(5),
            hooks_late: 100,
            hooks_lost: 100,
            ..CALM
        };
        let mut h = Harness::seeded(config, setup(), seed);
        let mut rng = temper_lib::Rng::new(seed);
        let work = h.commit(FIRST, &[(b"src", b"one")]);
        let mut answered: temper_lib::Set<u64> = temper_lib::Set::with_capacity(256);
        let mut sent: u64 = 0;
        for _ in 0..64_u32 {
            for _ in 0..rng.between(0, 3) {
                sent = sent.checked_add(1).expect("few");
                let user = [ENGINE, PERSON, MAINTAINER][usize::try_from(rng.below(3)).expect("small")];
                let op = match rng.below(6) {
                    0 => create(b"issue", b"body", &[]),
                    1 => write(Write::Comment { number: 1, body: copy_of(b"hi") }),
                    2 => Op::Git(Git::Push { branch: copy_of(b"work"), commit: work }),
                    3 => open_op(b"work"),
                    4 => merge(1, work),
                    _ => read(Read::Items {
                        state: None,
                        kind: None,
                        labels: names(&[]),
                        author: None,
                        since: Time::ZERO,
                        page: 1,
                        limit: 0,
                    }),
                };
                let event =
                    Event::Call { reply_to: ReplyTo::new(Token::new(sent)), user, repository: repository(), op };
                step(&mut h.domain, &h.env, event, &mut h.out);
                h.drain(&mut answered);
            }
            if let Some(at) = h.domain.next_deadline() {
                h.env.now = h.env.now.max(at);
                fire(&mut h.domain, &h.env, &mut h.out);
                h.drain(&mut answered);
            }
            h.domain.reclaim();
        }
        for _ in 0..4096_u32 {
            let Some(at) = h.domain.next_deadline() else {
                break;
            };
            h.env.now = h.env.now.max(at);
            fire(&mut h.domain, &h.env, &mut h.out);
            h.drain(&mut answered);
        }
        h.domain.reclaim();
        assert_eq!(u64::from(answered.len()), sent, "seed {seed}: every call answered");
        assert_eq!(h.domain.calls(), 0);
        assert_eq!(u64::from(h.domain.tally().answered), sent);
        let tally = h.domain.tally();
        for (fault, count) in
            faults.iter_mut().zip([tally.busy, tally.unavailable, tally.timeouts, tally.limited, tally.late])
        {
            *fault = fault.saturating_add(count);
        }
    }
    assert!(!faults.contains(&0), "busy, unavailable, timed out, rate-limited and late calls all came: {faults:?}");
}

#[test]
fn modify_and_delete_or_two_adds_conflict_unless_they_agree() {
    let mut h = Harness::new(CALM);
    // The head deletes the readme, which the base changed.
    let deleted = crate::commit(&mut h.domain, &h.env.limits, FIRST, files(&[(b"ci", b"green")]))
        .expect("room")
        .expect("a change");
    h.push(ENGINE, b"delete", deleted).expect("pushed");
    h.open(b"delete").expect("opened");
    // Another head adds what the base adds too, differently, and a third
    // the same.
    let added = h.commit(FIRST, &[(b"new", b"ours")]);
    h.push(ENGINE, b"add", added).expect("pushed");
    h.open(b"add").expect("opened");
    let same = h.commit(FIRST, &[(b"new", b"theirs")]);
    h.push(ENGINE, b"same", same).expect("pushed");
    h.open(b"same").expect("opened");
    let changed = h.commit(FIRST, &[(b"README", b"changed"), (b"new", b"theirs")]);
    h.push(MAINTAINER, MAIN, changed).expect("the base moves");
    assert_eq!(h.call(ENGINE, merge(1, deleted)), Err(Error::Conflict), "modify and delete");
    assert_eq!(h.call(ENGINE, merge(2, added)), Err(Error::Conflict), "two adds");
    assert!(matches_merged(&h.call(ENGINE, merge(3, same))), "two adds that agree");
    // The other way: the base deletes what a head changed.
    let mut h = Harness::new(CALM);
    let edited = h.commit(FIRST, &[(b"README", b"edited")]);
    h.push(ENGINE, b"edit", edited).expect("pushed");
    h.open(b"edit").expect("opened");
    let gone = crate::commit(&mut h.domain, &h.env.limits, FIRST, files(&[(b"ci", b"green")]))
        .expect("room")
        .expect("a change");
    h.push(MAINTAINER, MAIN, gone).expect("the base moves");
    assert_eq!(h.call(ENGINE, merge(1, edited)), Err(Error::Conflict), "delete and modify");
}
