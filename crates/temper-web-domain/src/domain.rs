//! Browser state and its step machine (web/architecture.md, sections 3–4).
//! Owns no browser, storage, protocol or presentation. Every external effect
//! is an ordered `Request`; callers reserve `max_out` slots and call `reclaim`
//! after routing all terminals in an iteration.
use crate::boundary::PersonSnapshot;
use crate::objects::Objects;
use crate::reads::ReadSlot;
use crate::requests::{Pending, Sending};
use crate::streams::{Following, Owner, Stream};
use crate::{
    Action, Address, Answer, Ask, Body, Card, Change, Confirming, Decision, Event, Fact, Field, FieldRef, Form, Frame,
    Intent, Key, Limits, LinkState, Notice, NoticeKind, Object, ObjectKey, Offset, Outcome, Page, Problem, Query,
    ReadResult, Request, Saved, SavedDraft, SavedPending, Snapshot, StreamEnd, StreamEvent, TaskSnapshot, Waiting,
    Watch, Why,
};
use alloc::boxed::Box;
use skein_lib::{Deadlines, Duration, Env, Id, List, Map, Queue, Rng, Slab, Time, Wall};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Timer {
    Retry(Id<Pending>),
    Reopen(Id<Stream>),
    Silent(Id<Stream>),
    Linger(Id<Object>),
    Notice,
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
    page_stream: Option<Id<Stream>>,
    reads: Slab<ReadSlot>,
    page_read: Option<Id<ReadSlot>>,
    requests: Slab<Pending>,
    pending: Map<Key, Id<Pending>>,
    objects: Objects,
    confirming: Option<Confirming>,
    timers: Deadlines<Timer>,
    new_chat: Field,
    reason: Field,
    notices: Queue<Notice>,
    facts: Queue<Fact>,
    lost: u64,
}

