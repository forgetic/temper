//! One root input per pass, with store answers always accepted (protocol/README.md, 6).
use crate::{End, route, stop, store::route::Up};
use alloc::boxed::Box;
use skein_lib::{Env, List, Queue, Time, Wall};
use temper_engine_domain as root;

pub(crate) struct Engine {
    pub(crate) domain: root::Domain,
    pub(crate) env: Env<root::Limits>,
    pub(crate) store: crate::store::Store,
    pub(crate) accounts: Option<crate::accounts::Accounts>,
    pub(crate) workers: crate::workers::Workers,
    pub(crate) clients: crate::clients::Clients,
    pub(crate) demo: Option<crate::settings::Demo>,
    pub(crate) box_link: route::box_link::Link,
    pub(crate) setup: crate::setup::Layer,
    pub(crate) forge: crate::forge::Layer,
    pub(crate) forge_records: crate::forge::records::Loader,
    pub(crate) admission: Option<skein_lib::Token>,
    pub(crate) facts: u64,
    pub(crate) values: crate::values::Values,
    pub(crate) events: Queue<root::Event>,
    pub(crate) requests: Queue<root::Request>,
    pub(crate) end: Option<End>,
    pub(crate) listening: Queue<crate::Note>,
    pub(crate) notes: Queue<crate::Note>,
    pub(crate) first_periods: List<(u32, u64)>,
    pub(crate) drain: Option<stop::Drain>,
    pub(crate) blocked: bool,
    pub(crate) pending: Option<Box<root::Event>>,
    begin: bool,
    pub(crate) credentials: List<u32>,
    ready: bool,
    aborted: Option<()>,
    max_out: u32,
}
pub(crate) struct Children {
    pub(crate) accounts: Option<crate::accounts::Accounts>,
    pub(crate) workers: crate::workers::Workers,
    pub(crate) clients: crate::clients::Clients,
    pub(crate) demo: Option<crate::settings::Demo>,
    pub(crate) setup: crate::setup::Layer,
    pub(crate) forge: crate::forge::Layer,
    pub(crate) limits: crate::limits::Limits,
    pub(crate) notes: Queue<crate::Note>,
}
impl Engine {
    /// Membership errors are unreachable after `startup::checks::root`; keep the
    /// typed refusal for callers constructing an engine directly.
    pub(crate) fn new(
        config: root::Config,
        limits: &root::Limits,
        store: crate::store::Store,
        children: Children,
        values: crate::values::Values,
        first_periods: List<(u32, u64)>,
    ) -> Result<Self, jig_protocol_config::Problems> {
        let forge_records = crate::forge::records::Loader::new(&children.limits, &children.workers.forges);
        let Children { accounts, workers, mut clients, demo, forge, setup, notes, limits: own } = children;
        clients.query_bytes = own.routes.query_bytes;
        let max_out = root::max_out(limits);
        let capacity = max_out.checked_mul(2).expect("root routing room");
        let credentials = values.identities.numbers();
        let mut domain = match root::Domain::new(config, limits) {
            Ok(domain) => domain,
            Err(problem) => {
                let (path, what) = crate::startup::checks::membership(problem);
                let mut problems = jig_protocol_config::Problems::empty();
                problems.push(path, what);
                return Err(problems);
            }
        };
        domain.connection_limits(own.forge_component.forges, own.forge_component.installations);
        Ok(Self {
            credentials,
            domain,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *limits },
            store,
            accounts,
            workers,
            clients,
            demo,
            forge,
            setup,
            box_link: route::box_link::Link::new(0, false),
            forge_records,
            admission: None,
            facts: 0,
            values,
            events: Queue::with_capacity(capacity.checked_mul(route_out()).expect("account event room")),
            requests: Queue::with_capacity(max_out),
            end: None,
            listening: Queue::with_capacity(3),
            notes,
            first_periods,
            drain: None,
            blocked: false,
            pending: None,
            begin: true,
            ready: true,
            aborted: None,
            max_out,
        })
    }
    pub(crate) fn room(&self) -> bool {
        self.setup.batch_room(self.max_out)
            && self.forge.room(self.max_out)
            && self.store.room()
            && self.workers.batch_room(self.max_out)
            && self.clients.room(self.max_out)
            && self.clients.commit_room()
            && self.events.room() >= self.max_out.checked_mul(route_out()).expect("translated root outputs")
            && match &self.accounts {
                Some(accounts) => accounts.batch_room(self.max_out),
                None => true,
            }
    }
    pub(crate) fn step(&mut self, event: root::Event) {
        let watcher = watcher(&event);
        if let Err(event) = root::try_step(&mut self.domain, &self.env, event) {
            self.pending = Some(Box::new(event));
            self.blocked = true;
        } else {
            self.admission = watcher;
        }
        self.drain_facts();
    }
    fn drain_facts(&mut self) {
        for _ in 0..root::FACTS {
            if self.domain.pop_fact().is_none() {
                break;
            }
            self.facts = self.facts.saturating_add(1);
        }
    }
    pub(crate) fn next_deadline(&self) -> Option<Time> {
        let mut next = crate::accounts::earliest(self.store.next_deadline(), self.workers.next_deadline());
        if self.end.is_none() {
            next = crate::accounts::earliest(next, self.domain.next_deadline(&self.env));
            next = crate::accounts::earliest(next, self.values.accounts.next_expiry());
        }
        next = crate::accounts::earliest(next, self.setup.next_deadline());
        next = crate::accounts::earliest(next, self.clients.next_deadline());
        next = crate::accounts::earliest(next, self.forge.component.next_deadline());
        crate::accounts::earliest(
            next,
            match &self.accounts {
                Some(accounts) => accounts.next_deadline(),
                None => None,
            },
        )
    }
    pub(crate) fn work_pending(&self, now: Time) -> bool {
        let components = (self.store.opened || self.end.is_some()) && self.clients.work_pending(now)
            || (self.store.opened || self.end.is_some()) && self.setup.work_pending(now)
            || self.setup.lifecycle_ready(self)
                && self.setup.room()
                && self.store.room()
                && self.forge.room(2)
                && self.events.room() >= 4
            || self.store.work_pending(now)
            || (self.store.opened || self.end.is_some()) && self.forge.work_pending(now)
            || (self.store.opened || self.end.is_some()) && self.workers.work_pending(now)
            || (self.store.opened || self.end.is_some())
                && match &self.accounts {
                    Some(accounts) => accounts.work_pending(now),
                    None => false,
                };
        if self.end.is_some() || !self.store.opened {
            return components;
        }
        let due = match self.next_deadline() {
            Some(deadline) => deadline <= now,
            None => false,
        };
        components
            || self.room()
                && (self.begin
                    || self.ready
                    || !self.blocked
                        && (self.pending.is_some()
                            || !self.events.is_empty()
                            || due
                            || self.domain.ready() && !self.first_periods.is_empty()))
    }
    pub(crate) fn abort_if_ended(&mut self) {
        if self.end.is_some() {
            self.setup.close();
        }
        if self.end.is_some() && self.forge.room(1) {
            self.forge.down(skein_forge_connection::Request::Close);
        }
        if let Some(End::Aborted | End::Stopped(_)) = self.end
            && self.aborted.is_none()
        {
            self.aborted = Some(());
            self.store.close();
            self.clients.abort(!self.store.opened);
            self.workers.abort(self.env.now, &mut self.events);
            if let Some(accounts) = &mut self.accounts {
                accounts.abort();
            }
        }
    }
}

