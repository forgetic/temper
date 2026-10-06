//! Browser state and its step machine (web/architecture.md, sections 3–4).
//! Owns no browser, storage, protocol or presentation. Every external effect
//! is an ordered `Request`; callers reserve `max_out` slots and call `reclaim`
//! after routing all terminals in an iteration.
use crate::boundary::PersonSnapshot;
use crate::reads::ReadSlot;
use crate::requests::{Pending, Sending};
use crate::streams::{Following, Stream};
use crate::{
    Action, Address, Answer, Ask, Change, Event, Fact, Field, FieldRef, Form, Frame, Key, Limits, LinkState, Notice,
    NoticeKind, Offset, Outcome, Page, Query, ReadResult, Request, Saved, SavedDraft, SavedPending, Snapshot,
    StreamEnd, StreamEvent, Watch,
};
use alloc::boxed::Box;
use skein_lib::{Deadlines, Duration, Env, Id, List, Map, Queue, Rng, Slab, Time, Wall};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Timer {
    Retry(Id<Pending>),
    Reopen(Id<Stream>),
    Silent(Id<Stream>),
    Notice,
    Save,
}

/// The browser's bounded state; read through its shared-borrow accessors.
#[derive(Debug)]
pub struct Domain {
    started: bool,
    signed_out: bool,
    rng: Rng,
    wall: Wall,
    offset: Offset,
    shown: u64,
    address: Address,
    frame: Frame,
    link: LinkState,
    page: Page,
    generation: u32,
    streams: Slab<Stream>,
    frame_stream: Option<Id<Stream>>,
    reads: Slab<ReadSlot>,
    page_read: Option<Id<ReadSlot>>,
    requests: Slab<Pending>,
    pending: Map<Key, Id<Pending>>,
    timers: Deadlines<Timer>,
    new_chat: Field,
    notices: Queue<Notice>,
    facts: Queue<Fact>,
    lost: u64,
}

