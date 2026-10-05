//! Cold admission of bounded live rows and complete link validation
//! (domain/tasks.md, sections 2, 5 and 14). Only Live/Ledger enter startup;
//! historical Ended/Closure stay in root storage. Successful restoration
//! reconstructs activations, claims, closing gates and projected retry timers;
//! failed restoration never becomes ready through more input.
use crate::domain::{Domain, Startup, Task, activate, publish, record, snapshot, task_mut};
use crate::{
    Active, Closing, Ending, Executor, Limits, Party, Phase, Problem, Refusal, Request, Stage, Stored, TaskRecord, Was,
};
use skein_lib::{Env, Queue};

fn valid_ending(limits: &Limits, ending: &Ending) -> bool {
    let cap = usize::try_from(limits.result_bytes).expect("u32 fits usize");
    match ending {
        Ending::Done(result) => crate::run::result_bytes(result) <= cap,
        Ending::Failed { reason } => reason.len() <= cap,
        Ending::Cancelled { reason, result } => {
            if reason.len() > cap {
                return false;
            }
            match result {
                Some(result) => crate::run::result_bytes(result) <= cap,
                None => true,
            }
        }
    }
}

fn valid_stage(task: &TaskRecord, closing: &Closing, limits: &Limits) -> bool {
    valid_ending(limits, &closing.ending)
        && match &closing.ending {
            Ending::Done(result) | Ending::Cancelled { result: Some(result), .. } => {
                crate::run::valid_result(&task.contract, result, limits)
            }
            Ending::Failed { .. } | Ending::Cancelled { result: None, .. } => true,
        }
        && match closing.stage {
            Stage::Run { attempt } => attempt != 0 && attempt == task.attempt && task.last_answer != Some(attempt),
            Stage::Delegates => true,
            Stage::Effects | Stage::Settled => task.delegates.is_empty(),
        }
}

fn valid_phase(task: &TaskRecord, limits: &Limits) -> bool {
    match &task.phase {
        Phase::Active(Active::Claimed { attempt } | Active::Running { attempt })
        | Phase::Held { was: Was::Active(Active::Claimed { attempt } | Active::Running { attempt }), .. } => {
            *attempt != 0 && *attempt == task.attempt && task.last_answer != Some(*attempt)
        }
        Phase::Closing(closing) => closing.stage != Stage::Settled && valid_stage(task, closing, limits),
        Phase::Held { was: Was::Closing(closing), .. } => valid_stage(task, closing, limits),
        Phase::Waiting
        | Phase::Active(Active::Idle | Active::Due | Active::Preparing | Active::BackingOff { .. })
        | Phase::Held {
            was: Was::Waiting | Was::Active(Active::Idle | Active::Due | Active::Preparing | Active::BackingOff { .. }),
            ..
        } => true,
        Phase::Ended(_) => false,
    }
}

fn valid_escalation(task: &TaskRecord, limits: &Limits) -> bool {
    let held = match task.phase {
        Phase::Held { .. } => true,
        Phase::Waiting | Phase::Active(_) | Phase::Closing(_) | Phase::Ended(_) => false,
    };
    match &task.escalation {
        crate::Escalation::Unheld { revision } => {
            *revision != u64::MAX
                && match task.requester {
                    Party::Person(_) => !held,
                    Party::Task(_) | Party::Deployment { .. } => *revision == 0,
                }
        }
        crate::Escalation::Routing { .. } => false,
        crate::Escalation::Waiting { revision, holder } => {
            *revision != 0
                && held
                && match task.requester {
                    Party::Person(requester) => match holder {
                        crate::EscalationHolder::Person(person) => *person != 0 && *person == requester,
                        crate::EscalationHolder::Role { project, .. } => *project == task.project,
                    },
                    Party::Task(_) | Party::Deployment { .. } => false,
                }
        }
        crate::Escalation::Rejected { revision, by, reason } => {
            *revision != 0
                && *by != 0
                && held
                && match task.requester {
                    Party::Person(_) => true,
                    Party::Task(_) | Party::Deployment { .. } => false,
                }
                && reason.len() <= usize::try_from(limits.result_bytes).expect("u32 fits usize")
        }
    }
}

