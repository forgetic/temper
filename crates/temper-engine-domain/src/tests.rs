//! The top level's step tests: its routing and translations, through its
//! boundary, with the forge, workers and people scripted inline.

#![expect(
    clippy::arithmetic_side_effects,
    reason = "the tests count small numbers, and an overflow traps in a test as anywhere"
)]

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token, Wall};
use temper_engine_domain_brief::{self as brief, Budgets};
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_forge::{self as forge, api};
use temper_engine_domain_notes as notes;
use temper_engine_domain_plan::{self as plan, Budget};
use temper_engine_domain_rules::{self as rules, Acts, Permission, Rules};
use temper_engine_domain_views::{self as views, Capture, Policy};
use temper_engine_domain_work::{self as work, Retries, Retry};

use crate::boundary::{Ask, Event, Hello, Item, Refusal, Reply, Request};
use crate::config::Config;
use crate::domain::{Domain, fire, max_out, step};
use crate::limits::{Limits, accepts, worst_case};
use crate::translate;

const RETRY: Retry = Retry { retries: 2, base: Duration::from_secs(1), max: Duration::from_secs(8) };

const BUDGET: Budget = Budget { tokens: 1_000, turns: 20, time: Duration::from_secs(600) };

const LIMITS: Limits = Limits {
    work: work::Limits {
        items: 12,
        retries: Retries { transient: RETRY, permanent: RETRY, run: RETRY, agent: RETRY, lost: RETRY, invalid: RETRY },
        undelivered: 2,
        facts: 64,
    },
    plan: plan::Limits {
        steps: 8,
        name_bytes: 16,
        dependencies: 4,
        gates: 2,
        targets: 2,
        instruction_bytes: 64,
        tasks: 3,
        events: 8,
        repairs: 2,
        rebases: 4,
        rejections: 2,
        stall: Duration::from_secs(3600),
        budget: BUDGET,
    },
    rules: rules::Limits { repositories: 2, protected: 2, branch_bytes: 16, grants: 4, reviews: 4, gates: 4, lands: 4 },
    forge: forge::Limits {
        repositories: 2,
        items: 12,
        labels: 3,
        members: 3,
        inbox: 4,
        reviewers: 4,
        reads: 16,
        writes: 16,
        calls: 16,
        page: 4,
        name_bytes: 32,
        title_bytes: 64,
        body_bytes: 256,
        rate: 1000,
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
    },
    fleet: fleet::Limits {
        workers: 2,
        slots: 2,
        workstreams: 2,
        workstream_bytes: 16,
        attempts: 16,
        calls: 4,
        grace: Duration::from_secs(10),
        facts: 64,
    },
    brief: brief::Limits {
        briefs: 4,
        sections: 10,
        items: 4,
        parts: 4,
        read_bytes: 256,
        budgets: Budgets {
            item: 64,
            comments: 64,
            dependencies: 64,
            ci: 64,
            reviews: 64,
            pull: 64,
            attempts: 64,
            plan: 64,
            notes: 64,
            template: 64,
        },
        brief_bytes: 640,
        gather: Duration::from_secs(10),
        facts: 64,
    },
    notes: notes::Limits {
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
    },
    views: views::Limits {
        runs: 4,
        watchers: 4,
        backlog: 4,
        report_bytes: 64,
        records: 4,
        batch_bytes: 256,
        appends: 2,
        flush: Duration::from_secs(1),
        retention: Duration::from_secs(600),
        sweep: Duration::from_secs(60),
        snapshot_bytes: 256,
        facts: 64,
    },
    asks: 4,
    text_bytes: 256,
    steps: 16,
    facts: 64,
};

/// The engine's forge user, and people.
const ENGINE: u64 = 99;
const ALICE: u64 = 1;
const BOB: u64 = 2;

const ITEM: Item = Item { repository: 0, number: 7 };

/// A deployment of two repositories, the first protecting `main`.
fn config() -> Config {
    let mut protected = List::with_capacity(LIMITS.rules.protected);
    protected.push(rules::Branch { repository: 0, name: copy_of(b"main") }).unwrap();
    let repo = plan::Repo { bases: Box::new([copy_of(b"main")]) };
    let charter = plan::Charter {
        instructions: copy_of(b"talk"),
        template: None,
        grants: plan::Grants { modify: false, shell: false, forge: true, subagents: false, note: true },
        budget: BUDGET,
    };
    let wake = plan::Wake {
        on: plan::Sources { own: true, related: true, subscribed: false, messages: true },
        every: None,
        batch: plan::Batch { count: 1, age: None },
    };
    Config {
        plan: plan::Config { repositories: Box::new([repo.clone(), repo]), templates: Box::new([]) },
        home: 0,
        forge: forge::Config {
            engine: ENGINE,
            tracking: copy_of(b"temper"),
            hand_in: copy_of(b"temper:hand-in"),
            projected: Box::new([]),
        },
        rules: Rules {
            repositories: 2,
            protected,
            engine: ENGINE,
            reviewer: Permission::Write,
            plan_steps: 8,
            plan_spend: 1000,
            plan_acceptance: Permission::Write,
            repository_notes: None,
            deployment_notes: Some(Permission::Admin),
            run_spend: 1000,
            goal_spend: 10_000,
            deployment_spend: 100_000,
            acts: Acts {
                open: Permission::Write,
                steer: Permission::Write,
                accept: Permission::Write,
                cancel: Permission::Write,
                release: Permission::Write,
                watch: Permission::Read,
            },
        },
        session: plan::SessionSpec { charter, resume: plan::Resume::Default, wake },
        models: copy_of(b"model"),
        policy: Policy {
            text: Capture::Content,
            progress: Capture::Shape,
            calls: Capture::Shape,
            tools: Capture::Shape,
            usage: Capture::Shape,
        },
        branches: copy_of(b"temper/"),
        saved: copy_of(b"temper-saved/"),
    }
}

const fn env(secs: u64) -> Env<Limits> {
    Env { now: Time::from_nanos(secs.saturating_mul(1_000_000_000)), wall: Wall::EPOCH, limits: LIMITS }
}

fn domain() -> Domain {
    assert!(worst_case(&LIMITS).is_some(), "the limits are bounded");
    Domain::new(config(), &LIMITS, 1, Time::ZERO)
}

/// What one event leads to.
fn stepped(domain: &mut Domain, env: &Env<Limits>, event: Event) -> List<Request> {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    step(domain, env, event, &mut out);
    drained(&mut out)
}

/// What the loop does between events at `env.now`: fire what is due, and go
/// on with the ready lists, until neither has anything left.
fn idle(domain: &mut Domain, env: &Env<Limits>) -> List<Request> {
    let mut requests = List::with_capacity(256);
    for _ in 0_u32..64 {
        let mut out = Queue::with_capacity(max_out(&LIMITS));
        if domain.is_ready() {
            crate::domain::resume(domain, env, &mut out);
        } else if domain.is_due(env.now) {
            fire(domain, env, &mut out);
        } else {
            return requests;
        }
        while let Some(request) = out.pop() {
            requests.push(request).unwrap();
        }
    }
    panic!("the loop settles");
}

fn drained(out: &mut Queue<Request>) -> List<Request> {
    let mut requests = List::with_capacity(out.len());
    while let Some(request) = out.pop() {
        requests.push(request).unwrap();
    }
    requests
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the limits are bounded");
    assert!(bound > 0, "the domain holds something");
    let more = worst_case(&Limits { steps: 32, ..LIMITS }).expect("more steps are bounded");
    assert!(more > bound, "more steps hold more");
    assert_eq!(worst_case(&Limits { steps: 1, ..LIMITS }), None, "a step bound leaves room for hand-offs");
    let fewer = forge::Limits { items: 2, ..LIMITS.forge };
    assert_eq!(worst_case(&Limits { forge: fewer, ..LIMITS }), None, "the hub and the forge hold one working set");
    let narrow = fleet::Limits { workstream_bytes: 8, ..LIMITS.fleet };
    assert_eq!(worst_case(&Limits { fleet: narrow, ..LIMITS }), None, "a workstream key fits the fleet");
    assert!(accepts(&config(), &LIMITS), "the configuration fits");
    let elsewhere = Config { home: 2, ..config() };
    assert!(!accepts(&elsewhere, &LIMITS), "the home is one of the deployment's repositories");
    let mut wordy = config();
    wordy.session.charter.instructions = Box::from([b'x'; 65].as_slice());
    assert!(!accepts(&wordy, &LIMITS), "a session's step is one the plan could have written");
}

#[test]
fn a_run_token_packs_its_item() {
    let run = translate::run(Item { repository: 3, number: 41 }).unwrap();
    assert_eq!(translate::item(run), Item { repository: 3, number: 41 });
    assert_eq!(translate::run(Item { repository: 1 << 16, number: 1 }), None);
    assert_eq!(translate::run(Item { repository: 0, number: 1 << 48 }), None);
    let workstream = translate::workstream(Item { repository: 1, number: 2 });
    assert_eq!(&workstream[..], &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2][..]);
}

#[test]
fn numbers_and_branches_are_written_in_decimal() {
    assert_eq!(&translate::decimal(0)[..], b"0");
    assert_eq!(&translate::decimal(1207)[..], b"1207");
    assert_eq!(&translate::decimal(u64::MAX)[..], b"18446744073709551615");
    assert_eq!(&translate::branch(b"temper/", ITEM)[..], b"temper/7");
}

#[test]
fn hold_reasons_keep_their_codes() {
    let codes = [
        translate::hold(plan::Hold::Rejected),
        translate::hold(plan::Hold::Repairs),
        translate::hold(plan::Hold::Rebases),
        translate::hold(plan::Hold::PullClosed),
        translate::hold(plan::Hold::Escalated),
        translate::hold(plan::Hold::Stalled),
    ];
    assert_eq!(codes, [1, 2, 3, 4, 5, 6], "a stored code means the same after a restart");
    let own = [translate::NO_STEP, translate::RUN_ACCEPTANCE, translate::RUN_REFUSED];
    assert_eq!(own, [0, 7, 8], "the top level's own codes mean the same after a restart");
    for code in own {
        assert!(!codes.contains(&code), "no plan reason is the top level's own: {code}");
    }
}

#[test]
fn a_cold_start_lists_live_work() {
    let mut domain = domain();
    let requests = idle(&mut domain, &env(1));
    let mut listed = false;
    for request in &requests {
        if let Request::Forge { op: api::Op::Items { .. }, payload: None, .. } = request {
            listed = true;
        }
    }
    assert!(listed, "the forge child domain lists live work: {requests:?}");
}

#[test]
fn a_person_asking_of_an_item_not_held_is_refused() {
    let mut domain = domain();
    let ask = Ask::Stop { item: ITEM };
    let requests =
        stepped(&mut domain, &env(1), Event::Ask { reply_to: ReplyTo::new(Token::new(5)), person: ALICE, ask });
    let [Request::Reply { to, reply }] = requests.as_slice() else { panic!("one reply: {requests:?}") };
    assert_eq!(*reply, Reply::Refused(Refusal::Unknown));
    assert_eq!(to, &ReplyTo::new(Token::new(5)), "the reply answers the call");
}

#[test]
fn a_workers_hello_reaches_the_fleet() {
    let mut domain = domain();
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    let requests = stepped(&mut domain, &env(1), Event::Hello { channel: Token::new(1), hello });
    assert!(requests.is_empty(), "a hello is not answered: {requests:?}");
    assert_eq!(domain.fleet().workers(), 1, "the worker is in contact");
}