impl Domain {
    /// Create unstarted state from validated capacities and a deterministic seed.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Domain {
        assert!(crate::worst_case(limits).is_some(), "web limits are valid");
        let timer_capacity = limits.requests.saturating_add(limits.streams).saturating_add(2);
        Domain {
            started: false,
            signed_out: false,
            rng: Rng::new(seed),
            wall: Wall::EPOCH,
            offset: Offset(0),
            shown: 0,
            address: Address::Inbox,
            frame: Frame::new(limits.projects),
            link: LinkState::Starting,
            page: Page::Starting,
            generation: 0,
            streams: Slab::with_capacity(limits.streams),
            frame_stream: None,
            reads: Slab::with_capacity(limits.reads),
            page_read: None,
            requests: Slab::with_capacity(limits.requests),
            pending: Map::with_capacity(limits.requests),
            timers: Deadlines::with_capacity(timer_capacity),
            new_chat: Field::empty(),
            notices: Queue::with_capacity(limits.notices),
            facts: Queue::with_capacity(limits.facts),
            lost: 0,
        }
    }

    /// Earliest monotonic timer, without firing it.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.timers.next()
    }

    /// Reclaim terminal slots after the iteration's outputs have been routed.
    pub fn reclaim(&mut self) {
        self.streams.reclaim();
        self.reads.reclaim();
        self.requests.reclaim();
    }

    /// Pop one optional content-free diagnostic.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// Number of optional diagnostics dropped at their bound.
    #[must_use]
    pub const fn facts_lost(&self) -> u64 {
        self.lost
    }

    /// Changes to render-visible state.
    #[must_use]
    pub const fn shown(&self) -> u64 {
        self.shown
    }

    /// Last wall time seen in a step or fire.
    #[must_use]
    pub const fn wall(&self) -> Wall {
        self.wall
    }

    /// Browser offset from UTC.
    #[must_use]
    pub const fn offset(&self) -> Offset {
        self.offset
    }

    /// Whether live engine data is reaching the browser.
    #[must_use]
    pub const fn link(&self) -> LinkState {
        self.link
    }

    /// Shared navigation frame.
    #[must_use]
    pub const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Current page's plain data.
    #[must_use]
    pub const fn page(&self) -> &Page {
        &self.page
    }

    /// A field's text and domain-write count.
    #[must_use]
    pub const fn field(&self, field: FieldRef) -> Option<&Field> {
        match field {
            FieldRef::NewChat => Some(&self.new_chat),
        }
    }

    /// Current transient notices.
    #[must_use]
    pub const fn notices(&self) -> &Queue<Notice> {
        &self.notices
    }

    /// Current typed browser destination.
    #[must_use]
    pub const fn address(&self) -> Address {
        self.address
    }

    fn changed(&mut self) {
        self.shown = self.shown.wrapping_add(1);
    }

    fn fact(&mut self, fact: Fact) {
        if self.facts.try_push(fact).is_err() {
            self.lost = self.lost.saturating_add(1);
        }
    }

    fn notice(&mut self, env: &Env<Limits>, kind: NoticeKind) {
        let until = env.now.saturating_add(env.limits.notice);
        if self.notices.try_push(Notice { kind, until }).is_ok() {
            let first = self.notices.iter().next().expect("a pushed notice exists");
            self.arm(Timer::Notice, first.until);
            self.changed();
        }
    }

    fn arm(&mut self, timer: Timer, at: Time) {
        self.timers.arm(timer, at).expect("validated timer capacity holds");
    }

    fn save(&self, out: &mut Queue<Request>) {
        let mut pending = List::with_capacity(self.pending.capacity());
        for (key, id) in &self.pending {
            let request = self.requests.get(*id).expect("pending map names retained request");
            pending
                .push(SavedPending { key: *key, ask: request.ask.clone() })
                .expect("pending list matches map capacity");
        }
        out.push(Request::Save {
            saved: Saved {
                project: self.frame.project,
                drafts: Box::from([SavedDraft { field: FieldRef::NewChat, text: self.new_chat.text.clone() }]),
                pending: pending.into_boxed(),
            },
        });
    }

    fn fresh_key(&mut self) -> Key {
        loop {
            let high = u128::from(self.rng.next_u64()).wrapping_shl(64);
            let low = u128::from(self.rng.next_u64());
            let key = Key(high.wrapping_add(low).to_be_bytes());
            if !self.pending.contains_key(&key) {
                return key;
            }
        }
    }

    fn send(&mut self, id: Id<Pending>, attempt: u32, out: &mut Queue<Request>) {
        let pending = self.requests.get_mut(id).expect("send names a pending request");
        pending.state = Sending::InFlight { attempt };
        out.push(Request::Send { request: id.token(), key: pending.key, ask: pending.ask.clone() });
    }

    fn open_frame(&mut self, out: &mut Queue<Request>) {
        let id = self
            .streams
            .insert(Stream { watch: Watch::Person, state: Following::Opening })
            .expect("validated stream capacity includes frame");
        self.frame_stream = Some(id);
        out.push(Request::Open { stream: id.token(), watch: Watch::Person });
    }

    fn open_page(&mut self, env: &Env<Limits>, address: Address, out: &mut Queue<Request>) {
        self.address = address;
        self.generation = self.generation.checked_add(1).expect("page generation remains representable");
        let mut read_full = false;
        self.page = match address {
            Address::Inbox | Address::Chats => {
                let mut chats = crate::Chats::new(env.limits.window);
                let read = self.reads.insert(ReadSlot {
                    query: Query::Chats { project: self.frame.project, live: false, after: None },
                    generation: self.generation,
                    abandoned: false,
                });
                match read {
                    Ok(id) => {
                        self.page_read = Some(id);
                        out.push(Request::Read {
                            read: id.token(),
                            query: Query::Chats { project: self.frame.project, live: false, after: None },
                        });
                    }
                    Err(_) => {
                        self.page_read = None;
                        chats.loading = false;
                        read_full = true;
                    }
                }
                Page::Chats(chats)
            }
            Address::Task { .. } => Page::Missing { address },
        };
        if read_full {
            self.notice(env, NoticeKind::ReadFull);
        }
        self.changed();
    }

    fn go(&mut self, env: &Env<Limits>, address: Address, push: bool, out: &mut Queue<Request>) {
        let current = match self.page {
            Page::Starting | Page::SignIn { .. } => false,
            Page::Missing { .. } | Page::Chats(_) => true,
        };
        if self.address == address && self.started && current {
            return;
        }
        if let Some(id) = self.page_read.take() {
            self.reads.get_mut(id).expect("page read remains until its terminal").abandoned = true;
        }
        if self.signed_out {
            self.address = address;
            self.page = Page::SignIn { then: address };
            self.changed();
        } else {
            self.open_page(env, address, out);
        }
        if push {
            out.push(Request::Address { address, push: true });
        }
    }

    fn offline(&mut self, env: &Env<Limits>) {
        if !self.link.offline() {
            self.link = LinkState::Offline { since: env.wall };
            self.notice(env, NoticeKind::WentOffline);
            self.changed();
        }
    }

    fn disconnect(&mut self, env: &Env<Limits>, out: &mut Queue<Request>) {
        self.offline(env);
        if let Some(id) = self.frame_stream
            && let Some(stream) = self.streams.get_mut(id)
            && stream.state.accepts_event()
        {
            stream.state = Following::Reopening;
            self.timers.cancel(Timer::Silent(id));
            out.push(Request::Close { stream: id.token() });
        }
    }

    fn live(&mut self, env: &Env<Limits>, out: &mut Queue<Request>) {
        let was_offline = self.link.offline();
        if self.link != LinkState::Live {
            self.link = LinkState::Live;
            self.changed();
        }
        if was_offline {
            self.notice(env, NoticeKind::CameBack);
            let ids: List<Id<Pending>> = {
                let mut ids = List::with_capacity(self.pending.capacity());
                for (_, id) in &self.pending {
                    ids.push(*id).expect("pending ids fit");
                }
                ids
            };
            for id in &ids {
                let state = self.requests.get(*id).expect("pending id exists").state;
                if state.waiting() {
                    self.timers.cancel(Timer::Retry(*id));
                    self.send(*id, 1, out);
                }
            }
        }
    }

    fn sign_out(&mut self, env: &Env<Limits>, out: &mut Queue<Request>) {
        self.signed_out = true;
        if let Some(id) = self.page_read.take() {
            self.reads.get_mut(id).expect("page read remains until terminal").abandoned = true;
        }
        if let Some(id) = self.frame_stream
            && let Some(stream) = self.streams.get_mut(id)
            && stream.state != Following::Closing
        {
            stream.state = Following::Closing;
            out.push(Request::Close { stream: id.token() });
        }
        for (_, id) in &self.pending {
            let pending = self.requests.get_mut(*id).expect("pending id exists");
            match pending.state {
                Sending::InFlight { .. } | Sending::Parked => {}
                Sending::Backoff { .. } => {
                    pending.state = Sending::Parked;
                    self.timers.cancel(Timer::Retry(*id));
                }
            }
        }
        self.frame.person = None;
        self.frame.projects.clear();
        self.frame.project = None;
        self.frame.inbox_count = 0;
        self.page = Page::SignIn { then: self.address };
        self.offline(env);
        self.changed();
    }
}