/// Every child sees this iteration's time before an input can arm a deadline.
pub(crate) fn clock(engine: &mut Engine, now: Time, wall: Wall) {
    engine.env.now = now;
    engine.env.wall = wall;
    engine.store.clock(now, wall);
    if let Some(accounts) = &mut engine.accounts {
        accounts.clock(now, wall);
    }
    engine.clients.clock(now, wall);
    engine.forge.env.now = now;
    engine.forge.env.wall = wall;
    engine.setup.env.now = now;
    engine.setup.env.wall = wall;
}

pub(crate) fn iterate(engine: &mut Engine, now: Time, wall: Wall) {
    clock(engine, now, wall);
    engine.store.iterate();
    answer(engine);
    if !engine.store.opened && engine.end.is_none() {
        return;
    }
    if let Some(accounts) = &mut engine.accounts {
        accounts.iterate(now, wall, &mut engine.values, &mut engine.events, &mut engine.store);
    }
    crate::forge::records::advance(engine);
    if !engine.forge_records.loading() || engine.end.is_some() || engine.drain.is_some() {
        engine.forge.iterate(now, wall);
    }
    // Reconcile must observe the root's restored repositories.
    if !engine.begin && engine.domain.connectors_restored() {
        crate::setup::lifecycle::restored(engine);
    }
    crate::setup::advance(engine);
    forge_pass(engine);
    engine.workers.iterate(now, &mut engine.events);
    let open = !engine.begin && engine.domain.ready() && engine.first_periods.is_empty();
    engine.clients.starting = if open { None } else { Some(()) };
    if let Some(demo) = &mut engine.demo {
        demo.state = engine.domain.demo_state();
    }
    engine.clients.iterate(
        now,
        wall,
        &mut engine.events,
        &engine.workers.forges,
        engine.demo.as_ref(),
        &mut engine.accounts,
    );
    clients_pass(engine);
    for _ in 0..engine.clients.early.len() {
        // A not-ready key returns its carried value before replying.
        let replies = if open { 1 } else { 2 };
        if !engine.clients.room(replies) || engine.events.room() == 0 {
            break;
        }
        let event = engine.clients.early.pop().expect("staged client input");
        if open {
            engine.events.push(event);
        } else if let root::Event::Party(value) = event {
            engine.clients.not_ready(value);
        }
    }
    stop::advance(engine);
    if engine.end.is_some() {
        engine.abort_if_ended();
        return;
    }
    if !engine.room() {
        return;
    }
    if !engine.forge_records.done() && engine.drain.is_none() {
        return;
    }
    if engine.begin {
        if engine.credentials.is_empty() {
            engine.begin = false;
            engine.step(root::Event::Restart);
        } else {
            let last = engine.credentials.len().checked_sub(1).expect("first credential");
            let number = *engine.credentials.get(last).expect("configured identity");
            engine.credentials.truncate(last);
            engine.step(root::Event::Credential {
                number,
                generation: 1,
                valid: skein_lib::Duration::from_nanos(u64::MAX),
            });
        }
    } else if !engine.blocked && engine.pending.is_some() {
        let event = engine.pending.take().expect("held input");
        engine.step(*event);
    } else if !engine.blocked && engine.domain.ready() && !engine.first_periods.is_empty() {
        let last = engine.first_periods.len().checked_sub(1).expect("first period");
        let (project, budget) = *engine.first_periods.get(last).expect("configured project");
        engine.first_periods.truncate(last);
        engine.step(root::Event::Party(root::PartyInput::Period { project, period: 1, budget, next: None }));
    } else if !engine.blocked && !engine.events.is_empty() {
        let event = engine.events.pop().expect("ready input");
        engine.step(event);
    } else if !engine.blocked
        && let Some(deadline) = engine.domain.next_deadline(&engine.env)
        && deadline <= now
    {
        root::fire(&mut engine.domain, &engine.env);
        engine.drain_facts();
    }
    release(engine);
    if engine.forge.room(1) {
        engine.forge.down(skein_forge_connection::Request::Offer);
    }
    clients_pass(engine);
}
fn clients_pass(engine: &mut Engine) {
    engine.clients.advance(&mut engine.events, &engine.workers.forges, engine.demo.as_ref(), &mut engine.accounts);
    if engine.end.is_none() {
        engine.clients.secrets_step(&mut engine.store);
    }
    engine.clients.advance(&mut engine.events, &engine.workers.forges, engine.demo.as_ref(), &mut engine.accounts);
}
fn release(engine: &mut Engine) {
    engine.ready = true;
    let mut empty = 0_u32;
    for _ in 0..root::max_out(&engine.env.limits) {
        if !engine.room() || engine.end.is_some() {
            break;
        }
        root::release(&mut engine.domain, &engine.env, &mut engine.requests);
        if engine.requests.is_empty() {
            root::try_step(&mut engine.domain, &engine.env, root::Event::Resume).expect("owner continuation admitted");
            engine.drain_facts();
            root::release(&mut engine.domain, &engine.env, &mut engine.requests);
        }
        engine.drain_facts();
        if !engine.requests.is_empty() || engine.domain.quiescent() {
            engine.blocked = false;
        }
        if engine.requests.is_empty() {
            empty = empty.checked_add(1).expect("two empty passes");
        } else {
            empty = 0;
        }
        for _ in 0..engine.requests.len() {
            let request = engine.requests.pop().expect("root outputs");
            route::request(engine, request);
            if engine.end.is_some() {
                break;
            }
        }
        if empty == 2 {
            engine.ready = false;
            break;
        }
    }
    if let Some(watcher) = engine.admission.take() {
        engine.clients.down(jig_protocol_engine_clients::Event::Admitted { watcher });
    }
    engine.domain.reclaim();
    stop::advance(engine);
}
pub(crate) fn answer(engine: &mut Engine) {
    if engine.end.is_some() {
        for _ in 0..engine.store.above.len() {
            let _late = engine.store.above.pop();
        }
        for _ in 0..engine.store.routed.len() {
            let _late = engine.store.routed.pop();
        }
        return;
    }
    for _ in 0..engine.store.above.len() {
        if !engine.setup.room()
            || engine.store.routed.room() < jig_protocol_store::MAX_OUT_DOWN
            || engine.notes.room() == 0
        {
            break;
        }
        let event = engine.store.above.pop().expect("store terminals");
        match event {
            jig_protocol_store::Event::Opened { installation, .. } => {
                engine.store.opened = true;
                engine.store.installation = installation;
            }
            jig_protocol_store::Event::OpenRefused { why, .. } => open_refused(engine, why),
            jig_protocol_store::Event::Patched { of, patch, counts, .. } => {
                engine.notes.try_push(crate::Note::Patched { of, patch, counts }).expect("patch progress note room");
            }
            event @ jig_protocol_store::Event::Site { .. } => {
                crate::store::route::up(&mut engine.store.routes, event, &mut engine.store.routed);
            }
            event @ (jig_protocol_store::Event::Kept { owner }
            | jig_protocol_store::Event::Stale { owner }
            | jig_protocol_store::Event::Erased { owner }
            | jig_protocol_store::Event::Carried { owner, .. }
            | jig_protocol_store::Event::Secret { owner, .. }
            | jig_protocol_store::Event::SecretsLoaded { owner, .. }) => {
                if owner == crate::forge::records::LOAD {
                    if let jig_protocol_store::Event::SecretsLoaded { rows, next, .. } = event {
                        engine.forge_records.loaded(rows, next);
                    } else if let jig_protocol_store::Event::Erased { .. } = event {
                        engine.forge_records.erased();
                    } else {
                        stop::unavailable(engine, b"forge records");
                    }
                } else if crate::tokens::prefix(owner) == 11 {
                    crate::setup::answer(engine, event);
                } else if crate::accounts::owns(owner) {
                    engine.store.sources.push(event);
                } else {
                    engine.store.secrets.push(event);
                }
            }
            event @ (jig_protocol_store::Event::Committed { .. }
            | jig_protocol_store::Event::Failed { .. }
            | jig_protocol_store::Event::Restored { .. }
            | jig_protocol_store::Event::Loaded { .. }
            | jig_protocol_store::Event::Read { .. }
            | jig_protocol_store::Event::Got { .. }
            | jig_protocol_store::Event::Turns { .. }
            | jig_protocol_store::Event::Unread { .. }
            | jig_protocol_store::Event::Corrupt { .. }) => {
                if let jig_protocol_store::Event::Corrupt { owner } = &event
                    && crate::tokens::prefix(*owner) == 11
                {
                    crate::setup::answer(engine, event);
                } else if let jig_protocol_store::Event::Corrupt { owner } = &event
                    && *owner == crate::forge::records::LOAD
                {
                    stop::unavailable(engine, b"forge records");
                } else {
                    crate::store::route::up(&mut engine.store.routes, event, &mut engine.store.routed);
                }
            }
        }
        if engine.end.is_some() {
            return;
        }
    }
    answer_routed(engine);
}