/// A forge scripted inline: open issues and their comments, which answers
/// every call at once, remembering the engine's records and outcomes and
/// decoding them back as the protocol layer would.
struct Forge {
    issues: List<Issue>,
    pulls: List<Change>,
    /// The wikis' pages: their repository, name, revision and note.
    pages: List<Wiki>,
    /// Where each branch is, as workers push.
    branches: List<(Box<[u8]>, [u8; 32])>,
    /// The forge's clock, which the world moves.
    now: Time,
    /// The next comment's id, and the next item's number.
    comments: u64,
    numbers: u64,
    /// An item whose record writes time out, without landing, so many times
    /// more.
    stuck: Option<(Item, u32)>,
    /// The people who may only read; the rest may write.
    readers: List<u64>,
}

#[derive(Debug)]
struct Issue {
    item: Item,
    labels: Box<[Box<[u8]>]>,
    open: bool,
    updated: Time,
    comments: List<Note>,
}

/// A wiki page: its repository, name, revision and the note it holds.
type Wiki = (u32, Box<[u8]>, u64, Option<notes::Page>);

/// A pull request: its item, branches, head, CI on it, and its reviews.
#[derive(Debug)]
struct Change {
    item: Item,
    head: Box<[u8]>,
    base: Box<[u8]>,
    commit: [u8; 32],
    ci: forge::Ci,
    merged: Option<[u8; 32]>,
    reviews: List<api::Review>,
    /// Its base moved under it, unread: a merge is refused for a conflict.
    conflicts: bool,
}

#[derive(Debug)]
struct Note {
    id: u64,
    author: u64,
    revision: u64,
    mark: api::Mark,
    body: Box<[u8]>,
    decoded: Option<crate::boundary::Decoded>,
}

impl Forge {
    fn new() -> Forge {
        Forge {
            issues: List::with_capacity(16),
            pulls: List::with_capacity(8),
            pages: List::with_capacity(8),
            branches: List::with_capacity(8),
            now: Time::ZERO,
            comments: 100,
            numbers: 1,
            stuck: None,
            readers: List::with_capacity(4),
        }
    }

    /// Whether a record write on `item` times out, without landing.
    fn times_out(&mut self, item: Item, body: &api::Body) -> bool {
        let record = match body {
            api::Body::Record { .. } => true,
            api::Body::Text(_) | api::Body::Payload(_) => false,
        };
        match self.stuck {
            Some((stuck, left)) if record && stuck == item && left > 0 => {
                self.stuck = Some((stuck, left - 1));
                true
            }
            Some(_) | None => false,
        }
    }

    fn issue(&mut self, item: Item) -> Option<&mut Issue> {
        let mut index = None;
        for (at, issue) in self.issues.iter().enumerate() {
            if issue.item == item {
                index = Some(u32::try_from(at).unwrap());
            }
        }
        self.issues.get_mut(index?)
    }

    fn change(&mut self, item: Item) -> Option<&mut Change> {
        let mut index = None;
        for (at, change) in self.pulls.iter().enumerate() {
            if change.item == item {
                index = Some(u32::try_from(at).unwrap());
            }
        }
        self.pulls.get_mut(index?)
    }

    fn branch(&self, name: &[u8]) -> Option<[u8; 32]> {
        for (branch, commit) in &self.branches {
            if **branch == *name {
                return Some(*commit);
            }
        }
        None
    }

    fn pull(&mut self, item: Item) -> Result<api::Answer, api::Error> {
        let open = self.issue(item).ok_or(api::Error::Missing)?.open;
        let change = self.change(item).ok_or(api::Error::Missing)?;
        let state = if open { api::State::Open } else { api::State::Closed };
        Ok(api::Answer::Pull(api::Pull {
            number: item.number,
            state,
            head: change.head.clone(),
            base: change.base.clone(),
            commit: change.commit,
            base_commit: Some([9; 32]),
            merged: change.merged,
            mergeable: true,
            ci: change.ci,
        }))
    }

    fn open(&mut self, repository: u32, labels: Box<[Box<[u8]>]>) -> Item {
        let item = Item { repository, number: self.numbers };
        self.numbers += 1;
        let updated = self.now;
        self.issues.push(Issue { item, labels, open: true, updated, comments: List::with_capacity(128) }).unwrap();
        item
    }

    fn post(
        &mut self,
        item: Item,
        author: u64,
        mark: api::Mark,
        body: Box<[u8]>,
        payload: Option<crate::boundary::Payload>,
    ) -> u64 {
        self.comments += 1;
        let id = self.comments;
        let decoded = decoded(id, payload);
        let now = self.now;
        let issue = self.issue(item).expect("a comment is on an issue the forge has");
        issue.updated = now;
        issue.comments.push(Note { id, author, revision: 1, mark, body, decoded }).unwrap();
        id
    }

    /// A page of a listing: the open or closed issues carrying `label`,
    /// updated since `since`, all on the first page.
    fn listing(
        &self,
        repository: u32,
        state: Option<api::State>,
        label: Option<&[u8]>,
        since: Time,
        page: u32,
    ) -> api::Answer {
        let mut items = List::with_capacity(16);
        for issue in &self.issues {
            let open = match state {
                Some(api::State::Open) => issue.open,
                Some(api::State::Closed) => !issue.open,
                None => true,
            };
            let labelled = match &label {
                Some(label) => carries(&issue.labels, label),
                None => true,
            };
            if issue.item.repository != repository || !open || !labelled || issue.updated < since || page > 1 {
                continue;
            }
            items.push(summary(issue)).unwrap();
        }
        api::Answer::Items { items: items.into_boxed(), more: false, now: self.now }
    }

    /// An issue and its comments above `after`, the engine's payloads among
    /// them decoded into `decoded`.
    fn read(
        &mut self,
        item: Item,
        after: u64,
        decoded: &mut List<crate::boundary::Decoded>,
    ) -> Result<api::Answer, api::Error> {
        let Some(issue) = self.issue(item) else { return Err(api::Error::Missing) };
        let mut comments = List::with_capacity(128);
        for note in &issue.comments {
            if note.id <= after {
                continue;
            }
            if let Some(found) = &note.decoded {
                decoded.push(found.clone()).unwrap();
            }
            let comment = api::Comment {
                id: note.id,
                author: note.author,
                created: Time::ZERO,
                revision: note.revision,
                mark: copy_mark(&note.mark),
                body: note.body.clone(),
            };
            comments.push(comment).unwrap();
        }
        Ok(api::Answer::Item { item: summary(issue), comments: comments.into_boxed(), more: false })
    }

    /// The answer to `op` on `repository`, and what is decoded inside it.
    #[expect(clippy::too_many_lines, reason = "the scripted forge answers every operation in one match")]
    fn answer(
        &mut self,
        repository: u32,
        op: api::Op,
        payload: Option<crate::boundary::Payload>,
    ) -> (Result<api::Answer, api::Error>, List<crate::boundary::Decoded>) {
        let mut decoded = List::with_capacity(128);
        let answer = match op {
            api::Op::Items { state, label, since, page, .. } => {
                Ok(self.listing(repository, state, label.as_deref(), since, page))
            }
            api::Op::Item { number, after } => self.read(Item { repository, number }, after, &mut decoded),
            api::Op::Permission { user, .. } => {
                let reads = self.readers.as_slice().contains(&user);
                Ok(api::Answer::Permission(if reads { api::Permission::Read } else { api::Permission::Write }))
            }
            api::Op::CreateIssue { labels, .. } => {
                let item = self.open(repository, labels);
                Ok(api::Answer::Created(item.number))
            }
            api::Op::Post { number, key, person, body } => {
                let item = Item { repository, number };
                if self.times_out(item, &body) {
                    return (Err(api::Error::Timeout), decoded);
                }
                let (mark, text) = match body {
                    api::Body::Record { position, nonce, .. } => {
                        (api::Mark::Record { position, nonce }, Box::default())
                    }
                    api::Body::Text(text) => match key {
                        Some(key) => (api::Mark::Key { key, person }, text),
                        None => (api::Mark::None, text),
                    },
                    api::Body::Payload(_) => match key {
                        Some(key) => (api::Mark::Key { key, person }, Box::default()),
                        None => (api::Mark::None, Box::default()),
                    },
                };
                let id = self.post(item, ENGINE, mark, text, payload);
                Ok(api::Answer::Commented { id, revision: 1 })
            }
            api::Op::EditComment { number, id, body } => {
                let item = Item { repository, number };
                if self.times_out(item, &body) {
                    return (Err(api::Error::Timeout), decoded);
                }
                let issue = self.issue(item).expect("an edit is on an issue the forge has");
                let mut revision = 0_u64;
                for at in 0..issue.comments.len() {
                    let note = issue.comments.get_mut(at).unwrap();
                    if note.id == id {
                        note.revision += 1;
                        revision = note.revision;
                        if let api::Body::Record { position, nonce, .. } = body {
                            note.mark = api::Mark::Record { position, nonce };
                        }
                        note.decoded = decoded_of(id, payload.clone());
                    }
                }
                Ok(api::Answer::Edited { revision })
            }
            api::Op::Close { number } => {
                let now = self.now;
                if let Some(issue) = self.issue(Item { repository, number }) {
                    issue.open = false;
                    issue.updated = now;
                }
                Ok(api::Answer::Done)
            }
            api::Op::Comment { number, id } => {
                let Some(issue) = self.issue(Item { repository, number }) else {
                    return (Err(api::Error::Missing), decoded);
                };
                let mut found = Err(api::Error::Missing);
                for note in &issue.comments {
                    if note.id != id {
                        continue;
                    }
                    if let Some(note) = &note.decoded {
                        decoded.push(note.clone()).unwrap();
                    }
                    found = Ok(api::Answer::Comment(api::Comment {
                        id,
                        author: note.author,
                        created: Time::ZERO,
                        revision: note.revision,
                        mark: copy_mark(&note.mark),
                        body: Box::new([]),
                    }));
                }
                found
            }
            api::Op::Pull { number } => self.pull(Item { repository, number }),
            api::Op::Reviews { number, page } => {
                let reviews = match self.change(Item { repository, number }) {
                    Some(change) if page == 1 => copy_reviews(&change.reviews),
                    Some(_) | None => Box::new([]),
                };
                Ok(api::Answer::Reviews { reviews, more: false })
            }
            api::Op::Statuses { commit, .. } => {
                let mut ci = forge::Ci::None;
                for change in &self.pulls {
                    if change.commit == commit {
                        ci = change.ci;
                    }
                }
                Ok(api::Answer::Statuses { ci, statuses: Box::new([]), more: false })
            }
            api::Op::Branch { branch } => match self.branch(&branch) {
                Some(commit) => Ok(api::Answer::Commit(commit)),
                None => Err(api::Error::Missing),
            },
            api::Op::OpenPull { head, base, .. } => {
                let Some(commit) = self.branch(&head) else { return (Err(api::Error::Missing), decoded) };
                let item = self.open(repository, Box::new([]));
                let change = Change {
                    item,
                    head,
                    base,
                    commit,
                    ci: forge::Ci::Passed,
                    merged: None,
                    reviews: List::with_capacity(4),
                    conflicts: false,
                };
                self.pulls.push(change).unwrap();
                Ok(api::Answer::Created(item.number))
            }
            api::Op::Merge { number, head } => {
                let item = Item { repository, number };
                if let Some(change) = self.change(item)
                    && change.conflicts
                {
                    return (Err(api::Error::Conflict), decoded);
                }
                let merged = match self.change(item) {
                    Some(change) if change.merged.is_none() && change.commit == head => {
                        change.merged = Some([8; 32]);
                        true
                    }
                    Some(_) | None => false,
                };
                if !merged {
                    return (Err(api::Error::Stale), decoded);
                }
                let now = self.now;
                let issue = self.issue(item).unwrap();
                issue.open = false;
                issue.updated = now;
                Ok(api::Answer::Merged([8; 32]))
            }
            api::Op::PullFor { head, base } => {
                let mut found = None;
                for change in &self.pulls {
                    if change.head == head && change.base == base {
                        found = Some(change.item);
                    }
                }
                match found {
                    Some(item) => self.pull(item),
                    None => Err(api::Error::Missing),
                }
            }
            api::Op::Pages { .. } => {
                let mut names = List::with_capacity(8);
                for (at, name, revision, _) in &self.pages {
                    if *at == repository {
                        names.push(api::PageName { name: name.clone(), revision: *revision }).unwrap();
                    }
                }
                Ok(api::Answer::Pages { pages: names.into_boxed(), next: None })
            }
            api::Op::Page { name } => {
                let mut found = Err(api::Error::Missing);
                for (at, page_name, revision, note) in &self.pages {
                    if *at != repository || **page_name != *name || note.is_none() {
                        continue;
                    }
                    if let Some(note) = note {
                        let page = Box::new(note.clone());
                        decoded.push(crate::boundary::Decoded::Page { name: name.clone(), page }).unwrap();
                    }
                    let page =
                        api::Page { name: name.clone(), content: Box::new([]), revision: *revision, nonce: None };
                    found = Ok(api::Answer::Page(page));
                }
                found
            }
            api::Op::PutPage { name, .. } => {
                let note = match payload {
                    Some(crate::boundary::Payload::Page(page)) => Some(*page),
                    Some(_) | None => None,
                };
                let mut revision = 1;
                for at in 0..self.pages.len() {
                    let page = self.pages.get_mut(at).unwrap();
                    if page.0 == repository && page.1 == name {
                        page.2 += 1;
                        page.3 = note.clone();
                        revision = page.2;
                    }
                }
                if revision == 1 {
                    self.pages.push((repository, name, 1, note)).unwrap();
                }
                Ok(api::Answer::Revision(revision))
            }
            api::Op::Remarks { .. } => Err(api::Error::Missing),
            api::Op::AddLabels { .. }
            | api::Op::RemoveLabels { .. }
            | api::Op::Review { .. }
            | api::Op::SetReviewers { .. }
            | api::Op::SetDependencies { .. }
            | api::Op::Reopen { .. }
            | api::Op::DeleteBranch { .. }
            | api::Op::DeletePage { .. } => Ok(api::Answer::Done),
        };
        (answer, decoded)
    }
}