/// Maximum number of outputs one step or one timer emits under these limits.
#[must_use]
pub fn max_out(limits: &Limits) -> u32 {
    limits.requests.saturating_add(limits.streams).saturating_add(5)
}

/// Apply one typed event and emit ordered shell operations.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    domain.wall = env.wall;
    assert!(domain.started || is_start(&event), "Start is the first event");
    match event {
        Event::Start { address, saved, offset } => start(domain, env, address, saved, offset, out),
        Event::Went { address } => domain.go(env, address, false, out),
        Event::Act { action } => act(domain, env, action, out),
        Event::Answered { request, answer } => answered(domain, env, Id::from_token(request), answer, out),
        Event::Read { read: token, result } => read(domain, env, Id::from_token(token), result, out),
        Event::Opened { stream } => opened(domain, env, Id::from_token(stream), out),
        Event::Streamed { stream, event } => streamed(domain, env, Id::from_token(stream), event, out),
        Event::Ended { stream, end } => ended(domain, env, Id::from_token(stream), end, out),
    }
}

fn is_start(event: &Event) -> bool {
    match event {
        Event::Start { .. } => true,
        Event::Went { .. }
        | Event::Act { .. }
        | Event::Answered { .. }
        | Event::Read { .. }
        | Event::Opened { .. }
        | Event::Streamed { .. }
        | Event::Ended { .. } => false,
    }
}

