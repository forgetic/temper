use crate::{Active, Event, Fact, Limits, New, Party, Phase, Problem, Refusal, Request, Stored, TaskRecord, Tries};
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
    pub(crate) alarms: Deadlines<u64>,
    pub(crate) funding: Map<crate::Funder, crate::FundingRecord>,
    pub(crate) charters: Box<[u32]>,
    pub(crate) rng: Rng,
    facts: Queue<Fact>,
    lost: u64,
}

impl Domain {
    #[must_use]
    pub fn new(limits: &Limits, seed: u64, charters: Box<[u32]>) -> Domain {
        assert!(crate::worst_case(limits).is_some(), "task limits are valid");
        assert!(charters.len() <= usize::try_from(limits.charters).expect("u32 fits usize"), "configured charters fit");
        for (at, charter) in charters.iter().enumerate() {
            for earlier in charters.iter().take(at) {
                assert!(earlier != charter, "charters are unique");
            }
        }
        Domain {
            startup: Startup::Restoring,
            tasks: Slab::with_capacity(limits.tasks),
            names: Map::with_capacity(limits.tasks),
            alarms: Deadlines::with_capacity(limits.tasks),
            funding: Map::with_capacity(limits.funders),
            charters,
            rng: Rng::new(seed),
            facts: Queue::with_capacity(limits.facts),
            lost: 0,
        }
    }

    /// Root borrows owned finite accounting for its current authority check;
    /// this query changes nothing and allocates nothing (domain/tasks.md, 2).
    #[must_use]
    pub fn funding(&self, funder: crate::Funder) -> Option<&crate::FundingRecord> {
        self.funding.get(&funder)
    }

    #[must_use]
    pub(crate) fn ready(&self) -> bool {
        self.startup == Startup::Ready
    }

    pub fn reclaim(&mut self) {
        self.tasks.reclaim();
    }

    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }
}

pub(crate) fn output_bound(limits: &Limits) -> Option<u32> {
    limits
        .tasks
        .checked_mul(20)?
        .checked_add(limits.batch.checked_mul(2)?)?
        .checked_add(limits.funders.checked_mul(3)?)?
        .checked_add(8)
}

/// Cascading dependency and closing decisions touch at most the bounded live
/// set; each task advances through a constant number of phases in one step.
#[must_use]
pub fn max_out(limits: &Limits) -> u32 {
    output_bound(limits).expect("task limits admit output bound")
}

pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::OpenPeriod { reply_to, project, period, budget } => {
            crate::funders::open(domain, reply_to, project, period, budget, out);
        }
        Event::CarvePool { reply_to, project, person, period, budget } => {
            crate::funders::carve(domain, reply_to, project, person, period, budget, out);
        }
        Event::Make { reply_to, creator, batch } => make(domain, env, reply_to, creator, batch, out),
        Event::Prepare { reply_to, task } => crate::run::prepare(domain, env, reply_to, task, out),
        Event::Claim { reply_to, task, attempt } => crate::run::claim(domain, env, reply_to, task, attempt, out),
        Event::Turn { reply_to, task, attempt, turn, read, cumulative } => {
            crate::admission::turn(domain, env, reply_to, task, attempt, turn, read, cumulative, out);
        }
        Event::Started { task, attempt } => crate::run::started(domain, env, task, attempt, out),
        Event::Activation { reply_to, task, attempt, end, cause } => match cause {
            crate::Cause::Priced { cumulative } => {
                crate::admission::activation(domain, env, reply_to, task, attempt, end, cumulative, out);
            }
            crate::Cause::Unpriced => crate::run::activation(domain, env, reply_to, task, attempt, end, out),
        },
        Event::PreparationFailed { task } => crate::run::preparation_failed(domain, env, task, out),
        Event::Hold { task, why } => crate::run::hold(domain, env, task, why, out),
        Event::Settled { task } => crate::closing::settled(domain, env, task, out),
        Event::Restore { record } => crate::stored::restore(domain, env, record, out),
        Event::Restored => crate::stored::restored(domain, env, out),
    }
    if domain.ready() {
        crate::closing::progress(domain, env, out);
    }
}