fn answer_routed(engine: &mut Engine) {
    for _ in 0..engine.store.routed.len() {
        match engine.store.routed.pop().expect("store routing") {
            Up::Root(event) => {
                if let root::Event::Store(root::StoreInput::Committed { number, .. }) = event.as_ref() {
                    engine.clients.committed(*number);
                }
                if let root::Event::Store(root::StoreInput::Loaded { rows, .. }) = event.as_ref() {
                    for row in rows {
                        if row.key()
                            == root::Key::Core(jig_domain_core::Key::Core(jig_domain_core::CoreKey::Deployment))
                        {
                            engine.first_periods.truncate(0);
                        }
                    }
                }
                if let root::Event::Store(root::StoreInput::Uncommitted { number }) = *event {
                    stop::failed(engine, number);
                } else {
                    engine.blocked = false;
                    engine.step(*event);
                }
            }
            Up::Ask(request) => engine.store.ask(request),
        }
        if engine.end.is_some() {
            break;
        }
    }
}

fn open_refused(engine: &mut Engine, why: jig_protocol_store::OpenRefusal) {
    use jig_protocol_store::OpenRefusal;
    #[expect(clippy::match_same_arms, reason = "each store refusal keeps its own startup stop line")]
    match why {
        why @ OpenRefusal::Format { .. } => stop::open(engine, &why),
        why @ OpenRefusal::Deployment => stop::open(engine, &why),
        why @ OpenRefusal::Numbers { .. } => stop::open(engine, &why),
        why @ OpenRefusal::Failed => stop::open(engine, &why),
        why @ OpenRefusal::OlderBuild { .. } => stop::open(engine, &why),
        why @ OpenRefusal::Generations { .. } => stop::open(engine, &why),
        why @ OpenRefusal::Ahead { .. } => stop::open(engine, &why),
        why @ OpenRefusal::Patch { .. } => stop::open(engine, &why),
        why @ OpenRefusal::Set { .. } => stop::open(engine, &why),
        why @ OpenRefusal::Check { .. } => stop::open(engine, &why),
    }
}