fn start(
    domain: &mut Domain,
    env: &Env<Limits>,
    address: Address,
    saved: Option<Saved>,
    offset: Offset,
    out: &mut Queue<Request>,
) {
    assert!(!domain.started, "Start is the first event, once");
    domain.started = true;
    domain.offset = offset;
    if let Some(saved) = saved {
        domain.frame.project = saved.project;
        for draft in &saved.drafts {
            match draft.field {
                FieldRef::NewChat => {
                    assert!(
                        draft.text.len() <= usize::try_from(env.limits.words).expect("u32 fits usize"),
                        "saved draft fits words bound"
                    );
                    domain.new_chat.text.clone_from(&draft.text);
                    domain.new_chat.written = domain.new_chat.written.wrapping_add(1);
                }
            }
        }
        for item in &saved.pending {
            let Ask::StartChat { words, .. } = &item.ask;
            assert!(
                words.len() <= usize::try_from(env.limits.words).expect("u32 fits usize"),
                "saved ask fits words bound"
            );
            let pending = Pending { key: item.key, ask: item.ask.clone(), state: Sending::Parked };
            let id = domain.requests.insert(pending).expect("saved pending count fits requests");
            let old = domain.pending.insert(item.key, id).expect("saved pending count fits requests");
            assert!(old.is_none(), "saved keys are unique");
            domain.send(id, 1, out);
        }
    }
    domain.open_frame(out);
    domain.open_page(env, address, out);
}

fn act(domain: &mut Domain, env: &Env<Limits>, action: Action, out: &mut Queue<Request>) {
    match action {
        Action::Go { address } => domain.go(env, address, true, out),
        Action::Edit { field: FieldRef::NewChat, text } => {
            if let Page::Chats(_) = domain.page {
            } else {
                return;
            }
            if text.len() > usize::try_from(env.limits.words).expect("u32 fits usize") {
                domain.notice(env, NoticeKind::WordsTooLong);
            } else {
                domain.new_chat.text = text;
                domain.arm(Timer::Save, env.now.saturating_add(env.limits.save));
            }
        }
        Action::Submit { form: Form::NewChat } => submit_chat(domain, env, out),
        Action::SignIn => out.push(Request::SignIn { then: domain.address }),
    }
}

fn submit_chat(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let chats = match domain.page {
        Page::Chats(_) => true,
        Page::Starting | Page::SignIn { .. } | Page::Missing { .. } => false,
    };
    if domain.signed_out || !chats {
        return;
    }
    let Some(project) = domain.frame.project else {
        return;
    };
    if domain.new_chat.text.is_empty() {
        return;
    }
    for (_, id) in &domain.pending {
        let request = domain.requests.get(*id).expect("pending id exists");
        match &request.ask {
            Ask::StartChat { project: asked_project, words } => {
                if *asked_project == project && words.as_ref() == domain.new_chat.text.as_ref() {
                    return;
                }
            }
        }
    }
    let key = domain.fresh_key();
    let pending =
        Pending { key, ask: Ask::StartChat { project, words: domain.new_chat.text.clone() }, state: Sending::Parked };
    let Ok(id) = domain.requests.insert(pending) else {
        domain.notice(env, NoticeKind::RequestFull);
        return;
    };
    domain.pending.insert(key, id).expect("pending map matches slab capacity");
    domain.save(out);
    if !domain.link.offline() {
        domain.send(id, 1, out);
    }
    domain.changed();
}

fn answered(domain: &mut Domain, env: &Env<Limits>, id: Id<Pending>, answer: Answer, out: &mut Queue<Request>) {
    let pending = domain.requests.get(id).expect("Answered names a live Send");
    assert!(pending.state.in_flight(), "Answered closes one Send");
    let key = pending.key;
    let ask = pending.ask.clone();
    let attempt = match pending.state {
        Sending::InFlight { attempt } => attempt,
        Sending::Backoff { .. } | Sending::Parked => unreachable!("checked in-flight state"),
    };
    match answer {
        Answer::Busy => {
            domain.requests.get_mut(id).expect("request exists").state = Sending::Backoff { attempt };
            let delay = backoff(&mut domain.rng, &env.limits, attempt);
            domain.arm(Timer::Retry(id), env.now.saturating_add(delay));
        }
        Answer::Unreachable => {
            domain.requests.get_mut(id).expect("request exists").state = Sending::Parked;
            domain.disconnect(env, out);
        }
        Answer::SignedOut => {
            domain.requests.get_mut(id).expect("request exists").state = Sending::Parked;
            domain.sign_out(env, out);
        }
        Answer::Done(outcome) => {
            domain.pending.remove(&key).expect("answered key is pending");
            domain.requests.retire(id);
            match outcome {
                Outcome::Started { task } => {
                    let Ask::StartChat { words, .. } = ask;
                    if domain.new_chat.text.as_ref() == words.as_ref() {
                        domain.new_chat.text = Box::from([]);
                        domain.new_chat.written = domain.new_chat.written.wrapping_add(1);
                    }
                    domain.timers.cancel(Timer::Save);
                    domain.save(out);
                    domain.go(env, Address::Task { number: task, section: None }, true, out);
                }
            }
            domain.changed();
        }
        Answer::Refused(refusal) => {
            domain.pending.remove(&key).expect("refused key is pending");
            domain.requests.retire(id);
            domain.save(out);
            if refusal == crate::Refusal::KeyConflict {
                domain.fact(Fact::KeyConflict);
            }
            domain.notice(env, NoticeKind::Refused(refusal));
            domain.changed();
        }
    }
}