impl Domain {
    /// Create unstarted state from validated capacities and a deterministic seed.
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Domain {
        assert!(crate::worst_case(limits).is_some(), "web limits are valid");
        let timer_capacity =
            limits.requests.saturating_add(limits.streams).saturating_add(limits.objects).saturating_add(2);
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
            page_stream: None,
            reads: Slab::with_capacity(limits.reads),
            page_read: None,
            requests: Slab::with_capacity(limits.requests),
            pending: Map::with_capacity(limits.requests),
            objects: Objects::new(limits.objects),
            confirming: None,
            timers: Deadlines::with_capacity(timer_capacity),
            new_chat: Field::empty(),
            reason: Field::empty(),
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
        self.objects.slab.reclaim();
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
            FieldRef::Reason => Some(&self.reason),
        }
    }

    /// One current page object, if the handle remains valid.
    #[must_use]
    pub fn object(&self, id: Id<Object>) -> Option<&Object> {
        self.objects.get(id)
    }

    /// The one open confirmation, if any.
    #[must_use]
    pub const fn confirming(&self) -> Option<&Confirming> {
        self.confirming.as_ref()
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
                drafts: Box::from([
                    SavedDraft { field: FieldRef::NewChat, text: self.new_chat.text.clone() },
                    SavedDraft { field: FieldRef::Reason, text: self.reason.text.clone() },
                ]),
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
            .insert(Stream { watch: Watch::Person, owner: Owner::Frame, state: Following::Opening })
            .expect("validated stream capacity includes frame");
        self.frame_stream = Some(id);
        out.push(Request::Open { stream: id.token(), watch: Watch::Person });
    }

    fn open_page(&mut self, env: &Env<Limits>, address: Address, out: &mut Queue<Request>) {
        self.address = address;
        self.generation = self.generation.checked_add(1).expect("page generation remains representable");
        let mut read_full = false;
        let mut stream_full = false;
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
            Address::Task { number, .. } => {
                let mut task = crate::TaskPage::new(number);
                let watch = Watch::Task { number };
                match self.streams.insert(Stream {
                    watch,
                    owner: Owner::Page { generation: self.generation },
                    state: Following::Opening,
                }) {
                    Ok(id) => {
                        self.page_stream = Some(id);
                        out.push(Request::Open { stream: id.token(), watch });
                    }
                    Err(_) => {
                        task.loading = false;
                        stream_full = true;
                    }
                }
                Page::Task(task)
            }
        };
        if read_full {
            self.notice(env, NoticeKind::ReadFull);
        }
        if stream_full {
            self.notice(env, NoticeKind::StreamFull);
        }
        if self.link != LinkState::Starting && !self.link.offline() {
            let frame_live = match self.frame_stream {
                Some(id) => self.watch_live(id),
                None => false,
            };
            let needs_task_watch = match self.page {
                Page::Task(_) => true,
                Page::Starting | Page::SignIn { .. } | Page::Missing { .. } | Page::Chats(_) => false,
            };
            self.link = if frame_live && !needs_task_watch { LinkState::Live } else { LinkState::Behind };
        }
        self.changed();
    }

    fn go(&mut self, env: &Env<Limits>, address: Address, push: bool, out: &mut Queue<Request>) {
        let current = match self.page {
            Page::Starting | Page::SignIn { .. } => false,
            Page::Missing { .. } | Page::Chats(_) | Page::Task(_) => true,
        };
        if self.address == address && self.started && current {
            return;
        }
        if let Some(id) = self.page_read.take() {
            self.reads.get_mut(id).expect("page read remains until its terminal").abandoned = true;
        }
        self.close_page_stream(out);
        self.objects.clear();
        self.confirming = None;
        let had_reason = !self.reason.text.is_empty();
        self.clear_reason();
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
        if had_reason {
            self.save(out);
        }
    }

    fn close_page_stream(&mut self, out: &mut Queue<Request>) {
        if let Some(id) = self.page_stream.take() {
            let state = self.streams.get(id).expect("page watch exists").state;
            self.timers.cancel(Timer::Silent(id));
            match state {
                Following::Opening | Following::Waiting | Following::Live => {
                    self.streams.get_mut(id).expect("page watch exists").state = Following::Closing;
                    out.push(Request::Close { stream: id.token() });
                }
                Following::Reopening => {
                    self.streams.get_mut(id).expect("page watch exists").state = Following::Closing;
                }
                Following::Backoff { .. } => {
                    self.timers.cancel(Timer::Reopen(id));
                    self.streams.retire(id);
                    self.streams.reclaim();
                }
                Following::Closing => {}
            }
        }
    }

    fn open_pending_task_watch(&mut self, out: &mut Queue<Request>) {
        if self.signed_out || self.page_stream.is_some() {
            return;
        }
        let Page::Task(page) = &mut self.page else {
            return;
        };
        let watch = Watch::Task { number: page.number };
        if let Ok(id) = self.streams.insert(Stream {
            watch,
            owner: Owner::Page { generation: self.generation },
            state: Following::Opening,
        }) {
            self.page_stream = Some(id);
            page.loading = true;
            out.push(Request::Open { stream: id.token(), watch });
            self.changed();
        }
    }

    fn clear_reason(&mut self) {
        if !self.reason.text.is_empty() {
            self.reason.text = Box::from([]);
            self.reason.written = self.reason.written.wrapping_add(1);
        }
    }

    fn leave_object(&mut self, env: &Env<Limits>, key: ObjectKey, why: Why) {
        if let Some(id) = self.objects.find(key) {
            let until = env.now.saturating_add(env.limits.linger);
            let object = self.objects.get_mut(id).expect("indexed object exists");
            object.card = Card::Leaving { why, until };
            self.arm(Timer::Linger(id), until);
            self.changed();
        } else {
            self.notice(env, NoticeKind::Decided);
        }
    }

    fn mark_deciding(&mut self, id: Id<Object>, key: ObjectKey) {
        for (_, pending_id) in &self.pending {
            let pending = self.requests.get(*pending_id).expect("pending request exists");
            if pending.about == Some(key) {
                self.objects.get_mut(id).expect("object exists").card = Card::Deciding { request: pending_id.token() };
                return;
            }
        }
    }

    fn task_snapshot(&mut self, env: &Env<Limits>, snapshot: TaskSnapshot) {
        let Page::Task(page) = &self.page else {
            return;
        };
        if page.number != snapshot.chip.task {
            return;
        }
        let text_bound = usize::try_from(env.limits.text).expect("u32 fits usize");
        assert!(snapshot.chip.title.len() <= text_bound, "task title fits text bound");
        assert!(snapshot.first_words.len() <= text_bound, "opening words fit text bound");
        let had_escalation = snapshot.escalation.is_some();
        let had_result = snapshot.result.is_some();
        let task_number = snapshot.chip.task;
        self.objects.clear_source(1);
        let chip_key = ObjectKey::Task(snapshot.chip.task);
        let chip = self.objects.put(chip_key, snapshot.chip.revision, Body::Chip(snapshot.chip), 1);
        let escalation = match snapshot.escalation {
            Some(escalation) => {
                let key = ObjectKey::Escalation { task: escalation.task };
                let id = self.objects.put(key, escalation.revision, Body::Escalation(escalation), 1);
                if let Some(id) = id {
                    self.mark_deciding(id, key);
                }
                id
            }
            None => match self.objects.find(ObjectKey::Escalation { task: task_number }) {
                Some(id) => {
                    let object = self.objects.get(id).expect("indexed object exists");
                    match object.card {
                        Card::Deciding { .. } | Card::Leaving { .. } => Some(id),
                        Card::Open | Card::Refused { .. } => None,
                    }
                }
                None => None,
            },
        };
        let result = match snapshot.result {
            Some(result) => {
                assert!(result.words.len() <= text_bound, "task result fits text bound");
                self.objects.put(ObjectKey::Result { task: result.task }, result.revision, Body::Ended(result), 1)
            }
            None => None,
        };
        self.objects.prune();
        if chip.is_none() || (had_escalation && escalation.is_none()) || (had_result && result.is_none()) {
            self.notice(env, NoticeKind::ObjectFull);
        }
        if let Page::Task(page) = &mut self.page {
            page.chip = chip;
            page.escalation = escalation;
            page.result = result;
            page.first_words = Some(snapshot.first_words);
            page.loading = false;
        }
        if let Some(open) = &mut self.confirming {
            if let Some(object) = self.objects.get(open.object) {
                if object.revision != open.revision {
                    open.changed = true;
                    open.problem = Some(Problem::Changed);
                }
            } else {
                open.changed = true;
                open.problem = Some(Problem::Changed);
            }
        }
        self.changed();
    }

    fn read_task(&mut self, env: &Env<Limits>, query: Query, out: &mut Queue<Request>) {
        match self.reads.insert(ReadSlot { query: query.clone(), generation: self.generation, abandoned: false }) {
            Ok(id) => {
                self.page_read = Some(id);
                out.push(Request::Read { read: id.token(), query });
            }
            Err(_) => {
                if let Page::Task(page) = &mut self.page {
                    page.loading = false;
                }
                self.notice(env, NoticeKind::ReadFull);
            }
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
        if let Some(id) = self.page_stream
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
        let frame_live = match self.frame_stream {
            Some(id) => self.watch_live(id),
            None => false,
        };
        let page_live = match (&self.page, self.page_stream) {
            (Page::Task(_), None) => false,
            (_, Some(id)) => self.watch_live(id),
            (_, None) => true,
        };
        let next = if frame_live && page_live { LinkState::Live } else { LinkState::Behind };
        if self.link != next {
            self.link = next;
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
            if let Page::Task(page) = &mut self.page
                && let Some(query) = page.retry.take()
            {
                self.read_task(env, query, out);
            }
        }
    }

    fn watch_live(&self, id: Id<Stream>) -> bool {
        match self.streams.get(id) {
            Some(stream) => stream.state == Following::Live,
            None => false,
        }
    }

    fn sign_out(&mut self, env: &Env<Limits>, out: &mut Queue<Request>) {
        self.signed_out = true;
        if let Some(id) = self.page_read.take() {
            self.reads.get_mut(id).expect("page read remains until terminal").abandoned = true;
        }
        self.close_page_stream(out);
        self.objects.clear();
        self.confirming = None;
        self.clear_reason();
        if let Some(id) = self.frame_stream {
            let state = self.streams.get(id).expect("frame watch exists").state;
            match state {
                Following::Opening | Following::Waiting | Following::Live => {
                    self.streams.get_mut(id).expect("frame watch exists").state = Following::Closing;
                    self.timers.cancel(Timer::Silent(id));
                    out.push(Request::Close { stream: id.token() });
                }
                Following::Reopening => {
                    self.streams.get_mut(id).expect("frame watch exists").state = Following::Closing;
                }
                Following::Backoff { .. } => {
                    self.timers.cancel(Timer::Reopen(id));
                    self.streams.retire(id);
                    self.frame_stream = None;
                }
                Following::Closing => {}
            }
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
                FieldRef::Reason => {
                    assert!(
                        draft.text.len() <= usize::try_from(env.limits.words).expect("u32 fits usize"),
                        "saved reason fits words bound"
                    );
                    domain.reason.text.clone_from(&draft.text);
                    domain.reason.written = domain.reason.written.wrapping_add(1);
                }
            }
        }
        for item in &saved.pending {
            let about = match &item.ask {
                Ask::StartChat { words, .. } => {
                    assert!(
                        words.len() <= usize::try_from(env.limits.words).expect("u32 fits usize"),
                        "saved ask fits words bound"
                    );
                    None
                }
                Ask::Decide { waiting: Waiting::Escalation { task }, decision, .. } => {
                    if let Decision::Reject { reason } = decision {
                        assert!(
                            reason.len() <= usize::try_from(env.limits.words).expect("u32 fits usize"),
                            "saved reason fits words bound"
                        );
                    }
                    Some(ObjectKey::Escalation { task: *task })
                }
            };
            let pending = Pending { key: item.key, ask: item.ask.clone(), about, state: Sending::Parked };
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
                domain.save(out);
            }
        }
        Action::Edit { field: FieldRef::Reason, text } => {
            if domain.confirming.is_none() {
                return;
            }
            if text.len() > usize::try_from(env.limits.words).expect("u32 fits usize") {
                domain.notice(env, NoticeKind::WordsTooLong);
            } else {
                domain.reason.text = text;
                domain.save(out);
            }
        }
        Action::Submit { form: Form::NewChat } => submit_chat(domain, env, out),
        Action::Intend { intent, object } => intend(domain, env, intent, object, out),
        Action::Confirm => confirm(domain, env, out),
        Action::Dismiss => {
            domain.confirming = None;
            domain.clear_reason();
            domain.save(out);
            domain.changed();
        }
        Action::SignIn => out.push(Request::SignIn { then: domain.address }),
    }
}