fn carries(labels: &[Box<[u8]>], label: &[u8]) -> bool {
    for carried in labels {
        if **carried == *label {
            return true;
        }
    }
    false
}

fn summary(issue: &Issue) -> api::Summary {
    api::Summary {
        number: issue.item.number,
        kind: api::Kind::Issue,
        state: if issue.open { api::State::Open } else { api::State::Closed },
        author: ALICE,
        key: None,
        labels: issue.labels.clone(),
        title: copy_of(b"title"),
        body: copy_of(b"body"),
        updated: issue.updated,
    }
}

fn copy_reviews(reviews: &List<api::Review>) -> Box<[api::Review]> {
    let mut copies = List::with_capacity(reviews.len());
    for review in reviews {
        let copy = api::Review {
            id: review.id,
            author: review.author,
            verdict: review.verdict,
            commit: review.commit,
            key: review.key.clone(),
            body: review.body.clone(),
        };
        copies.push(copy).unwrap();
    }
    copies.into_boxed()
}

fn copy_mark(mark: &api::Mark) -> api::Mark {
    match mark {
        api::Mark::None => api::Mark::None,
        api::Mark::Key { key, person } => api::Mark::Key { key: key.clone(), person: *person },
        api::Mark::Record { position, nonce } => api::Mark::Record { position: *position, nonce: *nonce },
        api::Mark::Mangled => api::Mark::Mangled,
    }
}

fn decoded(id: u64, payload: Option<crate::boundary::Payload>) -> Option<crate::boundary::Decoded> {
    decoded_of(id, payload)
}

/// What the protocol layer would decode from the comment `id` the payload
/// was written into.
fn decoded_of(id: u64, payload: Option<crate::boundary::Payload>) -> Option<crate::boundary::Decoded> {
    match payload? {
        crate::boundary::Payload::Record(record) => Some(crate::boundary::Decoded::Record { comment: id, record }),
        crate::boundary::Payload::Outcome(posted) => Some(crate::boundary::Decoded::Outcome { comment: id, posted }),
        crate::boundary::Payload::Page(_) => None,
    }
}

/// The engine, the scripted forge, and what reached workers, people and
/// the store, at a time that moves by seconds.
struct World {
    domain: Domain,
    forge: Forge,
    secs: u64,
    /// Requests for workers, people and the store, in order.
    seen: List<Request>,
    pending: Queue<Event>,
    /// How many more forge calls are answered, if the forge stops
    /// answering: those past it are lost.
    calls: Option<u32>,
    /// The deployment's configuration, at every start.
    config: fn() -> Config,
}

impl World {
    fn new() -> World {
        World::configured(config)
    }

    /// A world whose deployment is configured as `config` says.
    fn configured(config: fn() -> Config) -> World {
        World {
            domain: Domain::new(config(), &LIMITS, 1, Time::ZERO),
            forge: Forge::new(),
            secs: 1,
            seen: List::with_capacity(1024),
            pending: Queue::with_capacity(1024),
            calls: None,
            config,
        }
    }

    fn env(&self) -> Env<Limits> {
        env(self.secs)
    }

    /// Hands `event` in, then settles.
    fn deliver(&mut self, event: Event) {
        self.pending.push(event);
        self.settle();
    }

    /// Routes everything pending, answering the forge's calls and the
    /// store's operations at once, until nothing more happens now.
    fn settle(&mut self) {
        for _ in 0_u32..4096 {
            let env = self.env();
            let mut out = Queue::with_capacity(max_out(&LIMITS));
            if let Some(event) = self.pending.pop() {
                step(&mut self.domain, &env, event, &mut out);
            } else if self.domain.is_ready() {
                crate::domain::resume(&mut self.domain, &env, &mut out);
            } else if self.domain.is_due(env.now) {
                fire(&mut self.domain, &env, &mut out);
            } else {
                self.domain.reclaim();
                return;
            }
            self.domain.reclaim();
            while let Some(request) = out.pop() {
                self.answer(request);
            }
        }
        panic!("the world settles");
    }

    fn answer(&mut self, request: Request) {
        match request {
            Request::Forge { call, repository, op, payload } => {
                match self.calls {
                    Some(0) => return,
                    Some(left) => self.calls = Some(left - 1),
                    None => {}
                }
                self.forge.now = self.env().now;
                let (result, decoded) = self.forge.answer(repository, op, payload);
                self.pending.push(Event::Answered { call, result, decoded: decoded.into_boxed() });
            }
            Request::Store { owner, op } => {
                let stored = match op {
                    crate::boundary::Store::Get { .. } => crate::boundary::Stored::Got(None),
                    crate::boundary::Store::Put { .. }
                    | crate::boundary::Store::Drop { .. }
                    | crate::boundary::Store::Append { .. }
                    | crate::boundary::Store::Expire { .. } => crate::boundary::Stored::Done,
                };
                self.pending.push(Event::Stored { owner, stored });
            }
            seen @ (Request::Assign { .. }
            | Request::Inbound { .. }
            | Request::Cancel { .. }
            | Request::Relayed { .. }
            | Request::Acknowledge { .. }
            | Request::Refuse { .. }
            | Request::Reply { .. }
            | Request::Deliver { .. }
            | Request::Ended { .. }) => self.seen.push(seen).unwrap(),
        }
    }

    /// The engine restarts: everything in memory is lost, and it starts
    /// cold on the forge as it is.
    fn restart(&mut self) {
        self.domain = Domain::new((self.config)(), &LIMITS, 1, Time::ZERO);
        self.calls = None;
        self.pending = Queue::with_capacity(1024);
        self.seen = List::with_capacity(1024);
        self.settle();
        assert!(self.domain.is_loaded(), "the cold start is done");
    }

    /// A session opened by Alice, on the first item, its first turn
    /// assigned to the worker on channel 1.
    fn session() -> (World, Item) {
        let mut world = World::started();
        let ask = Ask::Open { repository: 0, key: copy_of(b"k1"), title: copy_of(b"hi"), message: copy_of(b"hello") };
        world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(9)), person: ALICE, ask });
        let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
        world.deliver(Event::Hello { channel: Token::new(1), hello });
        let item = Item { repository: 0, number: 1 };
        assert_eq!(assignment(&world.seen).expect("the first turn is assigned").item, item);
        (world, item)
    }

    /// The cold start, on an empty forge.
    fn started() -> World {
        let mut world = World::new();
        world.settle();
        assert!(world.domain.is_loaded(), "the cold start is done");
        world
    }
}

#[test]
fn a_cold_start_on_an_empty_forge_tells_the_fleet() {
    let mut world = World::started();
    let mut loaded = false;
    while let Some(fact) = world.domain.pop_fact() {
        if fact == crate::facts::Fact::Loaded {
            loaded = true;
        }
    }
    assert!(loaded, "the cold start's end is told");
    assert!(world.seen.is_empty(), "nothing reaches workers or people: {:?}", world.seen.as_slice());
}

#[test]
fn a_session_opened_from_the_web_runs_on_a_worker_and_its_reply_is_applied() {
    let mut world = World::started();
    let ask = Ask::Open { repository: 0, key: copy_of(b"k1"), title: copy_of(b"hi"), message: copy_of(b"hello") };
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(9)), person: ALICE, ask });
    let item = Item { repository: 0, number: 1 };
    let opened = replied(&world.seen, Reply::Opened { item });
    assert!(opened, "the person hears the session is open: {:?}", world.seen.as_slice());
    assert_eq!(world.domain.work().items(), 1, "the hub holds the session");
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    world.deliver(Event::Hello { channel: Token::new(1), hello });
    let assigned = assignment(&world.seen).expect("the session's first turn is assigned");
    assert_eq!(assigned.item, item);
    assert_eq!(assigned.attempt, 1);
    assert!(!assigned.charter.brief.is_empty(), "the charter carries the brief");
    let outcome = crate::boundary::Outcome::Reply { text: copy_of(b"hello to you") };
    let work = crate::boundary::Work { landed: Box::new([]) };
    let answer = crate::boundary::Answer::Ended { outcome, work };
    world.deliver(Event::Answer { channel: Token::new(1), item, attempt: 1, answer });
    let mut acknowledged = false;
    for request in &world.seen {
        if let Request::Acknowledge { item: of, attempt: 1, .. } = request {
            acknowledged = *of == item;
        }
    }
    assert!(acknowledged, "the answer is acknowledged once durable: {:?}", world.seen.as_slice());
    let issue = world.forge.issue(item).unwrap();
    let mut outcomes = 0_u32;
    for note in &issue.comments {
        if let Some(crate::boundary::Decoded::Outcome { .. }) = note.decoded {
            outcomes += 1;
        }
    }
    assert_eq!(outcomes, 1, "the outcome is posted once");
}

/// The assignment the workers were sent last.
fn assignment(seen: &List<Request>) -> Option<crate::boundary::Assignment> {
    let mut found = None;
    for request in seen {
        if let Request::Assign { assignment, .. } = request {
            found = Some(assignment.clone());
        }
    }
    found
}