fn read(domain: &mut Domain, env: &Env<Limits>, id: Id<ReadSlot>, result: ReadResult, out: &mut Queue<Request>) {
    let slot = domain.reads.get(id).expect("Read names a live read");
    match slot.query {
        Query::Chats { .. } => {}
    }
    let active = !slot.abandoned && slot.generation == domain.generation;
    domain.reads.retire(id);
    if domain.page_read == Some(id) {
        domain.page_read = None;
    }
    if !active {
        return;
    }
    if let Page::Chats(chats) = &mut domain.page {
        chats.loading = false;
    }
    match result {
        ReadResult::Chats { rows, older } => {
            if let Page::Chats(chats) = &mut domain.page {
                assert!(
                    rows.len() <= usize::try_from(env.limits.window).expect("u32 fits usize"),
                    "chat read fits window"
                );
                chats.rows.clear();
                for row in rows {
                    assert!(
                        row.title.len() <= usize::try_from(env.limits.text).expect("u32 fits usize"),
                        "chat title fits text bound"
                    );
                    chats.rows.push(row).expect("read rows fit window");
                }
                chats.older = older;
                chats.loading = false;
                domain.changed();
            }
        }
        ReadResult::SignedOut => domain.sign_out(env, out),
        ReadResult::Unreachable => domain.disconnect(env, out),
        ReadResult::Refused(refusal) => domain.notice(env, NoticeKind::Refused(refusal)),
    }
}

fn opened(domain: &mut Domain, env: &Env<Limits>, id: Id<Stream>, _out: &mut Queue<Request>) {
    let stream = domain.streams.get_mut(id).expect("Opened names a live watch");
    match stream.state {
        Following::Opening => {
            stream.state = Following::Waiting;
            domain.arm(Timer::Silent(id), env.now.saturating_add(env.limits.heartbeat));
        }
        Following::Closing | Following::Reopening => {}
        Following::Waiting | Following::Live | Following::Backoff { .. } => unreachable!("Opened follows one Open"),
    }
}

fn apply_person(domain: &mut Domain, limits: &Limits, snapshot: PersonSnapshot) {
    assert!(
        snapshot.projects.len() <= usize::try_from(domain.frame.projects.capacity()).expect("u32 fits usize"),
        "person snapshot projects fit"
    );
    let text = usize::try_from(limits.text).expect("u32 fits usize");
    assert!(snapshot.person.name.len() <= text, "person name fits text bound");
    for project in &snapshot.projects {
        assert!(project.name.len() <= text, "project name fits text bound");
    }
    domain.frame.person = Some(snapshot.person);
    domain.frame.projects.clear();
    for project in snapshot.projects {
        domain.frame.projects.push(project).expect("projects fit");
    }
    let mut project_held = false;
    if let Some(chosen) = domain.frame.project {
        for project in &domain.frame.projects {
            if project.number == chosen {
                project_held = true;
                break;
            }
        }
    }
    if !project_held {
        if let Some(project) = domain.frame.projects.get(0) {
            domain.frame.project = Some(project.number);
        } else {
            domain.frame.project = None;
        }
    }
    domain.frame.inbox_count = snapshot.inbox_count;
    domain.signed_out = false;
    domain.changed();
}

fn streamed(domain: &mut Domain, env: &Env<Limits>, id: Id<Stream>, event: StreamEvent, out: &mut Queue<Request>) {
    let stream = domain.streams.get(id).expect("Streamed names a live watch");
    match stream.state {
        Following::Closing | Following::Reopening => return,
        Following::Waiting | Following::Live => {}
        Following::Opening | Following::Backoff { .. } => unreachable!("events require an opened watch"),
    }
    domain.timers.cancel(Timer::Silent(id));
    match event {
        StreamEvent::Snapshot(Snapshot::Person(snapshot)) => {
            domain.streams.get_mut(id).expect("stream exists").state = Following::Live;
            apply_person(domain, &env.limits, snapshot);
            domain.live(env, out);
        }
        StreamEvent::Change(Change::Person(snapshot)) => {
            assert!(stream.state == Following::Live, "change follows snapshot");
            apply_person(domain, &env.limits, snapshot);
        }
        StreamEvent::Missed { .. } => {
            domain.streams.get_mut(id).expect("stream exists").state = Following::Reopening;
            domain.link = LinkState::Behind;
            domain.changed();
            out.push(Request::Close { stream: id.token() });
            return;
        }
        StreamEvent::Alive => {}
    }
    domain.arm(Timer::Silent(id), env.now.saturating_add(env.limits.heartbeat));
}