fn valid_record(domain: &Domain, limits: &Limits, task: &TaskRecord) -> bool {
    if match task.historical_spend.checked_add(task.numbers.spent) {
        Some(total) => task.run_spent > total,
        None => true,
    } || crate::funders::total(task.numbers).is_none()
        || task.allotment == 0
        || task.depth > limits.depth
        || task.made == 0
        || task.made > limits.tree_tasks
        || task.delegates.len() > usize::try_from(limits.delegates).expect("u32 fits usize")
        || task.waiting_on.len() > usize::try_from(limits.dependencies).expect("u32 fits usize")
        || task.dependencies.len() > usize::try_from(limits.dependencies).expect("u32 fits usize")
    {
        return false;
    }
    if !crate::batch::valid_spec(limits, &task.spec)
        || !crate::batch::valid_contract(limits, &task.contract)
        || !crate::batch::valid_authority(limits, &task.authority)
        || !valid_phase(task, limits)
        || !valid_escalation(task, limits)
    {
        return false;
    }
    if task.turn != 0 && task.attempt == 0 {
        return false;
    }
    if let Some(attempt) = task.last_answer
        && (attempt == 0 || attempt > task.attempt)
    {
        return false;
    }
    for numbers in [&*task.delegates, &*task.dependencies, &*task.spec.inputs, &*task.waiting_on] {
        for (at, number) in numbers.iter().enumerate() {
            for earlier in numbers.iter().take(at) {
                if earlier == number {
                    return false;
                }
            }
        }
    }
    for number in &task.waiting_on {
        if !crate::batch::contains(&task.dependencies, *number) {
            return false;
        }
    }
    match task.executor {
        Executor::Agent { charter } => {
            for configured in &domain.charters {
                if *configured == charter {
                    return true;
                }
            }
            false
        }
    }
}

pub(crate) fn restore(domain: &mut Domain, env: &Env<Limits>, stored: Stored, out: &mut Queue<Request>) {
    match domain.startup {
        Startup::Ready => return failed(domain, None, Refusal::NotReady, out),
        Startup::Failed => return,
        Startup::Restoring => {}
    }
    match stored {
        Stored::Live(task) => {
            let number = task.number;
            if domain.names.contains_key(&number) || domain.tasks.is_full() || !valid_record(domain, &env.limits, &task)
            {
                return failed(domain, Some(number), Refusal::Restore, out);
            }
            let id = domain.tasks.insert(Task { record: *task, alarm: None }).expect("restored task admitted");
            let indexed = domain.names.insert(number, id);
            assert!(indexed == Ok(None), "restored name admitted");
        }
        // Historical ended rows stay outside the live arena;
        // they cannot accidentally return an ended task to the live arena.
        Stored::Ended(task) => failed(domain, Some(task.number), Refusal::Restore, out),
        Stored::Ledger(record) => {
            if !crate::funders::restore_funding(domain, record) {
                failed(domain, None, Refusal::Restore, out);
            }
        }
        Stored::Closure(_) => failed(domain, None, Refusal::Restore, out),
    }
}

fn failed(domain: &mut Domain, task: Option<u64>, why: Refusal, out: &mut Queue<Request>) {
    domain.startup = Startup::Failed;
    out.push(Request::RestoreRefused { problem: Problem { task, why } });
}

fn links(domain: &Domain, env: &Env<Limits>, task: &TaskRecord) -> bool {
    match task.requester {
        Party::Task(number) => {
            let Some(parent) = record(domain, number) else {
                return false;
            };
            if task.project != parent.project
                || task.root != parent.root
                || parent.depth.checked_add(1) != Some(task.depth)
                || !crate::batch::contains(&parent.delegates, task.number)
            {
                return false;
            }
        }
        Party::Person(_) | Party::Deployment { .. } => {
            if task.root != task.number || task.depth != 0 {
                return false;
            }
        }
    }
    match task.requester {
        Party::Deployment { project } if project != task.project => return false,
        Party::Deployment { .. } | Party::Task(_) | Party::Person(_) => {}
    }
    let Some(root) = record(domain, task.root) else {
        return false;
    };
    if root.depth != 0 || root.made < task.made {
        return false;
    }
    let mut project_count = 0_u32;
    let mut subtree = 0_u32;
    for (_, id) in &domain.names {
        let other = &domain.tasks.get(*id).expect("name indexes task").record;
        if other.project == task.project {
            project_count = project_count.saturating_add(1);
        }
        if other.root == task.root {
            subtree = subtree.saturating_add(1);
        }
    }
    if project_count > env.limits.project_tasks || subtree > root.made {
        return false;
    }
    let mut descendants_made = 1_u32;
    for delegate in &task.delegates {
        let Some(child) = record(domain, *delegate) else {
            return false;
        };
        let Some(total) = descendants_made.checked_add(child.made) else {
            return false;
        };
        descendants_made = total;
        if child.requester != Party::Task(task.number) {
            return false;
        }
    }
    if descendants_made > task.made {
        return false;
    }
    for dependency in &task.waiting_on {
        if !crate::batch::contains(&task.dependencies, *dependency) {
            return false;
        }
        let Some(other) = record(domain, *dependency) else {
            return false;
        };
        if other.project != task.project {
            return false;
        }
    }
    for dependency in &task.dependencies {
        if let Some(other) = record(domain, *dependency)
            && (other.project != task.project || !crate::batch::contains(&task.waiting_on, *dependency))
        {
            return false;
        }
    }
    let engaged = match &task.phase {
        Phase::Active(_) | Phase::Held { was: Was::Active(_), .. } => true,
        Phase::Waiting
        | Phase::Closing(_)
        | Phase::Held { was: Was::Waiting | Was::Closing(_), .. }
        | Phase::Ended(_) => false,
    };
    if engaged && !task.waiting_on.is_empty() {
        return false;
    }
    if !task.spec.inputs.is_empty() {
        return false;
    }
    true
}

