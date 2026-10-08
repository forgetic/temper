//! One level-triggered issue projection step (domain/forge.md, section 12).
//!
//! The top commits the returned `Projected` and `Effect` together. A body
//! digest and write time throttle only changed bodies; milestone keys are
//! retained independently and emitted once. No issue is read back, and the
//! symbolic goal on an effect is resolved to its issue by the top.
use alloc::boxed::Box;
use sha2::{Digest, Sha256};
use skein_lib::{List, Wall};

use crate::{Decision, Effect, GoalView, Key, Limits, MilestoneKey, Phase, Projected, Projection};

/// Project one goal from its current committed record and root facts.
#[must_use]
pub fn project(goal: &GoalView, last: &Projected, now: Wall, limits: &Limits) -> Projection {
    if goal.plan.len() > usize::try_from(limits.plan_items).expect("u32 fits usize")
        || goal.milestones.len() > usize::try_from(limits.milestones).expect("u32 fits usize")
        || goal.title.len() > usize::try_from(limits.title_bytes).expect("u32 fits usize")
        || last.milestones.len() > usize::try_from(limits.milestones).expect("u32 fits usize")
        || !valid_history(goal, last, limits)
    {
        return held(last);
    }
    let Some(body) = body(goal, limits) else { return held(last) };
    let mut digest = Sha256::new();
    digest.update(goal.title.as_bytes());
    digest.update(&body);
    let hash: [u8; 32] = digest.finalize().into();
    if last.closed {
        let mut complete = has(&last.milestones, MilestoneKey::Finished);
        for milestone in &goal.milestones {
            if !has(&last.milestones, milestone.key) {
                complete = false;
            }
        }
        return if last.digest == Some(hash) && goal.finished.is_some() && complete {
            Projection { projected: last.clone(), decision: Decision::None }
        } else {
            held(last)
        };
    }
    if !last.opened {
        let mut next = last.clone();
        next.opened = true;
        next.digest = Some(hash);
        next.last_body = Some(now);
        return Projection {
            projected: next,
            decision: Decision::Effect(Effect::Open {
                goal: goal.goal,
                repository: goal.repository,
                key: Key::Open,
                title: goal.title.clone(),
                body,
            }),
        };
    }
    if last.digest != Some(hash) {
        let since = last.last_body.unwrap_or(now);
        let due = Wall::from_nanos(since.as_nanos().saturating_add(limits.interval.as_nanos()));
        if now >= due {
            let Some(revision) = last.body_revision.checked_add(1) else { return held(last) };
            let mut next = last.clone();
            next.digest = Some(hash);
            next.last_body = Some(now);
            next.body_revision = revision;
            return Projection {
                projected: next,
                decision: Decision::Effect(Effect::Body {
                    goal: goal.goal,
                    repository: goal.repository,
                    key: Key::Body(revision),
                    title: goal.title.clone(),
                    body,
                }),
            };
        }
    }
    for milestone in &goal.milestones {
        if !has(&last.milestones, milestone.key) {
            return comment(goal, last, milestone.key, milestone.text.clone(), limits);
        }
    }
    if let Some(finished) = &goal.finished {
        if last.digest != Some(hash) {
            let since = last.last_body.unwrap_or(now);
            return Projection {
                projected: last.clone(),
                decision: Decision::Wait(Wall::from_nanos(since.as_nanos().saturating_add(limits.interval.as_nanos()))),
            };
        }
        if !has(&last.milestones, MilestoneKey::Finished) {
            return comment(goal, last, MilestoneKey::Finished, finished.clone(), limits);
        }
        let mut next = last.clone();
        next.closed = true;
        return Projection {
            projected: next,
            decision: Decision::Effect(Effect::Close { goal: goal.goal, repository: goal.repository, key: Key::Close }),
        };
    }
    if last.digest != Some(hash) {
        let since = last.last_body.unwrap_or(now);
        return Projection {
            projected: last.clone(),
            decision: Decision::Wait(Wall::from_nanos(since.as_nanos().saturating_add(limits.interval.as_nanos()))),
        };
    }
    Projection { projected: last.clone(), decision: Decision::None }
}

