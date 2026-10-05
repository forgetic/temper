use crate::{
    Active, Event, Fact, Limits, New, Party, Phase, Problem, Refusal, Request, Stored, Stub, TaskRecord, Tries,
};
use alloc::boxed::Box;
use skein_lib::{Deadlines, Env, Id, List, Map, Queue, ReplyTo, Rng, Slab, Time, Wall};
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Alarm {
    pub until: Wall,
    pub due: Time,
}
#[derive(Debug)]
pub(crate) struct Task {
    pub record: TaskRecord,
    pub alarm: Option<Alarm>,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Startup {
    Restoring,
    Ready,
    Failed,
}
#[derive(Debug)]
pub struct Domain {
    pub(crate) startup: Startup,
    pub(crate) tasks: Slab<Task>,
    pub(crate) names: Map<u64, Id<Task>>,
    pub(crate) stubs: Map<u64, Stub>,
    pub(crate) alarms: Deadlines<u64>,
    pub(crate) charters: Box<[u32]>,
    pub(crate) rng: Rng,
    facts: Queue<Fact>,
    lost: u64,
}
impl Domain {
    #[must_use]
    pub fn new(l: &Limits, seed: u64, charters: Box<[u32]>) -> Domain {
        assert!(crate::worst_case(l).is_some(), "task limits are valid");
        assert!(charters.len() <= usize::try_from(l.charters).expect("u32 fits usize"), "configured charters fit");
        for (at, charter) in charters.iter().enumerate() {
            for earlier in charters.iter().take(at) {
                assert!(earlier != charter, "charters are unique");
            }
        }
        Domain {
            startup: Startup::Restoring,
            tasks: Slab::with_capacity(l.tasks),
            names: Map::with_capacity(l.tasks),
            stubs: Map::with_capacity(l.stubs),
            alarms: Deadlines::with_capacity(l.tasks),
            charters,
            rng: Rng::new(seed),
            facts: Queue::with_capacity(l.facts),
            lost: 0,
        }
    }
    #[must_use]
    pub fn ready(&self) -> bool {
        self.startup == Startup::Ready
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.next_deadline() {
            Some(at) => at <= now,
            None => false,
        }
    }
    pub fn reclaim(&mut self) {
        self.tasks.reclaim();
    }
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }
    #[must_use]
    pub const fn facts_lost(&self) -> u64 {
        self.lost
    }
}
pub(crate) fn output_bound(l: &Limits) -> Option<u32> {
    l.tasks.checked_mul(16)?.checked_add(l.stubs.checked_mul(2)?)?.checked_add(l.batch.checked_mul(2)?)?.checked_add(4)
}
/// Cascading dependency and closing decisions touch at most the bounded live
/// set; each task advances through a constant number of phases in one step.
#[must_use]
pub fn max_out(l: &Limits) -> u32 {
    output_bound(l).expect("task limits admit output bound")
}
pub fn step(d: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::RememberStub { reply_to, stub } => crate::stored::remember(d, env, reply_to, stub, out),
        Event::ForgetStub { reply_to, task } => crate::stored::forget(d, reply_to, task, out),
        Event::Restore { record } => crate::stored::restore(d, env, record, out),
        Event::Restored => crate::stored::restored(d, env, out),
        Event::Make { reply_to, creator, batch } => make(d, env, reply_to, creator, batch, out),
        Event::Prepare { reply_to, task } => crate::run::prepare(d, env, reply_to, task, out),
        Event::Claim { reply_to, task, attempt } => crate::run::claim(d, env, reply_to, task, attempt, out),
        Event::Started { task, attempt } => crate::run::started(d, env, task, attempt, out),
        Event::Activation { reply_to, task, attempt, end } => {
            crate::run::activation(d, env, reply_to, task, attempt, end, out);
        }
        Event::PreparationFailed { task } => crate::run::preparation_failed(d, env, task, out),
        Event::Hold { task, why } => crate::run::hold(d, env, task, why, out),
        Event::Release { reply_to, task } => crate::run::release(d, env, reply_to, task, out),
        Event::Cancel { reply_to, task, reason } => crate::closing::cancel(d, env, reply_to, task, reason, out),
        Event::Settled { task } => crate::closing::settled(d, env, task, out),
    }
    if d.ready() {
        crate::closing::progress(d, env, out);
    }
}
/// One backoff expiration per iteration; a wall-clock correction does not
/// change an already projected monotonic deadline.
pub fn fire(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if !d.ready() {
        return;
    }
    if let Some(number) = d.alarms.expire(env.now)
        && let Some(task) = task_mut(d, number)
    {
        match task.record.phase {
            Phase::Active(Active::BackingOff { .. }) => {
                task.record.phase = Phase::Active(Active::Due);
                publish(d, env, number, out);
                activate(d, number, out);
            }
            Phase::Waiting
            | Phase::Active(
                Active::Idle | Active::Due | Active::Preparing | Active::Claimed { .. } | Active::Running { .. },
            )
            | Phase::Closing(_)
            | Phase::Held { .. }
            | Phase::Ended(_) => {}
        }
    }
    crate::closing::progress(d, env, out);
}
pub(crate) fn record(d: &Domain, number: u64) -> Option<&TaskRecord> {
    Some(&d.tasks.get(*d.names.get(&number)?).expect("name indexes live task").record)
}
pub(crate) fn task_mut(d: &mut Domain, number: u64) -> Option<&mut Task> {
    let id = *d.names.get(&number)?;
    Some(d.tasks.get_mut(id).expect("name indexes live task"))
}
pub(crate) fn snapshot(d: &Domain, capacity: u32) -> List<u64> {
    let mut numbers = List::with_capacity(capacity);
    for (number, _) in &d.names {
        numbers.push(*number).expect("one snapshot entry per live name");
    }
    numbers
}
pub(crate) fn fact(d: &mut Domain, observation: Fact) {
    if d.facts.try_push(observation).is_err() {
        d.lost = d.lost.saturating_add(1);
    }
}
pub(crate) fn refused(to: ReplyTo, task: Option<u64>, why: Refusal, out: &mut Queue<Request>) {
    out.push(Request::Refused { reply_to: to, problem: Problem { task, why } });
}
pub(crate) fn entrance(d: &Domain, to: ReplyTo, number: u64) -> Result<ReplyTo, (ReplyTo, Refusal)> {
    if !d.ready() {
        return Err((to, Refusal::NotReady));
    }
    if !d.names.contains_key(&number) {
        return Err((to, Refusal::Unknown));
    }
    Ok(to)
}
pub(crate) fn activate(d: &Domain, number: u64, out: &mut Queue<Request>) {
    let task = record(d, number).expect("activation names live task");
    out.push(Request::Activate { task: number, executor: task.executor });
}
pub(crate) fn publish(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let task = task_mut(d, number).expect("published task is live");
    let until = match task.record.phase {
        Phase::Active(Active::BackingOff { until }) => Some(until),
        Phase::Waiting
        | Phase::Active(
            Active::Idle | Active::Due | Active::Preparing | Active::Claimed { .. } | Active::Running { .. },
        )
        | Phase::Closing(_)
        | Phase::Held { .. }
        | Phase::Ended(_) => None,
    };
    let alarm = match until {
        Some(until) => match task.alarm {
            Some(old) if old.until == until => Some(old),
            Some(_) | None => Some(Alarm {
                until,
                due: env.now.saturating_add(skein_lib::Duration::from_nanos(
                    until.as_nanos().saturating_sub(env.wall.as_nanos()),
                )),
            }),
        },
        None => None,
    };
    task.alarm = alarm;
    out.push(Request::Save { record: Stored::Live(Box::new(task.record.clone())) });
    match alarm {
        Some(alarm) => {
            let armed = d.alarms.arm(number, alarm.due);
            assert!(armed.is_ok(), "one alarm per live task");
        }
        None => {
            d.alarms.cancel(number);
        }
    }
}
fn make(d: &mut Domain, env: &Env<Limits>, to: ReplyTo, creator: Party, batch: Box<[New]>, out: &mut Queue<Request>) {
    if !d.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    if let Err(problem) = crate::batch::check(d, &env.limits, creator, &batch) {
        return out.push(Request::Refused { reply_to: to, problem });
    }
    let parent = match creator {
        Party::Task(number) => Some(number),
        Party::Person(_) | Party::Deployment { .. } => None,
    };
    let (root, depth) = match parent {
        Some(number) => {
            let parent = record(d, number).expect("creator admitted");
            (Some(parent.root), parent.depth.checked_add(1).expect("depth admitted"))
        }
        None => (None, 0),
    };
    let mut numbers = List::with_capacity(env.limits.batch);
    for new in &batch {
        numbers.push(new.number).expect("batch admitted");
    }
    let count = numbers.len();
    if let Some(number) = parent {
        let mut delegates = List::with_capacity(env.limits.delegates);
        let old = task_mut(d, number).expect("creator admitted");
        for delegate in &old.record.delegates {
            delegates.push(*delegate).expect("existing delegate admitted");
        }
        for new in &batch {
            delegates.push(new.number).expect("new delegates admitted");
        }
        old.record.delegates = delegates.into_boxed();
        // Count each made task in all ancestors, so ending a delegate does not
        // make lifetime tree capacity reappear.
        let mut ancestor = Some(number);
        for _ in 0..env.limits.tasks {
            let Some(number) = ancestor else {
                break;
            };
            let task = task_mut(d, number).expect("all ancestors of live task are live");
            task.record.made = task.record.made.checked_add(count).expect("tree count admitted");
            ancestor = match task.record.requester {
                Party::Task(parent) => Some(parent),
                Party::Person(_) | Party::Deployment { .. } => None,
            };
            publish(d, env, number, out);
        }
    }
    for new in batch {
        let number = new.number;
        let task = Task {
            record: TaskRecord {
                number,
                project: new.project,
                requester: creator,
                root: root.unwrap_or(number),
                depth,
                executor: new.executor,
                spec: new.spec,
                contract: new.contract,
                authority: new.authority,
                numbers: new.numbers,
                funder: new.funder,
                dependencies: new.dependencies,
                delegates: Box::new([]),
                made: 1,
                attempt: 0,
                last_answer: None,
                tries: Tries::NONE,
                refusals: 0,
                phase: Phase::Waiting,
            },
            alarm: None,
        };
        let id = d.tasks.insert(task).expect("batch slab room admitted");
        let indexed = d.names.insert(number, id);
        assert!(indexed == Ok(None), "batch names admitted");
        publish(d, env, number, out);
        fact(d, Fact::Made { task: number, requester: creator });
    }
    out.push(Request::Made { reply_to: to, tasks: numbers.into_boxed() });
}
