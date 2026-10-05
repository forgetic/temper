use crate::domain::{Domain, Startup, Task, activate, publish, record, refused, snapshot, task_mut};
use crate::{
    Active, Closing, Ending, Executor, Key, Limits, Party, Phase, Problem, Refusal, Request, Stage, Stored, Stub,
    TaskRecord, Was,
};
use skein_lib::{Env, Queue, ReplyTo};
pub(crate) fn remember(d: &mut Domain, env: &Env<Limits>, to: ReplyTo, stub: Stub, out: &mut Queue<Request>) {
    if !d.ready() {
        return refused(to, Some(stub.number), Refusal::NotReady, out);
    }
    if d.names.contains_key(&stub.number) || !valid_stub(&stub) {
        return refused(to, Some(stub.number), Refusal::Restore, out);
    }
    if let Some(old) = d.stubs.get(&stub.number) {
        if old == &stub {
            return out.push(Request::Done { reply_to: to });
        }
        return refused(to, Some(stub.number), Refusal::Duplicate, out);
    }
    if d.stubs.len().saturating_add(d.names.len()) >= env.limits.stubs {
        return refused(to, Some(stub.number), Refusal::Busy, out);
    }
    let saved = d.stubs.insert(stub.number, stub);
    assert!(saved == Ok(None), "remembered stub admitted");
    out.push(Request::Save { record: Stored::Stub(stub) });
    out.push(Request::Done { reply_to: to });
}
pub(crate) fn forget(d: &mut Domain, to: ReplyTo, number: u64, out: &mut Queue<Request>) {
    if !d.ready() {
        return refused(to, Some(number), Refusal::NotReady, out);
    }
    if crate::closing::needed(d, number) {
        return refused(to, Some(number), Refusal::Busy, out);
    }
    if d.stubs.remove(&number).is_some() {
        out.push(Request::Erase { key: Key::Stub(number) });
    }
    out.push(Request::Done { reply_to: to });
}
fn valid_stub(stub: &Stub) -> bool {
    match stub.last_answer {
        Some(attempt) => attempt != 0 && attempt <= stub.attempt,
        None => true,
    }
}
fn valid_ending(l: &Limits, ending: &Ending) -> bool {
    let cap = usize::try_from(l.result_bytes).expect("u32 fits usize");
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
fn valid_stage(task: &TaskRecord, closing: &Closing, l: &Limits) -> bool {
    valid_ending(l, &closing.ending)
        && match &closing.ending {
            Ending::Done(result) | Ending::Cancelled { result: Some(result), .. } => {
                crate::run::valid_result(&task.contract, result, l)
            }
            Ending::Failed { .. } | Ending::Cancelled { result: None, .. } => true,
        }
        && match closing.stage {
            Stage::Run { attempt } => attempt != 0 && attempt == task.attempt && task.last_answer != Some(attempt),
            Stage::Delegates => true,
            Stage::Effects | Stage::Settled => task.delegates.is_empty() && task.results_due.is_empty(),
        }
}
fn valid_phase(task: &TaskRecord, l: &Limits) -> bool {
    match &task.phase {
        Phase::Active(Active::Claimed { attempt } | Active::Running { attempt })
        | Phase::Held { was: Was::Active(Active::Claimed { attempt } | Active::Running { attempt }), .. } => {
            *attempt != 0 && *attempt == task.attempt && task.last_answer != Some(*attempt)
        }
        Phase::Closing(closing) | Phase::Held { was: Was::Closing(closing), .. } => valid_stage(task, closing, l),
        Phase::Waiting
        | Phase::Active(Active::Idle | Active::Due | Active::Preparing | Active::BackingOff { .. })
        | Phase::Held {
            was: Was::Waiting | Was::Active(Active::Idle | Active::Due | Active::Preparing | Active::BackingOff { .. }),
            ..
        } => true,
        Phase::Ended(_) => false,
    }
}
fn valid_record(d: &Domain, l: &Limits, task: &TaskRecord) -> bool {
    if match task.historical_spend.checked_add(task.numbers.spent) {
        Some(total) => task.run_spent > total,
        None => true,
    } || crate::funders::total(task.numbers).is_none()
        || task.allotment == 0
        || (task.narrowing && crate::run::run_attempt(&task.phase).is_none())
        || task.depth > l.depth
        || task.made == 0
        || task.made > l.tree_tasks
        || task.delegates.len() > usize::try_from(l.delegates).expect("u32 fits usize")
        || !crate::wake::valid(&task.policy)
        || task.references.len() > usize::try_from(l.references).expect("u32 fits usize")
        || task.results_due.len() > usize::try_from(l.delegates).expect("u32 fits usize")
        || task.dependencies.len() > usize::try_from(l.dependencies).expect("u32 fits usize")
    {
        return false;
    }
    if !crate::batch::valid_spec(l, &task.spec)
        || !crate::batch::valid_contract(l, &task.contract)
        || !crate::batch::valid_authority(l, &task.authority)
        || !valid_phase(task, l)
    {
        return false;
    }
    if task.turn != 0 && task.attempt == 0 {
        return false;
    }
    if let Some(read) = task.last_read
        && (task.turn == 0 || read == 0 || read > task.last_message)
    {
        return false;
    }
    if let Some(attempt) = task.last_answer
        && (attempt == 0 || attempt > task.attempt)
    {
        return false;
    }
    for numbers in [&*task.delegates, &*task.dependencies, &*task.spec.inputs, &*task.references, &*task.results_due] {
        for (at, number) in numbers.iter().enumerate() {
            for earlier in numbers.iter().take(at) {
                if earlier == number {
                    return false;
                }
            }
        }
    }
    match task.executor {
        Executor::Agent { charter } => {
            for configured in &d.charters {
                if *configured == charter {
                    return true;
                }
            }
            false
        }
    }
}
pub(crate) fn restore(d: &mut Domain, env: &Env<Limits>, stored: Stored, out: &mut Queue<Request>) {
    match d.startup {
        Startup::Ready => return failed(d, None, Refusal::NotReady, out),
        Startup::Failed => return,
        Startup::Restoring => {}
    }
    match stored {
        Stored::Live(task) => {
            let number = task.number;
            if d.names.contains_key(&number)
                || d.stubs.contains_key(&number)
                || d.tasks.is_full()
                || d.names.len().saturating_add(d.stubs.len()) >= env.limits.stubs
                || !valid_record(d, &env.limits, &task)
            {
                return failed(d, Some(number), Refusal::Restore, out);
            }
            let id = d
                .tasks
                .insert(Task { record: *task, alarm: None, wake_alarms: [None; 3] })
                .expect("restored task admitted");
            let indexed = d.names.insert(number, id);
            assert!(indexed == Ok(None), "restored name admitted");
        }
        Stored::Stub(stub) => {
            if d.names.contains_key(&stub.number)
                || d.stubs.contains_key(&stub.number)
                || !valid_stub(&stub)
                || d.names.len().saturating_add(d.stubs.len()) >= env.limits.stubs
            {
                return failed(d, Some(stub.number), Refusal::Restore, out);
            }
            let saved = d.stubs.insert(stub.number, stub);
            assert!(saved == Ok(None), "restored stub admitted");
        }
        // Historical ended rows are read through RememberStub at runtime;
        // they cannot accidentally return an ended task to the live arena.
        Stored::Ended(task) => failed(d, Some(task.number), Refusal::Restore, out),
        Stored::History(_) | Stored::Closure(_) | Stored::Funding { .. } | Stored::ArchivedMessage(_) => {
            failed(d, None, Refusal::Restore, out);
        }
        Stored::Message(_) | Stored::Offer(_) | Stored::Receipt(_) | Stored::Question(_) | Stored::Subscription(_) => {
            if !crate::inbox::restore(d, env, stored) {
                failed(d, None, Refusal::Restore, out);
            }
        }
    }
}
fn failed(d: &mut Domain, task: Option<u64>, why: Refusal, out: &mut Queue<Request>) {
    d.startup = Startup::Failed;
    out.push(Request::RestoreRefused { problem: Problem { task, why } });
}
fn links(d: &Domain, env: &Env<Limits>, task: &TaskRecord) -> bool {
    match task.requester {
        Party::Task(number) => {
            let Some(parent) = record(d, number) else {
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
    let Some(root) = record(d, task.root) else {
        return false;
    };
    if root.depth != 0 || root.made < task.made {
        return false;
    }
    let mut project_count = 0_u32;
    let mut subtree = 0_u32;
    for (_, id) in &d.names {
        let other = &d.tasks.get(*id).expect("name indexes task").record;
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
        let Some(child) = record(d, *delegate) else {
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
    for dependency in &task.dependencies {
        if let Some(other) = record(d, *dependency) {
            if other.project != task.project {
                return false;
            }
        } else if let Some(stub) = d.stubs.get(dependency) {
            if stub.project != task.project {
                return false;
            }
        } else {
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
    if engaged {
        for dependency in &task.dependencies {
            if d.names.contains_key(dependency) {
                return false;
            }
            match d.stubs.get(dependency) {
                Some(stub) if stub.status == crate::Status::Done => {}
                Some(_) | None => return false,
            }
        }
    }
    if !reference_links(d, task) {
        return false;
    }
    for input in &task.spec.inputs {
        if !d.stubs.contains_key(input) {
            return false;
        }
    }
    true
}
pub(crate) fn restored(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    match d.startup {
        Startup::Ready | Startup::Failed => return,
        Startup::Restoring => {}
    }
    for (number, _) in &d.names {
        if !links(d, env, record(d, *number).expect("name indexes task")) {
            return failed(d, Some(*number), Refusal::Restore, out);
        }
    }
    if !crate::batch::acyclic(d, &env.limits, Party::Person(0), &[])
        || !crate::inbox::links(d, env)
        || !crate::funders::links(d, env.limits.tasks)
    {
        return failed(d, None, Refusal::Restore, out);
    }
    d.startup = Startup::Ready;
    let numbers = snapshot(d, env.limits.tasks);
    for number in numbers.into_boxed() {
        let phase = record(d, number).expect("restored name exists").phase.clone();
        match phase {
            Phase::Active(Active::Due | Active::Preparing) => {
                task_mut(d, number).expect("restored name exists").record.phase = Phase::Active(Active::Due);
                publish(d, env, number, out);
                activate(d, number, out);
            }
            Phase::Active(Active::Claimed { attempt } | Active::Running { attempt }) => {
                out.push(Request::Adopt { task: number, attempt });
                if record(d, number).expect("restored task live").narrowing {
                    out.push(Request::Stop { task: number, attempt });
                }
            }
            Phase::Active(Active::BackingOff { .. }) => publish(d, env, number, out),
            Phase::Closing(closing) => match closing.stage {
                Stage::Run { attempt } => {
                    out.push(Request::Adopt { task: number, attempt });
                    out.push(Request::Stop { task: number, attempt });
                }
                Stage::Effects => out.push(Request::Close { task: number, ending: closing.ending }),
                Stage::Delegates | Stage::Settled => {}
            },
            Phase::Held { was, .. } => match was {
                Was::Active(Active::Claimed { attempt } | Active::Running { attempt })
                | Was::Closing(Closing { stage: Stage::Run { attempt }, .. }) => {
                    out.push(Request::Adopt { task: number, attempt });
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
    crate::refs::restored(d, env, out);
    for (_, offer) in &d.offers {
        out.push(Request::Relay {
            task: offer.envelope.task,
            attempt: offer.attempt,
            envelope: offer.envelope.clone(),
        });
    }
}

fn reference_links(d: &Domain, task: &TaskRecord) -> bool {
    for reference in &task.references {
        if *reference == task.number {
            return false;
        }
        match record(d, *reference) {
            Some(other) => {
                if other.project != task.project {
                    return false;
                }
            }
            None => match d.stubs.get(reference) {
                Some(stub) => {
                    if stub.project != task.project {
                        return false;
                    }
                }
                None => return false,
            },
        }
    }
    true
}