/// One backoff expiration per iteration; a wall-clock correction does not
/// change an already projected monotonic deadline.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if !domain.ready() {
        return;
    }
    if let Some(number) = domain.alarms.expire(env.now)
        && let Some(task) = task_mut(domain, number)
    {
        match task.record.phase {
            Phase::Active(Active::BackingOff { .. }) => {
                task.record.phase = Phase::Active(Active::Due);
                publish(domain, env, number, out);
                activate(domain, number, out);
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
    crate::closing::progress(domain, env, out);
}

pub(crate) fn record(domain: &Domain, number: u64) -> Option<&TaskRecord> {
    Some(&domain.tasks.get(*domain.names.get(&number)?).expect("name indexes live task").record)
}

pub(crate) fn task_mut(domain: &mut Domain, number: u64) -> Option<&mut Task> {
    let id = *domain.names.get(&number)?;
    Some(domain.tasks.get_mut(id).expect("name indexes live task"))
}

pub(crate) fn snapshot(domain: &Domain, capacity: u32) -> List<u64> {
    let mut numbers = List::with_capacity(capacity);
    for (number, _) in &domain.names {
        numbers.push(*number).expect("one snapshot entry per live name");
    }
    numbers
}

pub(crate) fn fact(domain: &mut Domain, observation: Fact) {
    if domain.facts.try_push(observation).is_err() {
        domain.lost = domain.lost.saturating_add(1);
    }
}

pub(crate) fn refused(to: ReplyTo, task: Option<u64>, why: Refusal, out: &mut Queue<Request>) {
    out.push(Request::Refused { reply_to: to, problem: Problem { task, why } });
}

pub(crate) fn entrance(domain: &Domain, to: ReplyTo, number: u64) -> Result<ReplyTo, (ReplyTo, Refusal)> {
    if !domain.ready() {
        return Err((to, Refusal::NotReady));
    }
    if !domain.names.contains_key(&number) {
        return Err((to, Refusal::Unknown));
    }
    Ok(to)
}

pub(crate) fn activate(domain: &Domain, number: u64, out: &mut Queue<Request>) {
    let task = record(domain, number).expect("activation names live task");
    out.push(Request::Activate {
        context: Box::new(crate::RunContext {
            task: number,
            project: task.project,
            executor: task.executor,
            spec: task.spec.clone(),
            contract: task.contract.clone(),
            requester: task.requester,
            authority: task.authority.clone(),
            numbers: task.numbers,
        }),
    });
}

pub(crate) fn publish(domain: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let task = task_mut(domain, number).expect("published task is live");
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
            let armed = domain.alarms.arm(number, alarm.due);
            assert!(armed.is_ok(), "one alarm per live task");
        }
        None => {
            domain.alarms.cancel(number);
        }
    }
}

fn make(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    creator: Party,
    batch: Box<[New]>,
    out: &mut Queue<Request>,
) {
    if !domain.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    if let Err(problem) = crate::batch::check(domain, &env.limits, creator, &batch) {
        return out.push(Request::Refused { reply_to: to, problem });
    }
    crate::funders::reserve(domain, env, &batch, out);
    let parent = match creator {
        Party::Task(number) => Some(number),
        Party::Person(_) | Party::Deployment { .. } => None,
    };
    let (root, depth) = match parent {
        Some(number) => {
            let parent = record(domain, number).expect("creator admitted");
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
        let old = task_mut(domain, number).expect("creator admitted");
        old.record.delegates = append(env.limits.delegates, &old.record.delegates, &batch);
        // Count each made task in all ancestors, so ending a delegate does not
        // make lifetime tree capacity reappear.
        let mut ancestor = Some(number);
        for _ in 0..env.limits.tasks {
            let Some(number) = ancestor else {
                break;
            };
            let task = task_mut(domain, number).expect("all ancestors of live task are live");
            task.record.made = task.record.made.checked_add(count).expect("tree count admitted");
            ancestor = match task.record.requester {
                Party::Task(parent) => Some(parent),
                Party::Person(_) | Party::Deployment { .. } => None,
            };
            publish(domain, env, number, out);
        }
    }
    for new in batch {
        let number = new.number;
        let task = Task {
            record: TaskRecord {
                number,
                project: new.project,
                requester: creator,
                allotment: 1,
                historical_spend: 0,
                run_spent: 0,
                root: root.unwrap_or(number),
                depth,
                executor: new.executor,
                spec: new.spec,
                contract: new.contract,
                authority: new.authority,
                numbers: new.numbers,
                funder: new.funder,
                waiting_on: new.dependencies.clone(),
                dependencies: new.dependencies,
                delegates: Box::new([]),
                turn: 0,
                made: 1,
                attempt: 0,
                last_answer: None,
                tries: Tries::NONE,
                refusals: 0,
                phase: Phase::Waiting,
            },
            alarm: None,
        };
        let id = domain.tasks.insert(task).expect("batch slab room admitted");
        let indexed = domain.names.insert(number, id);
        assert!(indexed == Ok(None), "batch names admitted");
        publish(domain, env, number, out);
        fact(domain, Fact::Made { task: number, requester: creator });
    }
    out.push(Request::Made { reply_to: to, tasks: numbers.into_boxed() });
}

fn append(capacity: u32, old: &[u64], batch: &[New]) -> Box<[u64]> {
    let mut numbers = List::with_capacity(capacity);
    for number in old {
        numbers.push(*number).expect("existing numbers admitted");
    }
    for new in batch {
        numbers.push(new.number).expect("new numbers admitted");
    }
    numbers.into_boxed()
}