/// Whether a person was answered `wanted`.
fn replied(seen: &List<Request>, wanted: Reply) -> bool {
    for request in seen {
        if let Request::Reply { reply, .. } = request
            && *reply == wanted
        {
            return true;
        }
    }
    false
}

/// The answer of a session's turn that replies.
fn replied_answer() -> crate::boundary::Answer {
    let outcome = crate::boundary::Outcome::Reply { text: copy_of(b"hello to you") };
    crate::boundary::Answer::Ended { outcome, work: crate::boundary::Work { landed: Box::new([]) } }
}

fn acknowledged(seen: &List<Request>, item: Item, attempt: u64) -> bool {
    for request in seen {
        if let Request::Acknowledge { item: of, attempt: at, .. } = request
            && *of == item
            && *at == attempt
        {
            return true;
        }
    }
    false
}

#[test]
fn a_restart_adopts_the_claim_a_worker_still_hosts() {
    let (mut world, item) = World::session();
    world.restart();
    assert_eq!(world.domain.work().items(), 1, "the session is taken in again");
    assert!(assignment(&world.seen).is_none(), "the claim is adopted, not assigned again");
    let hosted = crate::boundary::Hosted { item, attempt: 1, phase: fleet::Phase::Active };
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([hosted]) };
    world.deliver(Event::Hello { channel: Token::new(2), hello });
    world.deliver(Event::Answer { channel: Token::new(2), item, attempt: 1, answer: replied_answer() });
    assert!(acknowledged(&world.seen, item, 1), "the adopted run's answer is applied: {:?}", world.seen.as_slice());
}

/// The phase the item's record on the forge says.
fn phase(world: &mut World, item: Item) -> Option<work::Phase> {
    let issue = world.forge.issue(item)?;
    let mut phase = None;
    for note in &issue.comments {
        if let Some(crate::boundary::Decoded::Record { record, .. }) = &note.decoded {
            phase = Some(record.lifecycle.phase);
        }
    }
    phase
}

#[test]
fn a_restart_while_applying_applies_the_posted_outcome_again() {
    let (mut world, item) = World::session();
    // The forge answers the outcome's post and the record's move to
    // applying, then nothing more.
    world.calls = Some(3);
    world.deliver(Event::Answer { channel: Token::new(1), item, attempt: 1, answer: replied_answer() });
    let applying = matches_applying(phase(&mut world, item));
    assert!(applying, "the record says the outcome is being applied: {:?}", phase(&mut world, item));
    world.restart();
    assert_eq!(phase(&mut world, item), Some(work::Phase::Waiting), "the application is resumed and committed");
    assert_eq!(world.domain.work().items(), 1, "the session is taken in again");
    let posted = {
        let issue = world.forge.issue(item).unwrap();
        let mut outcomes = 0_u32;
        for note in &issue.comments {
            if let Some(crate::boundary::Decoded::Outcome { .. }) = note.decoded {
                outcomes += 1;
            }
        }
        outcomes
    };
    assert_eq!(posted, 1, "a restart posts nothing twice");
}

fn matches_applying(phase: Option<work::Phase>) -> bool {
    match phase {
        Some(work::Phase::Applying { .. }) => true,
        Some(
            work::Phase::Waiting
            | work::Phase::Parked
            | work::Phase::Retrying(_)
            | work::Phase::Claimed
            | work::Phase::Held { .. }
            | work::Phase::Done,
        )
        | None => false,
    }
}

fn got(read: brief::Read) -> Box<[brief::Part]> {
    match read {
        brief::Read::Got(parts) => parts,
        brief::Read::Failed => panic!("a cut keeps what fits"),
    }
}

fn part(bytes: &[u8], left: u64) -> brief::Part {
    brief::Part { bytes: copy_of(bytes), left }
}

#[test]
fn a_read_is_cut_as_its_fit_says() {
    use crate::waits::Bounds;
    let found: [&[u8]; 3] = [b"abc", b"defg", b"hi"];
    let run = Bounds { keep: brief::Keep::Start, fit: brief::Fit::Run, parts: 4, bytes: 5 };
    let cut = got(crate::serve::cut(&found, run));
    assert_eq!(&cut[..], &[part(b"abc", 0), part(b"de", 4)][..], "one run from the start, the rest told");
    let end = Bounds { keep: brief::Keep::End, ..run };
    let cut = got(crate::serve::cut(&found, end));
    assert_eq!(&cut[..], &[part(b"efg", 4), part(b"hi", 0)][..], "one run from the end");
    let each = Bounds { keep: brief::Keep::Start, fit: brief::Fit::Each, parts: 2, bytes: 4 };
    let cut = got(crate::serve::cut(&found, each));
    assert_eq!(&cut[..], &[part(b"ab", 1), part(b"de", 4)][..], "each part its share, the farthest left out");
    let lines = Bounds { keep: brief::Keep::Start, fit: brief::Fit::Lines, parts: 3, bytes: 6 };
    let cut = got(crate::serve::cut(&found, lines));
    assert_eq!(&cut[..], &[part(b"abc", 0), part(b"hi", 4)][..], "whole lines, and the last always");
    let utf8 = Bounds { keep: brief::Keep::Start, fit: brief::Fit::Run, parts: 1, bytes: 2 };
    let cut = got(crate::serve::cut(&["é!".as_bytes()][..], Bounds { bytes: 1, ..utf8 }));
    assert_eq!(&cut[..], &[part(b"", 3)][..], "a UTF-8 sequence is never split");
}

impl World {
    /// Moves time on by `secs`, and settles.
    fn wait(&mut self, secs: u64) {
        self.secs += secs;
        self.settle();
    }

    /// The requests seen since `from`.
    fn since(&self, from: u32) -> &[Request] {
        self.seen.as_slice().get(usize::try_from(from).unwrap()..).unwrap_or(&[])
    }
}

#[test]
fn an_issue_handed_in_by_its_label_becomes_a_session() {
    let mut world = World::new();
    let labels: Box<[Box<[u8]>]> = Box::new([copy_of(b"temper:hand-in")]);
    let item = world.forge.open(0, labels);
    world.settle();
    assert!(world.domain.is_loaded(), "the cold start is done");
    assert_eq!(world.domain.work().items(), 1, "the issue handed in is taken in");
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    world.deliver(Event::Hello { channel: Token::new(1), hello });
    let assigned = assignment(&world.seen).expect("the session's first turn is assigned");
    assert_eq!(assigned.item, item);
    assert_eq!(assigned.charter.finish, plan::Finish::Turn { supervising: false }, "it runs a session's turn");
}

#[test]
fn a_person_stops_a_run_and_its_worker_is_told_to_cancel() {
    let (mut world, item) = World::session();
    let from = world.seen.len();
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(11)), person: ALICE, ask: Ask::Stop { item } });
    let mut cancelled = false;
    for request in world.since(from) {
        if let Request::Cancel { item: of, attempt: 1, .. } = request {
            cancelled = *of == item;
        }
    }
    assert!(cancelled, "the worker is told to cancel: {:?}", world.since(from));
    assert!(replied(&world.seen, Reply::Done), "the person hears the run is stopped");
    let failure = crate::boundary::Answer::Failed {
        failure: crate::boundary::Failure::Run,
        work: crate::boundary::Work { landed: Box::new([]) },
    };
    world.deliver(Event::Answer { channel: Token::new(1), item, attempt: 1, answer: failure });
    assert!(acknowledged(&world.seen, item, 1), "the stopped run's answer is taken");
    let held = match phase(&mut world, item) {
        Some(work::Phase::Held { why, .. }) => why == work::Hold::Stopped,
        Some(_) | None => false,
    };
    assert!(held, "the item is held for a person: {:?}", phase(&mut world, item));
}

#[test]
fn a_persons_message_reaches_the_live_run() {
    let (mut world, item) = World::session();
    let ask = Ask::Message { item, key: copy_of(b"m1"), message: copy_of(b"and another thing") };
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(12)), person: ALICE, ask });
    assert!(replied(&world.seen, Reply::Done), "the message is written: {:?}", world.seen.as_slice());
    let from = world.seen.len();
    world.deliver(Event::Hint { repository: 0, item: Some(item.number), commit: None, branch: None });
    world.wait(60);
    let mut relayed = false;
    for request in world.since(from) {
        if let Request::Inbound { item: of, attempt: 1, event: crate::boundary::Inbound::News(_), .. } = request {
            relayed = *of == item;
        }
    }
    assert!(relayed, "the message is relayed to the run: {:?}", world.since(from));
}

#[test]
fn a_runs_calls_are_served_once_each() {
    let (mut world, item) = World::session();
    let from = world.seen.len();
    let read = crate::boundary::Call::Read(forge::Read::Item { item: translate::forge_item(item), after: 0 });
    world.deliver(Event::Relay { channel: Token::new(1), item, attempt: 1, call: Token::new(1), body: read });
    let comment = crate::boundary::Call::Comment { text: copy_of(b"working on it") };
    world.deliver(Event::Relay { channel: Token::new(1), item, attempt: 1, call: Token::new(2), body: comment });
    let mut read_served = 0_u32;
    let mut posted = 0_u32;
    for request in world.since(from) {
        if let Request::Relayed { call, served, .. } = request {
            match served {
                crate::boundary::Served::Read(_) if *call == Token::new(1) => read_served += 1,
                crate::boundary::Served::Posted { .. } if *call == Token::new(2) => posted += 1,
                other @ (crate::boundary::Served::Read(_)
                | crate::boundary::Served::Posted { .. }
                | crate::boundary::Served::Recalled { .. }
                | crate::boundary::Served::Noted(_)
                | crate::boundary::Served::Unserved(_)) => panic!("an unexpected answer: {other:?}"),
            }
        }
    }
    assert_eq!((read_served, posted), (1, 1), "each call is answered once: {:?}", world.since(from));
}

#[test]
fn a_person_watches_an_item() {
    let (mut world, item) = World::session();
    let ask = Ask::Watch { subject: crate::boundary::Watched::Item { item } };
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(13)), person: ALICE, ask });
    let mut watching = false;
    for request in &world.seen {
        if let Request::Reply { reply: Reply::Watching { .. }, .. } = request {
            watching = true;
        }
    }
    assert!(watching, "the watch is taken: {:?}", world.seen.as_slice());
    let mut snapshot = None;
    for request in &world.seen {
        if let Request::Deliver { chunks, .. } = request
            && let Some(crate::boundary::Chunk::Snapshot { content, .. }) = chunks.first()
        {
            snapshot = Some(content.clone());
        }
    }
    assert_eq!(snapshot.as_deref(), Some(&b"1 running\n"[..]), "the watch begins from the item as it is");
}

/// An agent step that reports, in the first repository.
fn task(name: &[u8]) -> plan::Step {
    let charter = plan::Charter {
        instructions: copy_of(b"look into it"),
        template: None,
        grants: plan::Grants { modify: false, shell: false, forge: true, subagents: false, note: false },
        budget: Budget { tokens: 100, turns: 5, time: Duration::from_secs(60) },
    };
    plan::Step {
        name: copy_of(name),
        repository: plan::Repository(0),
        work: plan::Work::Agent(plan::AgentSpec { charter, grows: false }),
        after: Box::new([]),
        gates: Box::new([]),
    }
}

fn ended(outcome: crate::boundary::Outcome) -> crate::boundary::Answer {
    crate::boundary::Answer::Ended { outcome, work: crate::boundary::Work { landed: Box::new([]) } }
}