pub(crate) fn route_out() -> u32 {
    skein_oauth_accounts::MAX_OUT_DOWN
        .above
        .max(jig_protocol_engine_workers::MAX_OUT_DOWN)
        .max(jig_protocol_engine_clients::MAX_OUT_DOWN)
}

fn watcher(event: &root::Event) -> Option<skein_lib::Token> {
    match event {
        root::Event::Party(root::PartyInput::Watch { watcher, .. } | root::PartyInput::WatchRun { watcher, .. }) => {
            Some(*watcher)
        }
        root::Event::Party(_)
        | root::Event::Worker(_)
        | root::Event::Store(_)
        | root::Event::Account(_)
        | root::Event::Forge(_)
        | root::Event::Released(_)
        | root::Event::Credential { .. }
        | root::Event::Setup { .. }
        | root::Event::Restart
        | root::Event::Stop
        | root::Event::Resume => None,
    }
}

pub(crate) fn forge_pass(engine: &mut Engine) {
    for _ in 0..engine.forge.above.len() {
        if !engine.forge.room(1) || engine.end.is_none() && !engine.room() {
            break;
        }
        #[expect(clippy::match_same_arms, reason = "separate exhaustive facts name the tasks that give them meaning")]
        match engine.forge.above.pop().expect("forge outputs") {
            skein_forge_connection::Event::Answered(answer) => {
                if engine.end.is_some() {
                    engine.forge.down(skein_forge_connection::Request::Returned(answer));
                    continue;
                }
                if let Err(root::Event::Forge(event)) =
                    root::try_step(&mut engine.domain, &engine.env, crate::forge::event(answer))
                {
                    engine.forge.down(skein_forge_connection::Request::Refused(
                        crate::forge::back(event).expect("the routing hands the root only the component's answers"),
                    ));
                }
                engine.drain_facts();
            }
            skein_forge_connection::Event::Hint(hinted) => {
                if engine.drain.is_some()
                    || engine.end.is_some()
                    || crate::forge::ended(engine, hinted.hint.repository.forge)
                {
                    continue;
                }
                let connection = hinted.connection;
                if let Err(root::Event::Forge(event)) =
                    root::try_step(&mut engine.domain, &engine.env, crate::forge::hinted(hinted))
                {
                    let temper_engine_domain_forge::Event::Hint { hint } = *event else {
                        unreachable!("hint retained by admission")
                    };
                    engine.forge.down(skein_forge_connection::Request::RefusedHint(
                        skein_forge_connection::hooks::Hinted { connection, hint },
                    ));
                }
                engine.drain_facts();
            }
            skein_forge_connection::Event::Connected { forge } => crate::setup::lifecycle::connected(engine, forge),
            skein_forge_connection::Event::Installation { forge, installation, account, change } => {
                crate::setup::lifecycle::installation(engine, forge, installation, account, change);
            }
            skein_forge_connection::Event::GitToken { forge, installation, account, token, expires } => {
                crate::forge::tokens::token(engine, forge, installation, account, token, expires);
            }
            // temper-21 names unknown log origins, without deciding on them.
            skein_forge_connection::Event::Fact { fact: skein_forge_connection::Fact::Origin { .. }, .. } => {}
            // temper-21 logs gaps, without deciding on them.
            skein_forge_connection::Event::Fact { fact: skein_forge_connection::Fact::Gap, .. } => {}
            skein_forge_connection::Event::Fact {
                forge,
                fact: skein_forge_connection::Fact::Unkept { installation },
            } => {
                if engine.notes.room() > 0 {
                    engine.notes.push(crate::Note::ForgeUnkept { number: forge, installation });
                }
            }
            skein_forge_connection::Event::Listening { addr } => {
                if let Some(addr) = addr.socket() {
                    engine.setup.webhooks_listening(addr);
                    let note = crate::Note::Listening { listener: crate::Listening::Webhooks, addr };
                    engine.listening.try_push(note).expect("one note per listener bind");
                }
            }
            skein_forge_connection::Event::ListenerLost => stop::hooks(engine),
            skein_forge_connection::Event::Stopped { forge, why } => {
                crate::setup::lifecycle::stopped_connection(engine, forge);
                crate::setup::lifecycle::state(
                    engine,
                    forge,
                    temper_engine_domain_forge::Connection::Stopped {
                        reason: crate::setup::lifecycle::stopped_text(why),
                    },
                );
                // At most once per connect generation, possibly after Connected. temper-02 revisits
                // dropping a full queue when runtime reconnects are routed.
                if engine.notes.room() > 0 {
                    engine.notes.push(crate::Note::ForgeStopped { number: forge, why });
                }
            }
            skein_forge_connection::Event::Closed => engine.forge.closed = true,
        }
    }
}