fn offered(object: &Object, intent: Intent) -> bool {
    match &object.body {
        Body::Escalation(escalation) => match intent {
            Intent::Release => escalation.offers.release,
            Intent::LeaveHeld => escalation.offers.leave_held,
            Intent::PassUp => escalation.offers.pass_up,
        },
        Body::Chip(_) | Body::Ended(_) => false,
    }
}

fn intend(domain: &mut Domain, env: &Env<Limits>, intent: Intent, id: Id<Object>, out: &mut Queue<Request>) {
    let Some(object) = domain.objects.get(id) else {
        domain.notice(env, NoticeKind::StaleObject);
        return;
    };
    if !offered(object, intent) {
        domain.notice(env, NoticeKind::StaleObject);
        return;
    }
    match object.card {
        Card::Open | Card::Refused { .. } => {}
        Card::Deciding { .. } | Card::Leaving { .. } => return,
    }
    domain.confirming =
        Some(Confirming { intent, object: id, revision: object.revision, changed: false, problem: None });
    let had_reason = !domain.reason.text.is_empty();
    domain.clear_reason();
    if had_reason {
        domain.save(out);
    }
    domain.changed();
}

fn confirm(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(open) = &domain.confirming else {
        return;
    };
    let id = open.object;
    let revision = open.revision;
    let intent = open.intent;
    let object_key = match domain.objects.get(id) {
        Some(object) => object.key,
        None => {
            domain.notice(env, NoticeKind::StaleObject);
            return;
        }
    };
    let Some(object) = domain.objects.get(id) else {
        domain.notice(env, NoticeKind::StaleObject);
        return;
    };
    if open.changed || object.revision != open.revision {
        domain.confirming.as_mut().expect("confirmation exists").problem = Some(Problem::Changed);
        domain.changed();
        return;
    }
    if !offered(object, open.intent) {
        domain.confirming.as_mut().expect("confirmation exists").problem = Some(Problem::NotOffered);
        domain.changed();
        return;
    }
    let decision = match intent {
        Intent::Release => Decision::Release,
        Intent::PassUp => Decision::Pass,
        Intent::LeaveHeld => {
            if domain.reason.text.is_empty() {
                domain.confirming.as_mut().expect("confirmation exists").problem = Some(Problem::ReasonMissing);
                domain.changed();
                return;
            }
            Decision::Reject { reason: domain.reason.text.clone() }
        }
    };
    let ObjectKey::Escalation { task } = object_key else { unreachable!("only escalation has W4 offers") };
    for (_, pending_id) in &domain.pending {
        let pending = domain.requests.get(*pending_id).expect("pending request exists");
        if pending.about == Some(object_key) {
            return;
        }
    }
    let key = domain.fresh_key();
    let pending = Pending {
        key,
        ask: Ask::Decide { waiting: Waiting::Escalation { task }, revision, decision },
        about: Some(object_key),
        state: Sending::Parked,
    };
    let Ok(request) = domain.requests.insert(pending) else {
        domain.notice(env, NoticeKind::RequestFull);
        return;
    };
    domain.pending.insert(key, request).expect("pending map matches slab capacity");
    domain.objects.get_mut(id).expect("confirmed object exists").card = Card::Deciding { request: request.token() };
    domain.confirming = None;
    domain.clear_reason();
    domain.save(out);
    if !domain.link.offline() && !domain.signed_out {
        domain.send(request, 1, out);
    }
    domain.changed();
}