#[test]
fn a_task_a_session_makes_runs_and_is_closed_once_it_reports() {
    let (mut world, session) = World::session();
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([task(b"look")]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    assert!(acknowledged(&world.seen, session, 1), "the session's answer is applied: {:?}", world.seen.as_slice());
    let task = Item { repository: 0, number: 2 };
    assert!(world.forge.issue(task).is_some(), "the task's issue is made");
    let assigned = assignment(&world.seen).expect("the task is assigned");
    assert_eq!(assigned.item, task, "the task runs");
    let report = crate::boundary::Outcome::Report { text: copy_of(b"found it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: task, attempt: 1, answer: ended(report) });
    assert!(acknowledged(&world.seen, task, 1), "the task's report is applied");
    assert!(!world.forge.issue(task).unwrap().open, "the task's issue is closed once it reports");
    assert_eq!(phase(&mut world, task), Some(work::Phase::Done), "its record says done");
}

/// A change step into `main`, which a person reviews.
fn change_step(name: &[u8]) -> plan::Step {
    let produce = plan::Charter {
        instructions: copy_of(b"fix it"),
        template: None,
        grants: plan::Grants { modify: true, shell: true, forge: true, subagents: false, note: false },
        budget: Budget { tokens: 100, turns: 5, time: Duration::from_secs(60) },
    };
    let change = plan::ChangeSpec { base: copy_of(b"main"), produce, checks: false, review: plan::Review::Person };
    plan::Step {
        name: copy_of(name),
        repository: plan::Repository(0),
        work: plan::Work::Change(change),
        after: Box::new([]),
        gates: Box::new([]),
    }
}

/// A session's task, a change into `main`, pushed and its pull request
/// opened: the world, the change's item, its pull request's, and its head.
fn opened_change() -> (World, Item, Item, [u8; 32]) {
    let (mut world, session) = World::session();
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([change_step(b"fix")]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    let change = Item { repository: 0, number: 2 };
    let head = [1; 32];
    world.forge.branches.push((copy_of(b"temper/2"), head)).unwrap();
    let landed = crate::boundary::Landed { repository: 0, commit: head };
    let outcome = crate::boundary::Outcome::Change { message: copy_of(b"fixed") };
    let answer = crate::boundary::Answer::Ended { outcome, work: crate::boundary::Work { landed: Box::new([landed]) } };
    world.deliver(Event::Answer { channel: Token::new(1), item: change, attempt: 1, answer });
    let pull = Item { repository: 0, number: 3 };
    assert!(world.forge.change(pull).is_some(), "the engine opens its pull request");
    (world, change, pull, head)
}

#[test]
fn a_merge_refused_for_a_conflict_sends_the_change_back_for_a_rebase() {
    let (mut world, change, pull, head) = opened_change();
    world.forge.change(pull).unwrap().conflicts = true;
    let review = api::Review {
        id: 500,
        author: ALICE,
        verdict: api::Verdict::Approve,
        commit: head,
        key: None,
        body: copy_of(b"ok"),
    };
    world.forge.change(pull).unwrap().reviews.push(review).unwrap();
    world.forge.issue(pull).unwrap().updated = world.env().now;
    world.deliver(Event::Hint { repository: 0, item: Some(pull.number), commit: None, branch: None });
    for _ in 0_u32..10 {
        world.wait(30);
    }
    assert!(world.forge.change(pull).unwrap().merged.is_none(), "the forge refused the merge");
    assert_ne!(held_for(&mut world, change), Some(work::Hold::Writes), "a conflict is not a write failed for good");
    let assigned = assignment(&world.seen).expect("a run is assigned");
    assert_eq!(assigned.item, change, "the change runs again");
    assert_eq!(assigned.charter.why, plan::Why::Repair(plan::Repair::Conflicts), "to repair the conflict");
}

#[test]
fn a_change_is_pushed_opened_reviewed_and_merged_into_a_protected_branch() {
    let (mut world, session) = World::session();
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([change_step(b"fix")]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    let change = Item { repository: 0, number: 2 };
    let assigned = assignment(&world.seen).expect("the change is assigned");
    assert_eq!(assigned.item, change, "the change runs");
    let checkout = assigned.workspace.repositories.first().unwrap();
    assert_eq!(checkout.push.as_deref(), Some(&b"temper/2"[..]), "it may push to its branch");
    // The worker pushes, and answers.
    let head = [1; 32];
    world.forge.branches.push((copy_of(b"temper/2"), head)).unwrap();
    let landed = crate::boundary::Landed { repository: 0, commit: head };
    let outcome = crate::boundary::Outcome::Change { message: copy_of(b"fixed") };
    let answer = crate::boundary::Answer::Ended { outcome, work: crate::boundary::Work { landed: Box::new([landed]) } };
    world.deliver(Event::Answer { channel: Token::new(1), item: change, attempt: 1, answer });
    assert!(acknowledged(&world.seen, change, 1), "the change's answer is applied");
    let pull = Item { repository: 0, number: 3 };
    assert!(world.forge.change(pull).is_some(), "the engine opens its pull request");
    world.wait(60);
    assert!(world.forge.change(pull).unwrap().merged.is_none(), "nothing lands on main before a person approves");
    let review = api::Review {
        id: 500,
        author: ALICE,
        verdict: api::Verdict::Approve,
        commit: head,
        key: None,
        body: copy_of(b"lgtm"),
    };
    world.forge.change(pull).unwrap().reviews.push(review).unwrap();
    world.forge.issue(pull).unwrap().updated = world.env().now;
    world.deliver(Event::Hint { repository: 0, item: Some(pull.number), commit: None, branch: None });
    for _ in 0_u32..10 {
        world.wait(30);
    }
    assert_eq!(world.forge.change(pull).unwrap().merged, Some([8; 32]), "it is merged at its exact head once approved");
    assert!(
        !world.forge.issue(change).unwrap().open,
        "the change's item is closed once it has landed: {:?}",
        phase(&mut world, change)
    );
}

/// A plan of one change into `main`, which the rules want a person to
/// accept: it lands on a protected branch.
fn proposal() -> crate::boundary::Outcome {
    let envelope = plan::Envelope {
        agents: 0,
        changes: 0,
        waits: 0,
        sessions: 0,
        repositories: Box::new([plan::Repository(0)]),
        into: Box::new([plan::Target { repository: plan::Repository(0), base: copy_of(b"main") }]),
    };
    let plan = plan::Plan { steps: Box::new([change_step(b"fix")]), envelope, budget: 500 };
    crate::boundary::Outcome::Plan { plan, text: copy_of(b"shall we?") }
}

fn held_for(world: &mut World, item: Item) -> Option<work::Hold> {
    match phase(world, item) {
        Some(work::Phase::Held { why, .. }) => Some(why),
        Some(_) | None => None,
    }
}

#[test]
fn a_plan_landing_on_a_protected_branch_waits_for_a_persons_acceptance() {
    let (mut world, session) = World::session();
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(proposal()) });
    assert!(acknowledged(&world.seen, session, 1), "the proposal is recorded");
    assert_eq!(held_for(&mut world, session), Some(work::Hold::Acceptance), "it waits for a person");
    assert!(world.forge.issue(Item { repository: 0, number: 2 }).is_none(), "nothing of it is made yet");
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(14)),
        person: ALICE,
        ask: Ask::Accept { item: session },
    });
    assert!(replied(&world.seen, Reply::Done), "the acceptance is taken: {:?}", world.seen.as_slice());
    assert!(world.forge.issue(Item { repository: 0, number: 2 }).is_some(), "once accepted, its step's item is made");
    assert_eq!(held_for(&mut world, session), None, "the session goes on: {:?}", phase(&mut world, session));
}

#[test]
fn a_rejected_plan_is_not_made() {
    let (mut world, session) = World::session();
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(proposal()) });
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(15)),
        person: ALICE,
        ask: Ask::Reject { item: session },
    });
    assert!(replied(&world.seen, Reply::Done), "the rejection is taken");
    assert!(world.forge.issue(Item { repository: 0, number: 2 }).is_none(), "nothing of it is made");
    assert_eq!(held_for(&mut world, session), None, "the session goes on: {:?}", phase(&mut world, session));
}

#[test]
fn a_run_notes_what_it_learnt_and_recalls_it() {
    let (mut world, item) = World::session();
    let page = notes::Page {
        description: copy_of(b"the build is flaky"),
        author: notes::Author::Run { repository: 0, number: item.number },
        references: Box::new([]),
        body: copy_of(b"retry it"),
    };
    let note = crate::boundary::Call::Note {
        scope: notes::Scope::Repository(0),
        name: copy_of(b"flaky"),
        change: notes::Change::New(page),
    };
    world.deliver(Event::Relay { channel: Token::new(1), item, attempt: 1, call: Token::new(1), body: note });
    let recall = notes::Recall::Name { scope: notes::Scope::Repository(0), name: copy_of(b"flaky") };
    world.deliver(Event::Relay {
        channel: Token::new(1),
        item,
        attempt: 1,
        call: Token::new(2),
        body: crate::boundary::Call::Recall(recall),
    });
    let mut noted = None;
    let mut recalled = None;
    for request in &world.seen {
        if let Request::Relayed { call, served, .. } = request {
            if *call == Token::new(1)
                && let crate::boundary::Served::Noted(how) = served
            {
                noted = Some(*how);
            }
            if *call == Token::new(2)
                && let crate::boundary::Served::Recalled { entries, .. } = served
            {
                recalled = Some(entries.len());
            }
        }
    }
    assert_eq!(noted, Some(notes::Noted::Done), "the note is written: {:?}", world.seen.as_slice());
    assert_eq!(recalled, Some(1), "the note is recalled");
}

/// The answers the workers were sent to their calls, by the calls' names.
fn served(seen: &[Request], call: Token) -> List<crate::boundary::Served> {
    let mut found = List::with_capacity(8);
    for request in seen {
        if let Request::Relayed { call: of, served, .. } = request
            && *of == call
        {
            let copy = match served {
                crate::boundary::Served::Unserved(why) => crate::boundary::Served::Unserved(*why),
                crate::boundary::Served::Posted { comment } => crate::boundary::Served::Posted { comment: *comment },
                crate::boundary::Served::Read(_)
                | crate::boundary::Served::Recalled { .. }
                | crate::boundary::Served::Noted(_) => crate::boundary::Served::Posted { comment: 0 },
            };
            found.push(copy).unwrap();
        }
    }
    found
}

/// The outcomes posted on `item`.
fn outcomes(world: &mut World, item: Item) -> u32 {
    let issue = world.forge.issue(item).unwrap();
    let mut outcomes = 0_u32;
    for note in &issue.comments {
        if let Some(crate::boundary::Decoded::Outcome { .. }) = note.decoded {
            outcomes += 1;
        }
    }
    outcomes
}

#[test]
fn an_adopted_runs_calls_are_served_within_its_steps_grants() {
    let (mut world, item) = World::session();
    world.restart();
    let hosted = crate::boundary::Hosted { item, attempt: 1, phase: fleet::Phase::Active };
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([hosted]) };
    world.deliver(Event::Hello { channel: Token::new(2), hello });
    let comment = crate::boundary::Call::Comment { text: copy_of(b"still here") };
    world.deliver(Event::Relay { channel: Token::new(2), item, attempt: 1, call: Token::new(5), body: comment });
    let answers = served(world.seen.as_slice(), Token::new(5));
    assert!(
        matches_posted(answers.as_slice()),
        "the adopted run's call is served as its claim granted: {:?}",
        answers.as_slice()
    );
}

fn matches_posted(answers: &[crate::boundary::Served]) -> bool {
    match answers {
        [crate::boundary::Served::Posted { .. }] => true,
        [] | [_, ..] => false,
    }
}

