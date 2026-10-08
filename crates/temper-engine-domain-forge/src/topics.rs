//! Goal topic facts (jig's domain/connectors.md, 5.4; domain/forge.md, 7).
//! Subtree membership is durable in subscribers. File lists are disposable,
//! bounded and pinned by fresh pull reads around all pages. Missing facts
//! make overlap unknown. A head change refreshes the facts and CI topic.
use crate::{
    Class, Limits, News, Request, Subscriber, Topic,
    domain::{self, Domain},
};
use alloc::boxed::Box;
use skein_lib::{Env, List, Queue, Token};
use temper_engine_domain_forge_change as change;
use temper_engine_domain_forge_client as client;

/// One bounded file list pinned to an open pull's head.
#[derive(Debug)]
pub(crate) struct Files {
    pub head: client::api::Commit,
    pub paths: Option<Box<[Box<[u8]>]>>,
}
#[derive(Clone, Copy, Debug)]
enum Stage {
    Before,
    Page(u32),
    After,
}
/// One coherent pull-file read chain; at most one per live change.
#[derive(Debug)]
pub(crate) struct Pending {
    task: u64,
    repository: client::api::Repository,
    pull: u64,
    head: client::api::Commit,
    stage: Stage,
    paths: List<Box<[u8]>>,
}

pub(crate) fn contains(tasks: &[u64], task: u64) -> bool {
    for number in tasks {
        if *number == task {
            return true;
        }
    }
    false
}

pub(crate) fn subscribe(d: &Domain, subscriber: &Subscriber, out: &mut Queue<Request>) {
    match subscriber.topic {
        Topic::Landings { .. } => {}
        Topic::Ci { .. } | Topic::Pull { .. } | Topic::Participation { .. } => return,
    }
    let Some(tasks) = &subscriber.goal_tasks else { return };
    let mut heads = List::with_capacity(d.subscriptions.capacity());
    for task in tasks {
        let Some(row) = d.changes.get(task) else { continue };
        let Some(head) = row.change.last_head else { continue };
        let topic = Topic::Ci { repository: row.repository, head };
        if !heads.as_slice().contains(&topic)
            && heads.push(topic.clone()).is_ok()
            && !d.subscriptions.contains_key(&(subscriber.task, topic.clone()))
        {
            domain::emit(out, Request::GoalTopic { goal: subscriber.task, topic });
        }
    }
}

pub(crate) fn head_changed(d: &Domain, task: u64, out: &mut Queue<Request>) {
    let Some(row) = d.changes.get(&task) else { return };
    let Some(head) = row.change.last_head else { return };
    let mut goals = List::with_capacity(d.subscriptions.capacity());
    for (_, subscriber) in &d.subscriptions {
        if let Some(tasks) = &subscriber.goal_tasks
            && contains(tasks, task)
        {
            match subscriber.topic {
                Topic::Landings { .. } if !goals.as_slice().contains(&subscriber.task) => {
                    goals.push(subscriber.task).expect("bounded subscribers");
                    let topic = Topic::Ci { repository: row.repository, head };
                    if !d.subscriptions.contains_key(&(subscriber.task, topic.clone())) {
                        domain::emit(out, Request::GoalTopic { goal: subscriber.task, topic });
                    }
                }
                Topic::Landings { .. } | Topic::Ci { .. } | Topic::Pull { .. } | Topic::Participation { .. } => {}
            }
        }
    }
    for (_, subscriber) in &d.subscriptions {
        if !goals.as_slice().contains(&subscriber.task) {
            continue;
        }
        let Some(tasks) = &subscriber.goal_tasks else { continue };
        match subscriber.topic {
            Topic::Ci { repository, head: old_head } if repository == row.repository && old_head != head => {
                let mut current = false;
                for number in tasks {
                    if let Some(change) = d.changes.get(number)
                        && change.repository == repository
                        && change.change.last_head == Some(old_head)
                    {
                        current = true;
                    }
                }
                if !current {
                    domain::emit(out, Request::GoalUntopic { goal: subscriber.task, topic: subscriber.topic.clone() });
                }
            }
            Topic::Ci { .. } | Topic::Landings { .. } | Topic::Pull { .. } | Topic::Participation { .. } => {}
        }
    }
}

pub(crate) fn refresh(d: &mut Domain, env: &Env<Limits>, task: u64, out: &mut Queue<Request>) {
    let Some(row) = d.changes.get(&task) else { return };
    let Some(head) = row.change.last_head else { return };
    let repository = row.repository;
    let pull = row.pull;
    let mut wanted = false;
    for (_, subscriber) in &d.subscriptions {
        if let Some(tasks) = &subscriber.goal_tasks
            && contains(tasks, task)
        {
            match &subscriber.topic {
                Topic::Landings { repository: home, .. } if *home == repository => wanted = true,
                Topic::Landings { .. } | Topic::Ci { .. } | Topic::Pull { .. } | Topic::Participation { .. } => {}
            }
        }
    }
    if !wanted {
        return;
    }
    let Some(pull) = pull else { return };
    if let Some(files) = d.goal_files.get(&task)
        && files.head == head
    {
        return;
    }
    for (_, pending) in &d.goal_file_reads {
        if pending.task == task {
            return;
        }
    }
    if d.goal_file_reads.len() == env.limits.changes {
        return;
    }
    let Some(sequence) = d.sequence.checked_add(1) else { return };
    d.sequence = sequence;
    let owner = Token::new(sequence | (1_u64 << 63));
    let pending = Pending {
        task,
        repository,
        pull,
        head,
        stage: Stage::Before,
        paths: List::with_capacity(env.limits.paths_per_subscription),
    };
    d.goal_file_reads.insert(owner, pending).expect("one read per change");
    domain::child(
        d,
        env,
        client::Event::Read { owner, repository, read: client::api::Read::Pull { number: pull } },
        out,
    );
}

