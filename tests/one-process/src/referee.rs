//! Outside conformance over actual bytes and independently decoded durable records.
pub(crate) mod durable;
pub(crate) use durable::{Capture, Store};
mod agent;
pub(crate) mod client;
pub(crate) mod forge;
pub mod restart;
pub mod secrets;
use crate::world::Proc;
pub(crate) use agent::Wire;
use jig_conformance::referee as rules;
use jig_domain_core as core;
use skein_lib::Time;
use skein_shell::Host;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    rc::Rc,
};
use temper_engine_domain as root;
#[derive(Default)]
pub(crate) struct Run {
    rules: Option<Rc<RefCell<rules::Referee>>>,
    pub(crate) rows: BTreeMap<root::Key, root::Record>,
    pub(crate) applied: u64,
    pub(crate) restarts: u32,
    pub(crate) drained: bool,
    pub(crate) settled: u32,
    pub(crate) answers: BTreeMap<[u8; 16], temper_client_domain::forge::Answer>,
    applications: BTreeMap<[u8; 16], u32>,
    pub(crate) pending_adoption: bool,
    pub(crate) window: Option<(u32, u32)>,
    window_start: Option<u32>,
    saved: BTreeSet<[u8; 16]>,
    pending_keys: BTreeSet<[u8; 16]>,
}
pub(crate) struct Referee {
    controls: skein_world::Controls,
    pub(crate) store: Rc<RefCell<Store>>,
    rules: Rc<RefCell<rules::Referee>>,
    run: Rc<RefCell<Run>>,
    phase: crate::world::Phase,
    attached: Option<()>,
    cold: Option<()>,
    acknowledged: restart::Records,
    uncertain: Option<restart::Records>,
    applied: u64,
    settled_at: Option<u64>,
    committed: Store,
    agent_seen: usize,
    client_seen: usize,
    client_life: u32,
    party: Option<u64>,
    handed: bool,
    signed_in: bool,
    released: bool,
    done: Option<()>,
    drained: Option<()>,
    forge: Rc<RefCell<forge::Forge>>,
    story: crate::world::Story,
}
impl Referee {
    pub(crate) fn new(
        controls: skein_world::Controls,
        store: Rc<RefCell<Store>>,
        scenario: crate::world::Story,
        forge: Rc<RefCell<forge::Forge>>,
        run: Rc<RefCell<Run>>,
        phase: crate::world::Phase,
    ) -> Self {
        let mut scope = rules::Scope {
            budget: 100,
            executors: BTreeSet::from([(0, 1)]),
            descendants: 8,
            depth: 8,
            ..rules::Scope::default()
        };
        let mut implications = BTreeSet::new();
        if matches!(scenario, crate::world::Story::Adoption | crate::world::Story::Adopt | crate::world::Story::Merge) {
            scope.tools = 3;
            scope.depth = 3;
            scope.executors.insert((1, 2));
            for kind in [3, 11] {
                scope.grants.push(rules::Grant {
                    connector: 1,
                    kind,
                    pattern: rules::Pattern {
                        base: vec![b"forge".to_vec(), b"127.0.0.1:3000".to_vec()],
                        terminal: b"acme".to_vec(),
                        open: true,
                    },
                });
            }
            implications.extend([(1, 3, 1), (1, 3, 2)]);
        }
        let policy = rules::Policy {
            deployment: scope.clone(),
            projects: BTreeMap::from([(1, scope.clone())]),
            people: BTreeMap::from([((1, 1), scope)]),
            projections: BTreeMap::new(),
            effect_accepters: BTreeSet::new(),
            requirements: Vec::new(),
            implications,
            kinds: BTreeMap::new(),
            owned: Vec::new(),
            participating: BTreeSet::new(),
            mechanics: BTreeSet::new(),
            delivery_bound: 10_000_000_000,
            uncertainty_bound: 10_000_000_000,
            story_steps: 100_000,
        };
        let rules = run.borrow().rules.clone().unwrap_or_else(|| Rc::new(RefCell::new(rules::Referee::new(policy))));
        let applied = run.borrow().applied;
        let mut committed = Store::new();
        committed.rows = run.borrow().rows.clone();
        run.borrow_mut().rules = Some(rules.clone());
        Self {
            controls,
            store,
            rules,
            run,
            phase,
            attached: None,
            cold: (applied > 0).then_some(()),
            acknowledged: restart::forge(&committed.rows),
            uncertain: None,
            applied,
            settled_at: None,
            committed,
            agent_seen: 0,
            client_seen: 0,
            client_life: 0,
            party: None,
            handed: false,
            signed_in: false,
            released: false,
            done: None,
            drained: None,
            forge,
            story: scenario,
        }
    }
    fn say(&mut self, now: Time, event: rules::Observed) {
        self.rules
            .borrow_mut()
            .observe(now.as_nanos(), event)
            .expect("one-process conformance over bytes and durable rows");
    }
    fn adoption_window(&mut self, service: &crate::world::ServiceHost) {
        let saved = crate::world::saved_adoptions();
        if !saved.is_empty() && self.run.borrow().window_start.is_none() {
            self.run.borrow_mut().window_start = Some(service.submitted);
        }
        self.run.borrow_mut().saved = saved;
        let start = self.run.borrow().window_start;
        if let Some(start) = start
            && self.run.borrow().window.is_none()
            && self.store.borrow().rows.values().any(|row| matches!(row,
                root::Record::Forge { row } if matches!(row.as_ref(), temper_engine_domain_forge::Stored::Application { .. })
            ))
        {
            self.run.borrow_mut().window = Some((start, service.before_iteration));
        }
    }
    pub(crate) fn check_roots(service: &crate::world::ServiceHost) {
        let mut text = String::new();
        write!(&mut text, "{:?}", secrets::Roots(&service.service)).expect("root formatting");
        secrets::require_formatted(&text, "root");
    }
}
pub(crate) fn terminals(store: &Store) -> u32 {
    u32::try_from(store.rows.values().filter(|row| matches!(row, root::Record::Core(core::Record::Core(core::CoreRecord::Terminal(terminal))) if matches!(terminal.end, jig_domain_tasks::End::Finished { .. }))).count()).expect("fixture terminals")
}
pub(crate) fn chat_waited(store: &Store) -> bool {
    store.rows.values().any(|row| {
        matches!(row,
            root::Record::Core(core::Record::Tasks(jig_domain_tasks::Stored::Live(task)))
                if task.number == 1 && task.phase == jig_domain_tasks::Phase::Active(jig_domain_tasks::Active::Idle)
        )
    }) && snapshot(store).receipts.contains(&rules::Receipt::Turn(1, 1, 2))
}
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "the chat's conformance receipts come from relevant durable core rows"
)]
fn snapshot(store: &Store) -> rules::Snapshot {
    let mut snapshot = rules::Snapshot::default();
    for row in store.rows.values() {
        match row {
            root::Record::Core(core::Record::Tasks(
                jig_domain_tasks::Stored::Live(task) | jig_domain_tasks::Stored::Ended(task),
            )) => {
                snapshot.tasks.insert(task.number, jig_core_world::observations::task(task));
                snapshot.receipts.insert(rules::Receipt::Task(task.number));
                if matches!(task.phase, jig_domain_tasks::Phase::Ended(_)) {
                    snapshot.receipts.insert(rules::Receipt::Ended(task.number));
                }
                for word in &task.inbox {
                    snapshot.receipts.insert(rules::Receipt::Word(task.number, word.number));
                }
            }
            root::Record::Core(core::Record::Core(core::CoreRecord::RunProof(proof))) => {
                snapshot.receipts.insert(rules::Receipt::Claim(proof.task, proof.attempt));
            }
            root::Record::Core(core::Record::Core(core::CoreRecord::Turn(turn))) => {
                snapshot.receipts.insert(rules::Receipt::Turn(turn.task, turn.attempt, turn.turn));
            }
            root::Record::Core(core::Record::Core(core::CoreRecord::Terminal(end))) => {
                snapshot.receipts.insert(rules::Receipt::Terminal(end.task, end.attempt));
            }
            root::Record::Core(core::Record::People(jig_domain_people::Stored::SignIn { number, .. })) => {
                snapshot.receipts.insert(rules::Receipt::SignIn(*number));
            }
            root::Record::Core(core::Record::People(jig_domain_people::Stored::Answer { key, .. })) => {
                snapshot.receipts.insert(rules::Receipt::Party(key.person, key.key));
            }
            _ => {}
        }
    }
    snapshot
}
impl skein_world::Referee<Proc> for Referee {
    fn act(&mut self, now: Time, procs: &mut [Proc]) {
        let (service, other) = procs.split_at_mut(1);
        let Proc::Service(service) = &mut service[0] else { panic!("service first") };
        if let Some(bound) = service.hooks {
            self.forge.borrow_mut().hand_hooks(bound);
        }
        if !service.attached {
            service.attached = true;
            if self.attached.is_some() || self.cold.is_some() {
                let pending_keys = {
                    let run = self.run.borrow();
                    run.saved.iter().filter(|key| !run.answers.contains_key(*key)).copied().collect::<BTreeSet<_>>()
                };
                self.run.borrow_mut().pending_adoption = !pending_keys.is_empty();
                self.run.borrow_mut().pending_keys = pending_keys;
                self.acknowledged = restart::forge(&self.committed.rows);
                self.uncertain = self
                    .store
                    .borrow()
                    .pending
                    .front()
                    .map(|pending| restart::next(&self.committed.rows, &pending.writes));
                self.say(now, rules::Observed::Restart);
                self.store.borrow_mut().recovering();
                self.settled_at = None;
                self.cold = Some(());
                self.handed = false;
                self.agent_seen = 0;
                self.run.borrow_mut().restarts += 1;
            } else if let crate::world::Phase::Fault { before, .. } = self.phase {
                service.cut_before = Some(before);
            }
            self.attached = Some(());
        }
        if !self.handed
            && let Some(link) = service.service.links().pop()
        {
            for proc in other.iter_mut() {
                if let Proc::Client(person) = proc
                    && person.signed_in == 0
                {
                    person.sign_in(&link);
                }
            }
            self.handed = true;
        }
        let done = other.iter().all(|proc| match proc {
            Proc::Client(person) => person.is_empty(),
            Proc::Service(_) | Proc::Agent(_) | Proc::Peer(_) | Proc::Git(_) | Proc::Shell(_) | Proc::Forge(_) => true,
        });
        if crate::stories::adopted(&self.store.borrow().rows) > 0 {
            self.forge.borrow_mut().adopted();
        }
        let complete = match self.phase {
            crate::world::Phase::SignIn => {
                other.iter().any(|proc| matches!(proc, Proc::Client(person) if person.signed_in > 0))
            }
            crate::world::Phase::Drain => {
                snapshot(&self.store.borrow()).receipts.contains(&rules::Receipt::Turn(1, 1, 1))
            }
            crate::world::Phase::Restart => self.run.borrow().settled > 0 && self.cold.is_none(),
            crate::world::Phase::Fault { .. } => {
                self.run.borrow().restarts > 0
                    && self.cold.is_none()
                    && crate::stories::adopted(&self.store.borrow().rows) > 0
            }
            crate::world::Phase::Story => match self.story {
                crate::world::Story::Chat { .. } => terminals(&self.store.borrow()) > 0,
                crate::world::Story::ChatReadThenWait { .. } => chat_waited(&self.store.borrow()),
                crate::world::Story::Adoption => crate::stories::adopted(&self.store.borrow().rows) > 0,
                crate::world::Story::Adopt => {
                    let forge = self.forge.borrow();
                    forge.complete() && forge::Forge::hooks_complete(other, crate::stories::ADOPT_HOOKS)
                }
                crate::world::Story::Merge => self.forge.borrow().complete(),
            },
        };
        if self.done.is_none()
            && (done || !matches!(self.phase, crate::world::Phase::Story | crate::world::Phase::Fault { .. }))
            && complete
        {
            for proc in other.iter_mut() {
                if let Proc::Client(person) = proc {
                    person.finish();
                }
            }
            self.controls.signal(0, skein_io::kernel::ServiceSignal::Terminate);
            self.done = Some(());
        }
        if self.done.is_some() {
            for proc in other {
                match proc {
                    Proc::Peer(peer) => peer.shutdown(),
                    Proc::Forge(forge) => forge.shutdown(),
                    Proc::Service(_) | Proc::Agent(_) | Proc::Git(_) | Proc::Shell(_) | Proc::Client(_) => {}
                }
            }
        }
    }
    fn observe(&mut self, now: Time, procs: &[Proc]) {
        self.forge.borrow_mut().observe(procs);
        let Proc::Service(service) = &procs[0] else { panic!("service first") };
        self.adoption_window(service);
        let store = self.store.borrow();
        for commit in &service.commits {
            if commit.number <= self.applied {
                continue;
            }
            for write in &commit.writes {
                restart::apply(&mut self.committed.rows, write);
            }
            self.rules
                .borrow_mut()
                .observe(
                    now.as_nanos(),
                    rules::Observed::Durable { number: commit.number, snapshot: snapshot(&self.committed) },
                )
                .expect("durable conformance");
            self.applied = commit.number;
            self.run.borrow_mut().applied = self.applied;
            self.run.borrow_mut().rows.clone_from(&self.committed.rows);
        }
        drop(store);
        let restored = self.store.borrow_mut().restored.take();
        if self.cold.is_some()
            && let Some(rows) = restored
        {
            restart::check(&self.acknowledged, self.uncertain.as_ref(), &restart::forge(&rows))
                .expect("restart recovered a whole forge commit");
            let mut recovered = Store::new();
            recovered.rows = rows;
            let receipts = snapshot(&recovered).receipts;
            self.say(now, rules::Observed::ColdStore { receipts });
            self.cold = None;
        }
        // Only durable commits change these rows. Preserve the check at each
        // observation without scanning the same map again between commits.
        let store = self.store.borrow();
        if self.settled_at != Some(store.applied) {
            self.run.borrow_mut().settled = settled(&store);
            self.settled_at = Some(store.applied);
        }
        drop(store);
        for event in &service.wire.events[self.agent_seen..] {
            self.say(now, event.clone());
        }
        self.agent_seen = service.wire.events.len();
        for proc in procs {
            if let Proc::Client(person) = proc {
                if person.life != self.client_life {
                    self.client_life = person.life;
                    self.client_seen = 0;
                }
                for event in &person.wire.received[self.client_seen..] {
                    match event {
                        client::Received::ApplicationSent { key } => {
                            *self.run.borrow_mut().applications.entry(*key).or_default() += 1;
                        }
                        client::Received::Application { key, answer } => {
                            assert!(
                                matches!(answer, temper_client_domain::forge::Answer::Adopted(_)),
                                "adoption succeeded"
                            );
                            if let Some(old) = self.run.borrow_mut().answers.insert(*key, answer.clone()) {
                                assert_eq!(&old, answer, "one outcome per adoption key");
                            }
                        }
                        client::Received::SignedIn(_)
                        | client::Received::Answer { .. }
                        | client::Received::Delivery(_)
                        | client::Received::Read(_) => {}
                    }
                    if let client::Received::SignedIn(party) = event {
                        self.party = Some(*party);
                        self.signed_in = true;
                    }
                    let requires = client_receipts(event, &self.store.borrow(), self.party);
                    self.released |= requires
                        .iter()
                        .any(|receipt| matches!(receipt, rules::Receipt::Turn(..) | rules::Receipt::Terminal(..)));
                    if !requires.is_empty() {
                        self.say(now, rules::Observed::Released { requires });
                    }
                }
                self.client_seen = person.wire.received.len();
            }
        }
        if self.done.is_some() && self.drained.is_none() && service.is_empty() {
            Self::check_roots(service);
            assert!(self.store.borrow().pending.is_empty(), "drain settles commits");
            self.say(now, rules::Observed::Drained { at: self.applied });
            self.drained = Some(());
            self.run.borrow_mut().drained = true;
        }
        self.say(now, rules::Observed::Tick);
    }
    fn next_deadline(&self) -> Option<Time> {
        self.done.is_none().then_some(Time::from_nanos(10_000_000_000))
    }
    fn overdue(&self, now: Time) -> Option<String> {
        (self.done.is_none() && now >= Time::from_nanos(10_000_000_000)).then(|| {
            format!(
                "story not complete: link {}, signed in {}, terminals {}, tasks {:?}, agents {:?}, forge {:?}",
                self.handed,
                self.signed_in,
                terminals(&self.store.borrow()),
                self.store
                    .borrow()
                    .rows
                    .values()
                    .filter(|row| matches!(
                        row,
                        root::Record::Core(core::Record::Tasks(_) | core::Record::Core(core::CoreRecord::Call(_)))
                    ))
                    .collect::<Vec<_>>(),
                self.store.borrow().agent_records,
                self.store
                    .borrow()
                    .rows
                    .values()
                    .filter(|row| matches!(row, root::Record::Forge { .. }))
                    .collect::<Vec<_>>()
            )
        })
    }
    fn passed(&self) -> bool {
        self.done.is_some()
            && (matches!(self.phase, crate::world::Phase::Restart)
                || self.signed_in
                    && (self.released
                        || !matches!(self.phase, crate::world::Phase::Story)
                        || matches!(
                            self.story,
                            crate::world::Story::Adoption | crate::world::Story::Adopt | crate::world::Story::Merge
                        )))
    }
}
pub(crate) fn require_replayed(run: &Run, store: &Store) {
    assert_eq!(
        run.answers.keys().copied().collect::<BTreeSet<_>>(),
        run.pending_keys,
        "original pending keys answered"
    );
    assert_eq!(run.answers.len(), 1, "original adoption answered once");
    assert_eq!(
        store
            .rows
            .values()
            .filter(|row| matches!(row,
                root::Record::Forge { row } if matches!(row.as_ref(),
                    temper_engine_domain_forge::Stored::Application {
                        outcome: temper_engine_domain_forge::ApplicationOutcome::Adopted(_), ..
                    })
            ))
            .count(),
        1,
        "one durable adoption outcome"
    );
}
fn settled(store: &Store) -> u32 {
    u32::try_from(
        store
            .rows
            .values()
            .filter(|row| {
                matches!(row,
                    root::Record::Core(core::Record::Core(core::CoreRecord::Terminal(terminal)))
                        if matches!(terminal.end, jig_domain_tasks::End::Failed(jig_domain_tasks::Class::Lost))
                )
            })
            .count(),
    )
    .expect("settled claims")
}
#[expect(clippy::wildcard_enum_match_arm, reason = "the conformance receipts select committed client documents")]
fn client_receipts(event: &client::Received, store: &Store, party: Option<u64>) -> Vec<rules::Receipt> {
    use jig_client::contract as c;
    match event {
        client::Received::SignedIn(party) => {
            let receipts: Vec<_> = store
                .rows
                .values()
                .filter_map(|row| match row {
                    root::Record::Core(core::Record::People(jig_domain_people::Stored::SignIn {
                        number,
                        person,
                        ..
                    })) if person == party => Some(rules::Receipt::SignIn(*number)),
                    _ => None,
                })
                .collect();
            assert!(!receipts.is_empty(), "sign-in bytes follow a durable sign-in");
            receipts
        }
        client::Received::Answer { key, answer } => {
            if matches!(answer, c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::MarkedSeen { .. }))) {
                return Vec::new(); // A seen watermark has no kept request answer.
            }
            let mut receipts = vec![rules::Receipt::Party(party.expect("signed-in request"), *key)];
            if let c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::Created { first, last, .. })) = answer {
                receipts.extend((*first..=*last).map(rules::Receipt::Task));
            }
            receipts
        }
        client::Received::Delivery(c::Delivery::RunState(run)) if run.committed_turn > 0 => {
            vec![rules::Receipt::Turn(run.task, run.attempt.expect("committed attempt"), run.committed_turn)]
        }
        client::Received::Read(c::ReadValue::Historical(value)) => {
            let c::OwnerHistoricalValue::Task { document, result } = &value.value else { return Vec::new() };
            let c::ChipPhase::Owner { details, .. } = document.chip.details.as_ref() else {
                panic!("owner task facts")
            };
            assert_eq!(result.task.task, document.chip.task, "result belongs to its task");
            vec![
                rules::Receipt::Ended(document.chip.task),
                rules::Receipt::Terminal(document.chip.task, details.attempt.expect("finished attempt")),
            ]
        }
        client::Received::Read(c::ReadValue::Activity(page)) => page
            .rows
            .iter()
            .filter_map(|row| {
                if let c::Activity::Owner { source, .. } = &row.event {
                    row.event.committed_turn().map(|turn| rules::Receipt::Turn(source.task, turn.attempt, turn.number))
                } else {
                    None
                }
            })
            .collect(),
        client::Received::Application { key, .. } => {
            vec![rules::Receipt::Party(party.expect("signed-in adoption"), *key)]
        }
        client::Received::ApplicationSent { .. } | client::Received::Delivery(_) | client::Received::Read(_) => {
            Vec::new()
        }
    }
}