#[test]
fn a_call_that_reaches_no_live_claim_is_answered_at_once_on_its_channel() {
    let (mut world, item) = World::session();
    // Before the cold start is done, a call may yet be adopted: busy.
    world.domain = domain();
    world.calls = Some(0);
    world.settle();
    let comment = crate::boundary::Call::Comment { text: copy_of(b"anyone?") };
    world.deliver(Event::Relay { channel: Token::new(3), item, attempt: 1, call: Token::new(6), body: comment });
    let busy = crate::boundary::Served::Unserved(crate::boundary::Unserved::Busy);
    assert_eq!(served(world.seen.as_slice(), Token::new(6)).as_slice(), [busy], "a call made too early may come again");
    // Once it is done, an attempt that is not the live claim is fenced off.
    world.restart();
    let comment = crate::boundary::Call::Comment { text: copy_of(b"stale") };
    world.deliver(Event::Relay { channel: Token::new(3), item, attempt: 7, call: Token::new(7), body: comment });
    let failed = crate::boundary::Served::Unserved(crate::boundary::Unserved::Failed);
    let answers = served(world.seen.as_slice(), Token::new(7));
    assert_eq!(answers.as_slice(), [failed], "a fenced-off call is told so");
    for request in &world.seen {
        if let Request::Relayed { channel, call, .. } = request
            && *call == Token::new(7)
        {
            assert_eq!(*channel, Token::new(3), "on the channel it came on");
        }
    }
}

#[test]
fn an_adopted_runs_outcome_posted_before_a_restart_is_found_not_posted_again() {
    let (mut world, item) = World::session();
    // The forge takes the outcome's post, then answers nothing more: the
    // record never says it is being applied.
    world.calls = Some(1);
    world.deliver(Event::Answer { channel: Token::new(1), item, attempt: 1, answer: replied_answer() });
    assert_eq!(outcomes(&mut world, item), 1, "the outcome is posted");
    world.restart();
    let hosted = crate::boundary::Hosted { item, attempt: 1, phase: fleet::Phase::Answered };
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([hosted]) };
    world.deliver(Event::Hello { channel: Token::new(2), hello });
    world.deliver(Event::Answer { channel: Token::new(2), item, attempt: 1, answer: replied_answer() });
    assert!(acknowledged(&world.seen, item, 1), "the answer is applied: {:?}", world.seen.as_slice());
    assert_eq!(outcomes(&mut world, item), 1, "the outcome an earlier life posted is found, not posted again");
}

#[test]
fn a_decision_on_what_waits_for_none_is_refused() {
    let (mut world, item) = World::session();
    let from = world.seen.len();
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(20)), person: ALICE, ask: Ask::Accept { item } });
    let refused = Reply::Refused(Refusal::Unheld);
    assert!(replied(&world.seen, refused), "nothing waits for acceptance: {:?}", world.since(from));
    let record = {
        let issue = world.forge.issue(item).unwrap();
        let mut found = None;
        for note in &issue.comments {
            if let Some(crate::boundary::Decoded::Record { record, .. }) = &note.decoded {
                found = Some(record.relations.accepted);
            }
        }
        found
    };
    assert_eq!(record, Some(None), "no acceptance is kept to count for a later proposal");
}

/// The assignments of `item` the workers were sent, by attempt, and where
/// each one's checkout starts.
fn starts(seen: &List<Request>, item: Item) -> List<(u64, crate::boundary::Start)> {
    let mut found = List::with_capacity(16);
    for request in seen {
        if let Request::Assign { assignment, .. } = request
            && assignment.item == item
            && let Some(checkout) = assignment.workspace.repositories.first()
        {
            found.push((assignment.attempt, checkout.start.clone())).unwrap();
        }
    }
    found
}

#[test]
fn a_push_whose_answer_comes_after_its_run_was_presumed_lost_is_learned() {
    let (mut world, session) = World::session();
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([change_step(b"fix")]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    let change = Item { repository: 0, number: 2 };
    assert!(matches_base(starts(&world.seen, change).as_slice()), "the change's first run starts from the base");
    // Its worker goes out of reach past the grace, and its run pushes
    // meanwhile.
    world.deliver(Event::Lost { channel: Token::new(1) });
    for _ in 0_u32..8 {
        world.wait(10);
    }
    let head = [7; 32];
    world.forge.branches.push((copy_of(b"temper/2"), head)).unwrap();
    // It comes back with the answer of the attempt presumed lost.
    let hosted = crate::boundary::Hosted { item: change, attempt: 1, phase: fleet::Phase::Answered };
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([hosted]) };
    world.deliver(Event::Hello { channel: Token::new(2), hello });
    let landed = crate::boundary::Landed { repository: 0, commit: head };
    let outcome = crate::boundary::Outcome::Change { message: copy_of(b"late") };
    let late = crate::boundary::Answer::Ended { outcome, work: crate::boundary::Work { landed: Box::new([landed]) } };
    world.deliver(Event::Answer { channel: Token::new(2), item: change, attempt: 1, answer: late });
    assert!(acknowledged(&world.seen, change, 1), "the late answer is acknowledged, and dropped");
    // The attempt after it fails: the change is not made again from the
    // base, its pull request is opened for the branch the late answer said.
    let (attempt, _) = *starts(&world.seen, change).as_slice().last().expect("the change runs again");
    let failed = crate::boundary::Answer::Failed {
        failure: crate::boundary::Failure::Transient,
        work: crate::boundary::Work { landed: Box::new([]) },
    };
    world.deliver(Event::Answer { channel: Token::new(2), item: change, attempt, answer: failed });
    for _ in 0_u32..8 {
        world.wait(10);
    }
    let branch = {
        let issue = world.forge.issue(change).unwrap();
        let mut branch = None;
        for note in &issue.comments {
            if let Some(crate::boundary::Decoded::Record { record, .. }) = &note.decoded {
                branch = record.relations.branch;
            }
        }
        branch
    };
    assert_eq!(branch, Some(head), "the record says where the late answer pushed");
    let pull = Item { repository: 0, number: 3 };
    assert!(world.forge.change(pull).is_some(), "the change's pull request is opened for it");
}

fn matches_base(starts: &[(u64, crate::boundary::Start)]) -> bool {
    match starts {
        [(1, crate::boundary::Start::Base { .. })] => true,
        [] | [_, ..] => false,
    }
}

/// The attempts of `item` the workers were assigned.
fn assigned(seen: &[Request], item: Item) -> List<u64> {
    let mut found = List::with_capacity(16);
    for request in seen {
        if let Request::Assign { assignment, .. } = request
            && assignment.item == item
        {
            found.push(assignment.attempt).unwrap();
        }
    }
    found
}

#[test]
fn a_release_the_hub_refuses_changes_nothing() {
    let (mut world, item) = World::session();
    let ask = Ask::Release { item };
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(21)), person: ALICE, ask });
    assert!(replied(&world.seen, Reply::Refused(Refusal::Unheld)), "a running session is not held");
    let running = world.domain.items.get(crate::items::find(&world.domain, item).unwrap()).unwrap();
    let progress = running.step.as_ref().unwrap().progress;
    assert!(progress.running.is_some() && progress.released.is_none(), "its claim stands: {progress:?}");
    // Its turn replies, and nothing more is due until a message comes: the
    // refused release woke nothing.
    world.deliver(Event::Answer { channel: Token::new(1), item, attempt: 1, answer: replied_answer() });
    assert!(acknowledged(&world.seen, item, 1), "the turn's answer is applied");
    for _ in 0_u32..4 {
        world.wait(30);
    }
    assert_eq!(assigned(world.seen.as_slice(), item).as_slice(), [1], "the session takes no turn of its own");
}

/// Whether a message reached the attempt `attempt` of `item`, in what the
/// workers were sent: a comment relayed to it.
fn relayed_comment(seen: &[Request], item: Item, attempt: u64) -> bool {
    for request in seen {
        if let Request::Inbound { item: of, attempt: at, event: crate::boundary::Inbound::News(news), .. } = request
            && *of == item
            && *at == attempt
            && let forge::News::Comment { .. } = news
        {
            return true;
        }
    }
    false
}

/// A person's message on `item`, which the forge child domain is hinted at and
/// reads.
fn message(world: &mut World, item: Item, key: &[u8], reply_to: u64) {
    let ask = Ask::Message { item, key: copy_of(key), message: copy_of(b"and another thing") };
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(reply_to)), person: ALICE, ask });
    world.deliver(Event::Hint { repository: 0, item: Some(item.number), commit: None, branch: None });
    world.wait(60);
}

#[test]
fn a_message_reaches_a_supervisor_whatever_its_steps_escalated_meanwhile() {
    let (mut world, session) = World::session();
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(proposal()) });
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(29)),
        person: ALICE,
        ask: Ask::Accept { item: session },
    });
    let task = Item { repository: 0, number: 2 };
    assert_eq!(assigned(world.seen.as_slice(), task).as_slice(), [1], "the plan's step runs");
    message(&mut world, session, b"m1", 30);
    assert_eq!(assigned(world.seen.as_slice(), session).as_slice(), [1, 2], "the message wakes the session");
    // The task escalates, again and again, while the session's turn is live:
    // the session hears of each.
    let from = world.seen.len();
    for call in 1_u64..=40 {
        let escalate = crate::boundary::Call::Escalate { text: copy_of(b"stuck") };
        world.deliver(Event::Relay {
            channel: Token::new(1),
            item: task,
            attempt: 1,
            call: Token::new(call),
            body: escalate,
        });
    }
    let mut held = 0_u32;
    for request in world.since(from) {
        if let Request::Inbound { item: of, attempt: 2, event: crate::boundary::Inbound::Held { .. }, .. } = request {
            held += u32::from(*of == session);
        }
    }
    assert_eq!(held, 40, "the session hears of each escalation");
    let from = world.seen.len();
    message(&mut world, session, b"m2", 31);
    assert!(
        relayed_comment(world.since(from), session, 2),
        "the message reaches the live turn: {:?}",
        world.since(from)
    );
}

#[test]
fn news_the_inbox_had_no_room_for_is_told_again_once_a_turn_made_room() {
    let (mut world, session) = World::session();
    // The limits leave room for every news the forge child domain holds: an
    // inbox filled by hand stands for one that held more than it says.
    let id = crate::items::find(&world.domain, session).unwrap();
    let entry = world.domain.items.get_mut(id).unwrap();
    for _ in 0..crate::limits::inbox(&LIMITS).unwrap() {
        let seq = entry.next;
        entry.next += 1;
        let noted = crate::items::Noted {
            inbound: crate::boundary::Inbound::Decided { accepted: true },
            news: None,
            source: plan::Source::Message,
            at: Time::ZERO,
            delivered: true,
        };
        entry.inbox.insert(seq, noted).unwrap();
    }
    let from = world.seen.len();
    message(&mut world, session, b"m1", 32);
    assert!(!relayed_comment(world.since(from), session, 1), "no room for the message");
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: replied_answer() });
    world.wait(60);
    assert_eq!(assigned(world.seen.as_slice(), session).as_slice(), [1, 2], "the message, told again, wakes it");
}

/// A plan of one agent step, `build`, which may grow the plan by one more
/// agent step.
fn growing_plan() -> crate::boundary::Outcome {
    let charter = plan::Charter {
        instructions: copy_of(b"split it up"),
        template: None,
        grants: plan::Grants { modify: false, shell: false, forge: true, subagents: false, note: false },
        budget: Budget { tokens: 100, turns: 5, time: Duration::from_secs(60) },
    };
    let build = plan::Step {
        name: copy_of(b"build"),
        repository: plan::Repository(0),
        work: plan::Work::Agent(plan::AgentSpec { charter, grows: true }),
        after: Box::new([]),
        gates: Box::new([]),
    };
    let envelope = plan::Envelope {
        agents: 1,
        changes: 0,
        waits: 0,
        sessions: 0,
        repositories: Box::new([plan::Repository(0)]),
        into: Box::new([]),
    };
    let plan = plan::Plan { steps: Box::new([build]), envelope, budget: 500 };
    crate::boundary::Outcome::Plan { plan, text: copy_of(b"shall we?") }
}