pub(crate) fn answer(
    d: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    result: Result<client::api::Answer, client::api::Error>,
    out: &mut Queue<Request>,
) {
    let Some(mut pending) = d.goal_file_reads.remove(&owner) else { return };
    let next = match pending.stage {
        Stage::Before => match result {
            Ok(client::api::Answer::Pull(pull))
                if pull.number == pending.pull
                    && pull.commit == pending.head
                    && pull.state == client::api::State::Open =>
            {
                pending.stage = Stage::Page(1);
                Some(client::api::Read::PullFiles { number: pending.pull, head: pending.head, page: 1 })
            }
            Ok(_) | Err(_) => None,
        },
        Stage::Page(page) => match result {
            Ok(client::api::Answer::PullFiles { head, files, more }) if head == pending.head => {
                let empty = files.is_empty();
                for file in files {
                    if file.path.len() > usize::try_from(env.limits.name_bytes).expect("u32 fits usize")
                        || pending.paths.push(file.path).is_err()
                    {
                        finish(d, pending, None);
                        return;
                    }
                }
                if more {
                    if page >= env.limits.paths_per_subscription || empty {
                        finish(d, pending, None);
                        return;
                    }
                    let Some(page) = page.checked_add(1) else {
                        finish(d, pending, None);
                        return;
                    };
                    pending.stage = Stage::Page(page);
                    Some(client::api::Read::PullFiles { number: pending.pull, head, page })
                } else {
                    pending.stage = Stage::After;
                    Some(client::api::Read::Pull { number: pending.pull })
                }
            }
            Ok(_) | Err(_) => None,
        },
        Stage::After => {
            let paths = match result {
                Ok(client::api::Answer::Pull(pull))
                    if pull.number == pending.pull
                        && pull.commit == pending.head
                        && pull.state == client::api::State::Open =>
                {
                    Some(core::mem::replace(&mut pending.paths, List::with_capacity(0)).into_boxed())
                }
                Ok(_) | Err(_) => None,
            };
            finish(d, pending, paths);
            return;
        }
    };
    match next {
        Some(read) => {
            let repository = pending.repository;
            d.goal_file_reads.insert(owner, pending).expect("replaces pending read");
            domain::child(d, env, client::Event::Read { owner, repository, read }, out);
        }
        None => finish(d, pending, None),
    }
}
fn finish(d: &mut Domain, pending: Pending, paths: Option<Box<[Box<[u8]>]>>) {
    d.goal_files.insert(pending.task, Files { head: pending.head, paths }).expect("one file list per change");
}

pub(crate) fn classify(d: &Domain, subscriber: &Subscriber, news: &News) -> Class {
    let (after, files) = match news {
        News::Landing { after, files, .. } => (after, files),
        News::Ci { .. } | News::Changed { .. } => return Class::Wakes,
    };
    let Some(tasks) = &subscriber.goal_tasks else { return domain::classify(d, subscriber, news) };
    if let Some(owner) = d.landed.get(after)
        && contains(tasks, *owner)
    {
        return Class::Kept;
    }
    let Some(files) = files else { return Class::Wakes };
    if domain::overlap(files, &subscriber.paths) {
        return Class::Wakes;
    }
    let mut unknown = false;
    for task in tasks {
        let Some(row) = d.changes.get(task) else {
            if subscriber.paths.is_empty() {
                unknown = true;
            }
            continue;
        };
        match row.change.state {
            change::State::Landed { .. } => continue,
            change::State::Producing { .. }
            | change::State::Opening { .. }
            | change::State::Recreating { .. }
            | change::State::Reopening { .. }
            | change::State::Checking { .. }
            | change::State::Gating { .. }
            | change::State::Queued { .. }
            | change::State::First { .. }
            | change::State::Updating { .. }
            | change::State::Resolving { .. }
            | change::State::Repairing { .. }
            | change::State::Landing { .. }
            | change::State::Held { .. } => {}
        }
        if row.pull.is_none() {
            if subscriber.paths.is_empty() {
                unknown = true;
            }
            continue;
        }
        match &subscriber.topic {
            Topic::Landings { repository, branch } if row.repository == *repository && row.base == *branch => {}
            Topic::Landings { .. } | Topic::Ci { .. } | Topic::Pull { .. } | Topic::Participation { .. } => continue,
        }
        let cached = match d.goal_files.get(task) {
            Some(cached) if Some(cached.head) == row.change.last_head => cached.paths.as_ref(),
            Some(_) | None => None,
        };
        match cached {
            Some(paths) if domain::overlap(files, paths) => return Class::Wakes,
            Some(_) => {}
            None => unknown = true,
        }
    }
    if unknown { Class::Wakes } else { Class::Kept }
}