pub(crate) fn restored(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    match domain.startup {
        Startup::Ready | Startup::Failed => return,
        Startup::Restoring => {}
    }
    for (number, _) in &domain.names {
        if !links(domain, env, record(domain, *number).expect("name indexes task"))
            || !crate::funders::representable(domain, *number, 0)
        {
            return failed(domain, Some(*number), Refusal::Restore, out);
        }
    }
    if !crate::batch::acyclic(domain, &env.limits, Party::Person(0), &[])
        || !crate::funders::links(domain, env.limits.tasks)
    {
        return failed(domain, None, Refusal::Restore, out);
    }
    domain.startup = Startup::Ready;
    let numbers = snapshot(domain, env.limits.tasks);
    for number in numbers.into_boxed() {
        let phase = record(domain, number).expect("restored name exists").phase.clone();
        match record(domain, number).expect("restored name exists").escalation {
            crate::Escalation::Waiting { .. } => out.push(Request::EscalationNeeded {
                context: crate::escalation::context(domain, number).expect("valid held person context"),
            }),
            crate::Escalation::Unheld { .. }
            | crate::Escalation::Routing { .. }
            | crate::Escalation::Rejected { .. } => {}
        }
        match phase {
            Phase::Active(Active::Due | Active::Preparing) => {
                task_mut(domain, number).expect("restored name exists").record.phase = Phase::Active(Active::Due);
                publish(domain, env, number, out);
                activate(domain, number, out);
            }
            Phase::Active(Active::Claimed { attempt } | Active::Running { attempt }) => {
                out.push(Request::Adopt {
                    task: number,
                    attempt,
                    kept: record(domain, number).expect("restored live task").turn,
                });
            }
            Phase::Active(Active::BackingOff { .. }) => publish(domain, env, number, out),
            Phase::Closing(closing) => match closing.stage {
                Stage::Run { attempt } => {
                    out.push(Request::Adopt {
                        task: number,
                        attempt,
                        kept: record(domain, number).expect("restored live task").turn,
                    });
                    out.push(Request::Stop { task: number, attempt });
                }
                Stage::Effects => out.push(Request::Close { task: number, ending: closing.ending }),
                Stage::Delegates | Stage::Settled => {}
            },
            Phase::Held { was, .. } => match was {
                Was::Active(Active::Claimed { attempt } | Active::Running { attempt })
                | Was::Closing(Closing { stage: Stage::Run { attempt }, .. }) => {
                    out.push(Request::Adopt {
                        task: number,
                        attempt,
                        kept: record(domain, number).expect("restored live task").turn,
                    });
                    out.push(Request::Stop { task: number, attempt });
                }
                Was::Closing(Closing { stage: Stage::Effects, ending }) => {
                    out.push(Request::Close { task: number, ending });
                }
                Was::Waiting
                | Was::Active(Active::Idle | Active::Due | Active::Preparing | Active::BackingOff { .. })
                | Was::Closing(Closing { stage: Stage::Delegates | Stage::Settled, .. }) => {}
            },
            Phase::Waiting | Phase::Active(Active::Idle) => {}
            Phase::Ended(_) => unreachable!("ended record not restored into live arena"),
        }
    }
}