/// The names of the steps of the goal the record on `item` says.
fn goal_steps(world: &mut World, item: Item) -> List<Box<[u8]>> {
    let mut names = List::with_capacity(8);
    let issue = world.forge.issue(item).unwrap();
    let mut goal = None;
    for note in &issue.comments {
        if let Some(crate::boundary::Decoded::Record { record, .. }) = &note.decoded {
            goal = record.step.goal.clone();
        }
    }
    for entry in &goal.expect("the record carries the goal").steps {
        names.push(entry.name.clone()).unwrap();
    }
    names
}

#[test]
fn a_goals_record_its_growth_wrote_lands_before_the_growing_step_goes_on() {
    let (mut world, session) = World::session();
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(growing_plan()) });
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(33)),
        person: ALICE,
        ask: Ask::Accept { item: session },
    });
    let build = Item { repository: 0, number: 2 };
    assert_eq!(assigned(world.seen.as_slice(), build).as_slice(), [1], "the plan's step runs");
    assert_eq!(goal_steps(&mut world, session).as_slice(), [copy_of(b"build")], "the goal's plan is written");
    // The build adds a step, and the goal's record times out for as long as
    // the forge child domain tries it, once.
    world.forge.stuck = Some((session, LIMITS.forge.attempts));
    let steps = crate::boundary::Outcome::Steps { steps: Box::new([task(b"more")]), text: copy_of(b"and more") };
    world.deliver(Event::Answer { channel: Token::new(1), item: build, attempt: 1, answer: ended(steps) });
    let applying = matches_applying(phase(&mut world, build));
    assert!(applying, "the growing step waits for the goal's record: {:?}", phase(&mut world, build));
    for _ in 0_u32..8 {
        world.wait(10);
    }
    assert_eq!(world.forge.stuck, Some((session, 0)), "the goal's record timed out");
    assert!(!matches_applying(phase(&mut world, build)), "then goes on");
    assert!(acknowledged(&world.seen, build, 1), "the growth is applied");
    world.restart();
    let names = goal_steps(&mut world, session);
    assert_eq!(names.as_slice(), [copy_of(b"build"), copy_of(b"more")], "the goal lists the step it grew by");
}

#[test]
fn growth_whose_goals_record_cannot_be_written_is_held_unapplied() {
    let (mut world, session) = World::session();
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(growing_plan()) });
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(34)),
        person: ALICE,
        ask: Ask::Accept { item: session },
    });
    let build = Item { repository: 0, number: 2 };
    world.forge.stuck = Some((session, u32::MAX));
    let steps = crate::boundary::Outcome::Steps { steps: Box::new([task(b"more")]), text: copy_of(b"and more") };
    world.deliver(Event::Answer { channel: Token::new(1), item: build, attempt: 1, answer: ended(steps) });
    for _ in 0_u32..20 {
        world.wait(10);
    }
    assert_eq!(held_for(&mut world, build), Some(work::Hold::Writes), "the growth is held, its outcome kept");
}

/// Moves the change's pull request to `head`, pushed to its branch, with CI
/// `ci` on it.
fn pushed(world: &mut World, branch: &[u8], pull: Item, head: [u8; 32], ci: forge::Ci) {
    for at in 0..world.forge.branches.len() {
        let (name, commit) = world.forge.branches.get_mut(at).unwrap();
        if **name == *branch {
            *commit = head;
        }
    }
    let change = world.forge.change(pull).unwrap();
    change.commit = head;
    change.ci = ci;
    world.forge.issue(pull).unwrap().updated = world.env().now;
    world.deliver(Event::Hint { repository: 0, item: Some(pull.number), commit: Some(head), branch: None });
    world.wait(5);
}

/// The answer of a run that pushed `head` to the first repository.
fn changed(head: [u8; 32]) -> crate::boundary::Answer {
    let landed = crate::boundary::Landed { repository: 0, commit: head };
    let outcome = crate::boundary::Outcome::Change { message: copy_of(b"fixed") };
    crate::boundary::Answer::Ended { outcome, work: crate::boundary::Work { landed: Box::new([landed]) } }
}

#[test]
fn a_change_gated_on_acceptance_is_repaired_and_lands_on_one_acceptance() {
    let (mut world, session) = World::session();
    let gated = plan::Step { gates: Box::new([plan::Gate::Accepted]), ..change_step(b"fix") };
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([gated]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    let change = Item { repository: 0, number: 2 };
    assert!(assigned(world.seen.as_slice(), change).is_empty(), "nothing runs before a person accepts the step");
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(35)),
        person: ALICE,
        ask: Ask::Accept { item: change },
    });
    assert_eq!(assigned(world.seen.as_slice(), change).as_slice(), [1], "accepted, the change is made");
    world.forge.branches.push((copy_of(b"temper/2"), [1; 32])).unwrap();
    world.deliver(Event::Answer { channel: Token::new(1), item: change, attempt: 1, answer: changed([1; 32]) });
    let pull = Item { repository: 0, number: 3 };
    assert!(world.forge.change(pull).is_some(), "its pull request is opened on the same acceptance");
    // CI fails on it: it is repaired, on the same acceptance.
    pushed(&mut world, b"temper/2", pull, [1; 32], forge::Ci::Failed);
    world.wait(60);
    let attempts = assigned(world.seen.as_slice(), change);
    assert_eq!(attempts.as_slice(), [1, 2], "the change is repaired: {:?}", held_for(&mut world, change));
    pushed(&mut world, b"temper/2", pull, [2; 32], forge::Ci::Passed);
    world.deliver(Event::Answer { channel: Token::new(1), item: change, attempt: 2, answer: changed([2; 32]) });
    let review = api::Review {
        id: 501,
        author: ALICE,
        verdict: api::Verdict::Approve,
        commit: [2; 32],
        key: None,
        body: copy_of(b"lgtm"),
    };
    world.forge.change(pull).unwrap().reviews.push(review).unwrap();
    world.forge.issue(pull).unwrap().updated = world.env().now;
    world.deliver(Event::Hint { repository: 0, item: Some(pull.number), commit: None, branch: None });
    for _ in 0_u32..10 {
        world.wait(30);
    }
    let held = held_for(&mut world, change);
    assert_eq!(world.forge.change(pull).unwrap().merged, Some([8; 32]), "it lands: {held:?}");
    assert_eq!(assigned(world.seen.as_slice(), change).as_slice(), [1, 2], "made, then repaired once");
}

#[test]
fn a_step_accepted_and_then_released_waits_to_be_accepted_again() {
    let (mut world, session) = World::session();
    let gated = plan::Step { gates: Box::new([plan::Gate::Accepted]), ..change_step(b"fix") };
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([gated]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    let change = Item { repository: 0, number: 2 };
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(36)),
        person: ALICE,
        ask: Ask::Accept { item: change },
    });
    // Its runs fail until it is held.
    for attempt in 1_u64..=3 {
        assert_eq!(assigned(world.seen.as_slice(), change).as_slice().last(), Some(&attempt), "attempt {attempt} runs");
        let failed = crate::boundary::Answer::Failed {
            failure: crate::boundary::Failure::Run,
            work: crate::boundary::Work { landed: Box::new([]) },
        };
        world.deliver(Event::Answer { channel: Token::new(1), item: change, attempt, answer: failed });
        world.wait(10);
    }
    assert_eq!(held_for(&mut world, change), Some(work::Hold::Failures(work::Class::Run)), "held for its failures");
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(37)),
        person: ALICE,
        ask: Ask::Release { item: change },
    });
    world.wait(10);
    let accepting = work::Hold::Plan { reason: translate::RUN_ACCEPTANCE };
    assert_eq!(held_for(&mut world, change), Some(accepting), "released, it waits for its acceptance again");
    assert_eq!(assigned(world.seen.as_slice(), change).len(), 3, "and runs nothing before");
}

/// Edits the record on `item`, as a person might, so that it still decodes.
fn edit_record(world: &mut World, item: Item, edit: fn(&mut crate::boundary::Record)) {
    let issue = world.forge.issue(item).unwrap();
    for at in 0..issue.comments.len() {
        let note = issue.comments.get_mut(at).unwrap();
        if let Some(crate::boundary::Decoded::Record { record, .. }) = &mut note.decoded {
            edit(record);
        }
    }
}

/// Why the hub last held `item`, as its facts tell, among those not drained
/// yet.
fn hold_told(world: &mut World, item: Item) -> Option<work::Hold> {
    let mut why = None;
    while let Some(fact) = world.domain.pop_fact() {
        if let crate::facts::Fact::Work { fact: work::Fact::Held { item: of, why: held } } = fact
            && of == item
        {
            why = Some(held);
        }
    }
    why
}

/// Gives a change's review a megabyte of instructions.
fn wordy_review(record: &mut crate::boundary::Record) {
    if let plan::Work::Change(spec) = &mut record.step.step.work {
        let mut instructions = List::with_capacity(1 << 20);
        for _ in 0_u32..1 << 20_u32 {
            instructions.push(b'x').unwrap();
        }
        let instructions = instructions.into_boxed();
        spec.review = plan::Review::Agent(plan::Charter { instructions, ..spec.produce.clone() });
    }
}