fn submit_chat(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let chats = match domain.page {
        Page::Chats(_) => true,
        Page::Starting | Page::SignIn { .. } | Page::Missing { .. } | Page::Task(_) => false,
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
            Ask::Decide { .. } => {}
        }
    }
    let key = domain.fresh_key();
    let pending = Pending {
        key,
        ask: Ask::StartChat { project, words: domain.new_chat.text.clone() },
        about: None,
        state: Sending::Parked,
    };
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
    let about = pending.about;
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
                    let Ask::StartChat { words, .. } = ask else { unreachable!("Started answers StartChat") };
                    if domain.new_chat.text.as_ref() == words.as_ref() {
                        domain.new_chat.text = Box::from([]);
                        domain.new_chat.written = domain.new_chat.written.wrapping_add(1);
                    }
                    domain.save(out);
                    domain.go(env, Address::Task { number: task, section: None }, true, out);
                }
                Outcome::Decided { choice } => {
                    domain.save(out);
                    if let Some(key) = about {
                        if let Some(person) = &domain.frame.person {
                            domain.leave_object(env, key, Why::Decided { by: person.clone(), choice, at: env.wall });
                        } else {
                            domain.notice(env, NoticeKind::Decided);
                        }
                    }
                }
                Outcome::DecidedBefore { by, choice, at } => {
                    domain.save(out);
                    if let Some(key) = about {
                        domain.leave_object(env, key, Why::Decided { by, choice, at });
                    }
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
            if let Some(key) = about {
                if let Some(object_id) = domain.objects.find(key) {
                    domain.objects.get_mut(object_id).expect("indexed object exists").card = Card::Refused { refusal };
                } else {
                    domain.notice(env, NoticeKind::Refused(refusal));
                }
            } else {
                domain.notice(env, NoticeKind::Refused(refusal));
            }
            domain.changed();
        }
    }
}