fn ended(domain: &mut Domain, env: &Env<Limits>, id: Id<Stream>, end: StreamEnd, out: &mut Queue<Request>) {
    let stream = domain.streams.get(id).expect("Ended names a live watch");
    let state = stream.state;
    domain.timers.cancel(Timer::Silent(id));
    match state {
        Following::Closing => {
            domain.streams.retire(id);
            if domain.frame_stream == Some(id) {
                domain.frame_stream = None;
            }
        }
        Following::Reopening => {
            if end == StreamEnd::SignedOut {
                domain.streams.retire(id);
                if domain.frame_stream == Some(id) {
                    domain.frame_stream = None;
                }
                domain.sign_out(env, out);
                return;
            }
            assert!(end == StreamEnd::Closed, "reopening waits for Close terminal");
            if domain.link.offline() {
                domain.streams.get_mut(id).expect("stream exists").state = Following::Backoff { attempt: 1 };
                let delay = backoff(&mut domain.rng, &env.limits, 1);
                domain.arm(Timer::Reopen(id), env.now.saturating_add(delay));
            } else {
                domain.streams.get_mut(id).expect("stream exists").state = Following::Opening;
                out.push(Request::Open { stream: id.token(), watch: Watch::Person });
            }
        }
        Following::Opening | Following::Waiting | Following::Live => match end {
            StreamEnd::SignedOut => {
                domain.streams.retire(id);
                if domain.frame_stream == Some(id) {
                    domain.frame_stream = None;
                }
                domain.sign_out(env, out);
            }
            StreamEnd::Dropped | StreamEnd::Refused(_) | StreamEnd::Gone | StreamEnd::Closed => {
                let attempt = 1;
                domain.streams.get_mut(id).expect("stream exists").state = Following::Backoff { attempt };
                domain.offline(env);
                let delay = backoff(&mut domain.rng, &env.limits, attempt);
                domain.arm(Timer::Reopen(id), env.now.saturating_add(delay));
            }
        },
        Following::Backoff { .. } => unreachable!("a terminal cannot follow its prior terminal"),
    }
}

fn backoff(rng: &mut Rng, limits: &Limits, attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(31);
    let factor = 1_u64.checked_shl(shift).expect("shift is at most 31");
    let cap = limits.backoff.first.saturating_mul(factor).as_nanos().min(limits.backoff.most.as_nanos());
    let upper = cap.saturating_add(cap / 4).min(limits.backoff.most.as_nanos());
    Duration::from_nanos(rng.between(cap, upper))
}

/// Fire at most one due monotonic timer.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    domain.wall = env.wall;
    let Some(timer) = domain.timers.expire(env.now) else {
        return;
    };
    match timer {
        Timer::Retry(id) => {
            if let Some(pending) = domain.requests.get(id)
                && let Sending::Backoff { attempt } = pending.state
            {
                if !domain.link.offline() && !domain.signed_out {
                    domain.send(id, attempt.saturating_add(1), out);
                } else {
                    domain.requests.get_mut(id).expect("request exists").state = Sending::Parked;
                }
            }
        }
        Timer::Reopen(id) => {
            if let Some(stream) = domain.streams.get_mut(id)
                && stream.state.backoff()
            {
                stream.state = Following::Opening;
                out.push(Request::Open { stream: id.token(), watch: stream.watch });
            }
        }
        Timer::Silent(id) => {
            if let Some(stream) = domain.streams.get_mut(id)
                && stream.state.accepts_event()
            {
                stream.state = Following::Reopening;
                domain.offline(env);
                out.push(Request::Close { stream: id.token() });
            }
        }
        Timer::Notice => {
            while match domain.notices.iter().next() {
                Some(notice) => notice.until <= env.now,
                None => false,
            } {
                domain.notices.pop();
                domain.changed();
            }
            if let Some(notice) = domain.notices.iter().next() {
                domain.arm(Timer::Notice, notice.until);
            }
        }
        Timer::Save => domain.save(out),
    }
}