/// Retain immutable milestone history while replacing the goal and its whole plan.
#[must_use]
pub fn gather(prior: Option<&GoalView>, mut next: GoalView, limits: &Limits) -> Option<GoalView> {
    let mut milestones = List::with_capacity(limits.milestones);
    if let Some(prior) = prior {
        if prior.goal != next.goal
            || prior.repository != next.repository
            || (prior.finished.is_some() && prior.finished != next.finished)
        {
            return None;
        }
        for item in &prior.milestones {
            milestones.push(item.clone()).ok()?;
        }
    }
    for item in &next.milestones {
        let mut found = false;
        for kept in &milestones {
            if kept.key == item.key {
                if kept != item {
                    return None;
                }
                found = true;
            }
        }
        if !found {
            milestones.push(item.clone()).ok()?;
        }
    }
    next.milestones = milestones.into_boxed();
    if next.plan.len() > usize::try_from(limits.plan_items).ok()?
        || next.title.len() > usize::try_from(limits.title_bytes).ok()?
        || body(&next, limits).is_none()
    {
        return None;
    }
    for item in &next.milestones {
        if item.text.len() > usize::try_from(limits.comment_bytes).ok()? {
            return None;
        }
    }
    if !valid_history(&next, &Projected::default(), limits) {
        return None;
    }
    Some(next)
}

fn held(last: &Projected) -> Projection {
    Projection { projected: last.clone(), decision: Decision::Hold }
}

fn has(keys: &[MilestoneKey], key: MilestoneKey) -> bool {
    for item in keys {
        if *item == key {
            return true;
        }
    }
    false
}

fn valid_history(goal: &GoalView, last: &Projected, limits: &Limits) -> bool {
    for item in &goal.milestones {
        if item.key == MilestoneKey::Finished {
            return false;
        }
        let mut found = false;
        for previous in &goal.milestones {
            if previous.key == item.key {
                if found {
                    return false;
                }
                found = true;
            }
        }
    }
    for key in &last.milestones {
        if *key != MilestoneKey::Finished {
            let mut found = false;
            for item in &goal.milestones {
                if item.key == *key {
                    found = true;
                }
            }
            if !found {
                return false;
            }
        }
    }
    if goal.finished.is_none() && has(&last.milestones, MilestoneKey::Finished) {
        return false;
    }
    match goal.milestones.len().checked_add(usize::from(goal.finished.is_some())) {
        Some(count) => count <= usize::try_from(limits.milestones).expect("u32 fits usize"),
        None => false,
    }
}

fn body(goal: &GoalView, limits: &Limits) -> Option<Box<[u8]>> {
    let mut bytes = goal.goal_text.len().checked_add(9)?.checked_add(label(goal.phase).len())?;
    for item in &goal.plan {
        bytes = bytes.checked_add(10)?.checked_add(item.text.len())?.checked_add(label(item.phase).len())?;
    }
    if bytes > usize::try_from(limits.body_bytes).expect("u32 fits usize") {
        return None;
    }
    let mut rendered = List::with_capacity(limits.body_bytes);
    append(&mut rendered, goal.goal_text.as_bytes());
    append(&mut rendered, b"\nState: ");
    append(&mut rendered, label(goal.phase));
    rendered.push(b'\n').expect("body size checked");
    for item in &goal.plan {
        let done = match item.phase {
            Phase::Done | Phase::Failed | Phase::Cancelled => true,
            Phase::Waiting | Phase::Active | Phase::Settling | Phase::Held => false,
        };
        append(&mut rendered, if done { b"- [x] " } else { b"- [ ] " });
        append(&mut rendered, item.text.as_bytes());
        append(&mut rendered, b" (");
        append(&mut rendered, label(item.phase));
        append(&mut rendered, b")");
        rendered.push(b'\n').expect("body size checked");
    }
    Some(rendered.into_boxed())
}

fn label(phase: Phase) -> &'static [u8] {
    match phase {
        Phase::Waiting => b"waiting",
        Phase::Active => b"active",
        Phase::Settling => b"settling",
        Phase::Held => b"held",
        Phase::Done => b"done",
        Phase::Failed => b"failed",
        Phase::Cancelled => b"cancelled",
    }
}

fn append(into: &mut List<u8>, bytes: &[u8]) {
    for byte in bytes {
        into.push(*byte).expect("body size checked");
    }
}

fn comment(goal: &GoalView, last: &Projected, key: MilestoneKey, words: Box<str>, limits: &Limits) -> Projection {
    if words.len() > usize::try_from(limits.comment_bytes).expect("u32 fits usize")
        || last.milestones.len() >= usize::try_from(limits.milestones).expect("u32 fits usize")
    {
        return held(last);
    }
    let mut keys = List::with_capacity(limits.milestones);
    for prior in &last.milestones {
        keys.push(*prior).expect("retained milestone bound checked");
    }
    keys.push(key).expect("new milestone has room");
    let mut next = last.clone();
    next.milestones = keys.into_boxed();
    Projection {
        projected: next,
        decision: Decision::Effect(Effect::Comment {
            goal: goal.goal,
            repository: goal.repository,
            key: Key::Milestone(key),
            body: words,
        }),
    }
}