fn read(domain: &mut Domain, env: &Env<Limits>, id: Id<ReadSlot>, result: ReadResult, out: &mut Queue<Request>) {
    let slot = domain.reads.get(id).expect("Read names a live read");
    let query = slot.query.clone();
    let active = !slot.abandoned && slot.generation == domain.generation;
    domain.reads.retire(id);
    if domain.page_read == Some(id) {
        domain.page_read = None;
    }
    if !active {
        return;
    }
    match (query, result) {
        (Query::Chats { .. }, ReadResult::Chats { rows, older }) => {
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
        (Query::Escalation { task }, ReadResult::Escalation(value)) => {
            if let Page::Task(page) = &domain.page
                && page.number == task
            {
                domain.objects.clear_source(2);
                let had_value = value.is_some();
                let object = match value {
                    Some(escalation) => {
                        assert!(escalation.task == task, "escalation read names requested task");
                        let key = ObjectKey::Escalation { task };
                        let id = domain.objects.put(key, escalation.revision, Body::Escalation(escalation), 2);
                        if let Some(id) = id {
                            domain.mark_deciding(id, key);
                        }
                        id
                    }
                    None => None,
                };
                domain.objects.prune();
                if had_value && object.is_none() {
                    domain.notice(env, NoticeKind::ObjectFull);
                }
                if let Page::Task(page) = &mut domain.page {
                    page.escalation = object;
                }
                domain.changed();
                domain.read_task(env, Query::Result { task }, out);
            }
        }
        (Query::Result { task }, ReadResult::Result(value)) => {
            if let Page::Task(page) = &domain.page
                && page.number == task
            {
                domain.objects.clear_source(4);
                let object = match value {
                    Some(result) => {
                        assert!(result.task == task, "result read names requested task");
                        assert!(
                            result.words.len() <= usize::try_from(env.limits.text).expect("u32 fits usize"),
                            "result fits text bound"
                        );
                        domain.objects.put(ObjectKey::Result { task }, result.revision, Body::Ended(result), 4)
                    }
                    None => None,
                };
                domain.objects.prune();
                if let Page::Task(page) = &mut domain.page {
                    page.result = object;
                    page.loading = false;
                }
                domain.changed();
            }
        }
        (_, ReadResult::SignedOut) => domain.sign_out(env, out),
        (query, ReadResult::Unreachable) => {
            if let Page::Task(page) = &mut domain.page {
                page.retry = Some(query);
            }
            domain.disconnect(env, out);
        }
        (Query::Escalation { task }, ReadResult::Refused(refusal)) => {
            domain.notice(env, NoticeKind::Refused(refusal));
            domain.read_task(env, Query::Result { task }, out);
        }
        (_, ReadResult::Refused(refusal)) => domain.notice(env, NoticeKind::Refused(refusal)),
        (Query::Chats { .. }, ReadResult::Escalation(_) | ReadResult::Result(_))
        | (Query::Escalation { .. } | Query::Result { .. }, ReadResult::Chats { .. })
        | (Query::Escalation { .. }, ReadResult::Result(_))
        | (Query::Result { .. }, ReadResult::Escalation(_)) => unreachable!("read terminal matches issued query"),
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
    let watch = stream.watch;
    let owner = stream.owner;
    match stream.state {
        Following::Closing | Following::Reopening => return,
        Following::Waiting | Following::Live => {}
        Following::Opening | Following::Backoff { .. } => unreachable!("events require an opened watch"),
    }
    domain.timers.cancel(Timer::Silent(id));
    match event {
        StreamEvent::Snapshot(Snapshot::Person(snapshot)) => {
            assert!(watch == Watch::Person, "person snapshot belongs to person watch");
            domain.streams.get_mut(id).expect("stream exists").state = Following::Live;
            apply_person(domain, &env.limits, snapshot);
            domain.live(env, out);
        }
        StreamEvent::Snapshot(Snapshot::Task(snapshot)) => {
            let Watch::Task { number } = watch else { unreachable!("task snapshot belongs to task watch") };
            assert!(snapshot.chip.task == number, "task snapshot names watched task");
            domain.streams.get_mut(id).expect("stream exists").state = Following::Live;
            if owner == (Owner::Page { generation: domain.generation }) {
                domain.task_snapshot(env, snapshot);
            }
            domain.live(env, out);
        }
        StreamEvent::Change(Change::Person(snapshot)) => {
            assert!(watch == Watch::Person, "person change belongs to person watch");
            assert!(stream.state == Following::Live, "change follows snapshot");
            apply_person(domain, &env.limits, snapshot);
        }
        StreamEvent::Change(Change::Task(snapshot)) => {
            let Watch::Task { number } = watch else { unreachable!("task change belongs to task watch") };
            assert!(snapshot.chip.task == number, "task change names watched task");
            assert!(stream.state == Following::Live, "change follows snapshot");
            if owner == (Owner::Page { generation: domain.generation }) {
                domain.task_snapshot(env, snapshot);
            }
        }
        StreamEvent::Change(Change::Left { key, why }) => {
            let Watch::Task { .. } = watch else { unreachable!("left change belongs to task watch") };
            if owner == (Owner::Page { generation: domain.generation })
                && let Some(object_id) = domain.objects.find(key)
            {
                let object = domain.objects.get(object_id).expect("indexed object exists");
                match object.card {
                    Card::Deciding { .. } | Card::Leaving { .. } => {}
                    Card::Open | Card::Refused { .. } => domain.leave_object(env, key, why),
                }
            }
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
    let watch = stream.watch;
    domain.timers.cancel(Timer::Silent(id));
    match state {
        Following::Closing => {
            domain.streams.retire(id);
            if domain.frame_stream == Some(id) {
                domain.frame_stream = None;
            }
            if domain.page_stream == Some(id) {
                domain.page_stream = None;
            }
            domain.streams.reclaim();
            domain.open_pending_task_watch(out);
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
                out.push(Request::Open { stream: id.token(), watch });
            }
        }
        Following::Opening | Following::Waiting | Following::Live => match end {
            StreamEnd::SignedOut => {
                domain.streams.retire(id);
                if domain.frame_stream == Some(id) {
                    domain.frame_stream = None;
                }
                if domain.page_stream == Some(id) {
                    domain.page_stream = None;
                }
                domain.sign_out(env, out);
            }
            StreamEnd::Gone if matches_task(watch) => {
                domain.streams.retire(id);
                if domain.page_stream == Some(id) {
                    domain.page_stream = None;
                }
                if let Watch::Task { number } = watch
                    && let Page::Task(page) = &domain.page
                    && page.number == number
                {
                    domain.read_task(env, Query::Escalation { task: number }, out);
                }
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

fn matches_task(watch: Watch) -> bool {
    match watch {
        Watch::Task { .. } => true,
        Watch::Person => false,
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
        Timer::Linger(id) => {
            if let Some(object) = domain.objects.get(id) {
                let key = object.key;
                if let Card::Leaving { until, .. } = object.card {
                    assert!(until <= env.now, "linger fires no earlier than departure");
                    domain.objects.remove(key);
                    if let Page::Task(page) = &mut domain.page {
                        if page.chip == Some(id) {
                            page.chip = None;
                        }
                        if page.escalation == Some(id) {
                            page.escalation = None;
                        }
                        if page.result == Some(id) {
                            page.result = None;
                        }
                    }
                    domain.changed();
                }
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
    }
}