#[test]
fn a_record_whose_step_the_plan_could_not_have_written_is_held_not_run() {
    let (mut world, session) = World::session();
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([change_step(b"fix")]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    let change = Item { repository: 0, number: 2 };
    assert_eq!(assigned(world.seen.as_slice(), change).as_slice(), [1], "the change runs");
    // A person gives its review a megabyte of instructions.
    edit_record(&mut world, change, wordy_review);
    world.restart();
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    world.deliver(Event::Hello { channel: Token::new(2), hello });
    for _ in 0_u32..4 {
        world.wait(30);
    }
    assert_eq!(hold_told(&mut world, change), Some(work::Hold::Record), "the change is held for a person");
    assert!(assigned(world.seen.as_slice(), change).is_empty(), "and runs nothing");
}

#[test]
fn a_change_made_again_starts_from_a_push_whose_answer_never_came() {
    let (mut world, session) = World::session();
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([change_step(b"fix")]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    let change = Item { repository: 0, number: 2 };
    assert!(matches_base(starts(&world.seen, change).as_slice()), "the change's first run starts from the base");
    // Its run pushes, and its worker is lost for good, its answer with it.
    world.forge.branches.push((copy_of(b"temper/2"), [7; 32])).unwrap();
    world.deliver(Event::Lost { channel: Token::new(1) });
    for _ in 0_u32..8 {
        world.wait(10);
    }
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    world.deliver(Event::Hello { channel: Token::new(2), hello });
    world.wait(10);
    let starts = starts(&world.seen, change);
    let Some((attempt, start)) = starts.as_slice().last() else { panic!("the change runs again") };
    assert!(*attempt > 1, "a later attempt: {:?}", starts.as_slice());
    assert_eq!(*start, crate::boundary::Start::Branch { branch: copy_of(b"temper/2") }, "it starts from the push");
}

#[test]
fn a_late_answer_after_a_restart_never_moves_the_branch_back() {
    let (mut world, change, pull, _) = opened_change();
    // CI fails on the first push, and a repair pushes again.
    pushed(&mut world, b"temper/2", pull, [1; 32], forge::Ci::Failed);
    world.wait(60);
    let (attempt, _) = *starts(&world.seen, change).as_slice().last().expect("a repair runs");
    pushed(&mut world, b"temper/2", pull, [2; 32], forge::Ci::Passed);
    world.deliver(Event::Answer { channel: Token::new(1), item: change, attempt, answer: changed([2; 32]) });
    world.restart();
    // The first attempt's answer comes again, from a worker that never heard
    // it was taken.
    let hosted = crate::boundary::Hosted { item: change, attempt: 1, phase: fleet::Phase::Answered };
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([hosted]) };
    world.deliver(Event::Hello { channel: Token::new(3), hello });
    world.deliver(Event::Answer { channel: Token::new(3), item: change, attempt: 1, answer: changed([1; 32]) });
    world.wait(30);
    let id = crate::items::find(&world.domain, change).unwrap();
    let branch = world.domain.items.get(id).unwrap().relations.branch;
    assert_eq!(branch, Some([2; 32]), "the branch stays where the repair pushed it");
}

/// The text of the comments section of the brief of `item`'s attempt
/// `attempt`, as the workers were sent it.
fn comments_briefed(seen: &[Request], item: Item, attempt: u64) -> Option<Box<[u8]>> {
    for request in seen {
        if let Request::Assign { assignment, .. } = request
            && assignment.item == item
            && assignment.attempt == attempt
        {
            for section in &assignment.charter.brief {
                if section.kind == brief::Kind::Comments
                    && let brief::Body::Text(text) = &section.body
                {
                    return Some(text.clone());
                }
            }
        }
    }
    None
}

#[test]
fn messages_a_brief_has_no_room_for_reach_the_next_turn() {
    let (mut world, session) = World::session();
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: replied_answer() });
    // Two messages, each most of the comments' budget, while no turn runs.
    let first: Box<[u8]> = Box::from([b'a'; 50].as_slice());
    let second: Box<[u8]> = Box::from([b'b'; 50].as_slice());
    for (key, text, reply_to) in [(&b"m1"[..], first.clone(), 40_u64), (&b"m2"[..], second.clone(), 41)] {
        let ask = Ask::Message { item: session, key: copy_of(key), message: text };
        world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(reply_to)), person: ALICE, ask });
    }
    world.deliver(Event::Hint { repository: 0, item: Some(session.number), commit: None, branch: None });
    world.wait(60);
    assert_eq!(comments_briefed(world.seen.as_slice(), session, 2), Some(first), "the oldest message, whole");
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 2, answer: replied_answer() });
    world.wait(60);
    assert_eq!(comments_briefed(world.seen.as_slice(), session, 3), Some(second), "the next turn has the other");
}

#[test]
fn a_plan_proposed_beyond_what_its_goal_may_spend_is_refused() {
    let (mut world, session) = World::session();
    // Its runs have spent nearly all a goal may.
    let id = crate::items::find(&world.domain, session).unwrap();
    world.domain.items.get_mut(id).unwrap().relations.spent = 9_950;
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(proposal()) });
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(42)),
        person: ALICE,
        ask: Ask::Accept { item: session },
    });
    assert!(world.forge.issue(Item { repository: 0, number: 2 }).is_none(), "nothing of it is made");
}

/// A task that waits for a person's decision, and so is not done.
fn waiting_task(name: &[u8]) -> plan::Step {
    plan::Step {
        name: copy_of(name),
        repository: plan::Repository(0),
        work: plan::Work::Wait(plan::WaitSpec::Decision),
        after: Box::new([]),
        gates: Box::new([]),
    }
}

#[test]
fn a_task_past_those_a_session_may_keep_is_refused_not_made() {
    let (mut world, session) = World::session();
    let turns: [&[&[u8]]; 4] = [&[b"w1", b"w2", b"w3"], &[b"w4", b"w5", b"w6"], &[b"w7", b"w8"], &[b"w9"]];
    for (turn, names) in turns.iter().enumerate() {
        let attempt = u64::try_from(turn).unwrap() + 1;
        let mut tasks = List::with_capacity(3);
        for name in *names {
            tasks.push(waiting_task(name)).unwrap();
        }
        let tasks = crate::boundary::Outcome::Tasks { tasks: tasks.into_boxed(), text: copy_of(b"on it") };
        world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt, answer: ended(tasks) });
        message(&mut world, session, &[b'm', b'0' + u8::try_from(turn).unwrap()], 50 + attempt);
    }
    assert!(world.forge.issue(Item { repository: 0, number: 9 }).is_some(), "eight tasks are made");
    assert!(world.forge.issue(Item { repository: 0, number: 10 }).is_none(), "the ninth is refused, not made");
}

#[test]
fn a_change_whose_landing_waits_on_the_rules_stalls_at_its_deadline() {
    let (mut world, change, pull, head) = opened_change();
    // Only Bob approves, who may only read: the rules want a writer's
    // approval, which never comes.
    world.forge.readers.push(BOB).unwrap();
    let review = api::Review {
        id: 502,
        author: BOB,
        verdict: api::Verdict::Approve,
        commit: head,
        key: None,
        body: copy_of(b"ok"),
    };
    world.forge.change(pull).unwrap().reviews.push(review).unwrap();
    world.forge.issue(pull).unwrap().updated = world.env().now;
    world.deliver(Event::Hint { repository: 0, item: Some(pull.number), commit: None, branch: None });
    world.wait(30);
    assert!(world.forge.change(pull).unwrap().merged.is_none(), "nothing lands on a reader's approval");
    assert_eq!(held_for(&mut world, change), None, "it waits");
    world.wait(3600);
    let stalled = work::Hold::Plan { reason: translate::hold(plan::Hold::Stalled) };
    assert_eq!(held_for(&mut world, change), Some(stalled), "held as stalled at its deadline");
}

#[test]
fn a_run_the_rules_refuse_is_held_under_the_top_levels_own_code() {
    let (mut world, session) = World::session();
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: replied_answer() });
    // The deployment's runs have spent nearly all it may: the next turn
    // would spend past it.
    world.domain.spent = 99_500;
    message(&mut world, session, b"m1", 60);
    let refused = work::Hold::Plan { reason: translate::RUN_REFUSED };
    assert_eq!(held_for(&mut world, session), Some(refused), "held before it is claimed");
    assert_eq!(assigned(world.seen.as_slice(), session).as_slice(), [1], "and nothing runs");
}

#[test]
fn a_pull_request_retargeted_since_its_change_was_planned_is_not_merged() {
    let (mut world, change, pull, head) = opened_change();
    world.forge.change(pull).unwrap().base = copy_of(b"feat");
    let review = api::Review {
        id: 503,
        author: ALICE,
        verdict: api::Verdict::Approve,
        commit: head,
        key: None,
        body: copy_of(b"ok"),
    };
    world.forge.change(pull).unwrap().reviews.push(review).unwrap();
    world.forge.issue(pull).unwrap().updated = world.env().now;
    while world.domain.pop_fact().is_some() {}
    world.deliver(Event::Hint { repository: 0, item: Some(pull.number), commit: None, branch: None });
    world.wait(30);
    assert!(world.forge.change(pull).unwrap().merged.is_none(), "it lands nowhere it was not planned to");
    let mut refused = false;
    while let Some(fact) = world.domain.pop_fact() {
        refused |= fact == (crate::facts::Fact::Ruled { item: change, refused: true });
    }
    assert!(refused, "the rules refused the merge");
}

/// An agent step that may spend 400 tokens.
fn costly(name: &[u8]) -> plan::Step {
    let charter = plan::Charter {
        instructions: copy_of(b"look into it"),
        template: None,
        grants: plan::Grants { modify: false, shell: false, forge: true, subagents: false, note: false },
        budget: Budget { tokens: 400, turns: 5, time: Duration::from_secs(60) },
    };
    plan::Step { work: plan::Work::Agent(plan::AgentSpec { charter, grows: false }), ..task(name) }
}

#[test]
fn tasks_spending_past_what_a_plan_may_without_acceptance_wait_for_it() {
    let (mut world, session) = World::session();
    let tasks = crate::boundary::Outcome::Tasks {
        tasks: Box::new([costly(b"a"), costly(b"b"), costly(b"c")]),
        text: copy_of(b"on it"),
    };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    assert_eq!(held_for(&mut world, session), Some(work::Hold::Acceptance), "the rules want a person to accept them");
    assert!(world.forge.issue(Item { repository: 0, number: 2 }).is_none(), "and none is made before");
}

#[test]
fn a_released_session_takes_the_turn_it_never_had() {
    let (mut world, session) = World::session();
    for attempt in 1_u64..=3 {
        let failed = crate::boundary::Answer::Failed {
            failure: crate::boundary::Failure::Run,
            work: crate::boundary::Work { landed: Box::new([]) },
        };
        world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt, answer: failed });
        world.wait(10);
    }
    assert_eq!(held_for(&mut world, session), Some(work::Hold::Failures(work::Class::Run)), "held for its failures");
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(61)),
        person: ALICE,
        ask: Ask::Release { item: session },
    });
    world.wait(10);
    assert_eq!(assigned(world.seen.as_slice(), session).as_slice(), [1, 2, 3, 4], "released, it runs again");
}

/// A deployment whose plans landing on a protected branch an admin accepts.
fn admin_plans() -> Config {
    let mut config = config();
    config.rules.plan_acceptance = Permission::Admin;
    config
}

#[test]
fn what_an_admin_must_accept_a_writer_may_not_after_a_restart() {
    let mut world = World::configured(admin_plans);
    world.settle();
    let ask = Ask::Open { repository: 0, key: copy_of(b"k1"), title: copy_of(b"hi"), message: copy_of(b"hello") };
    world.deliver(Event::Ask { reply_to: ReplyTo::new(Token::new(9)), person: ALICE, ask });
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    world.deliver(Event::Hello { channel: Token::new(1), hello });
    let session = Item { repository: 0, number: 1 };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(proposal()) });
    assert_eq!(held_for(&mut world, session), Some(work::Hold::Acceptance), "it waits for an admin");
    world.restart();
    // Alice may only write.
    world.deliver(Event::Ask {
        reply_to: ReplyTo::new(Token::new(70)),
        person: ALICE,
        ask: Ask::Accept { item: session },
    });
    assert!(replied(&world.seen, Reply::Refused(Refusal::Unpermitted)), "a writer may not accept it: {:?}", world.seen);
    assert_eq!(held_for(&mut world, session), Some(work::Hold::Acceptance), "it still waits");
}

#[test]
fn a_change_whose_branch_was_deleted_is_made_again_from_its_base() {
    let (mut world, session) = World::session();
    let tasks = crate::boundary::Outcome::Tasks { tasks: Box::new([change_step(b"fix")]), text: copy_of(b"on it") };
    world.deliver(Event::Answer { channel: Token::new(1), item: session, attempt: 1, answer: ended(tasks) });
    let change = Item { repository: 0, number: 2 };
    // Its run pushes, and another party deletes the branch before its pull
    // request is opened.
    world.deliver(Event::Answer { channel: Token::new(1), item: change, attempt: 1, answer: changed([1; 32]) });
    world.wait(10);
    assert!(world.forge.change(Item { repository: 0, number: 3 }).is_none(), "no pull request for a branch gone");
    let starts = starts(&world.seen, change);
    let Some((attempt, start)) = starts.as_slice().last() else { panic!("the change runs") };
    assert_eq!(*attempt, 2, "it is made again: {:?}", starts.as_slice());
    let base = crate::boundary::Start::Base { branch: copy_of(b"main") };
    assert_eq!(*start, base, "from its base");
}
