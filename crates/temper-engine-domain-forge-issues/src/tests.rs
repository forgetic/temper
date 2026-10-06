//! Projection cells, intervals, bounds and keyed history.
use alloc::boxed::Box;
use skein_lib::{Duration, Wall, bytes::copy_of};

use crate::{Decision, Effect, GoalView, Key, Limits, Milestone, MilestoneKey, PlanItem, Projected, project};

const LIMITS: Limits = Limits {
    plan_items: 4,
    milestones: 8,
    title_bytes: 80,
    body_bytes: 512,
    comment_bytes: 120,
    interval: Duration::from_secs(60),
};

fn at(seconds: u64) -> Wall {
    Wall::from_nanos(seconds.saturating_mul(1_000_000_000))
}

fn goal() -> GoalView {
    GoalView {
        goal: 7,
        repository: 12,
        title: "Ship the goal".into(),
        goal_text: "A goal with a plan.".into(),
        plan: Box::new([PlanItem { text: "First step".into(), done: false }]),
        milestones: Box::new([]),
        finished: None,
    }
}

#[test]
fn opening_is_keyed_and_the_body_is_a_task_list() {
    let opened = project(&goal(), &Projected::default(), at(1), &LIMITS);
    assert_eq!(
        opened.decision,
        Decision::Effect(Effect::Open {
            goal: 7,
            repository: 12,
            key: Key::Open,
            title: "Ship the goal".into(),
            body: copy_of(b"A goal with a plan.\n- [ ] First step\n"),
        })
    );
    assert_eq!(project(&goal(), &opened.projected, at(1), &LIMITS).decision, Decision::None);
    assert_eq!(project(&goal(), &Projected::default(), at(1), &LIMITS), opened);
}

#[test]
fn a_changed_plan_writes_once_after_its_interval() {
    let old = project(&goal(), &Projected::default(), at(1), &LIMITS).projected;
    let mut changed = goal();
    changed.plan = Box::new([PlanItem { text: "First step".into(), done: true }]);
    assert_eq!(project(&changed, &old, at(30), &LIMITS).decision, Decision::Wait(at(61)));
    let rewritten = project(&changed, &old, at(61), &LIMITS);
    assert_eq!(
        rewritten.decision,
        Decision::Effect(Effect::Body {
            goal: 7,
            repository: 12,
            key: Key::Body(1),
            body: copy_of(b"A goal with a plan.\n- [x] First step\n"),
        })
    );
    assert_eq!(project(&changed, &rewritten.projected, at(61), &LIMITS).decision, Decision::None);
    changed.plan = Box::new([PlanItem { text: "Second step".into(), done: false }]);
    assert_eq!(project(&changed, &rewritten.projected, at(120), &LIMITS).decision, Decision::Wait(at(121)));
    let second = project(&changed, &rewritten.projected, at(121), &LIMITS);
    assert_eq!(
        second.decision,
        Decision::Effect(Effect::Body {
            goal: 7,
            repository: 12,
            key: Key::Body(2),
            body: copy_of(b"A goal with a plan.\n- [ ] Second step\n"),
        })
    );
}

#[test]
fn milestones_are_comments_once_even_while_a_body_waits() {
    let old = project(&goal(), &Projected::default(), at(1), &LIMITS).projected;
    let mut changed = goal();
    changed.plan = Box::new([PlanItem { text: "First step".into(), done: true }]);
    changed.milestones = Box::new([
        Milestone { key: MilestoneKey::PlanAccepted, text: "Plan accepted".into() },
        Milestone { key: MilestoneKey::Revision(2), text: "Revised because tests changed".into() },
    ]);
    let first = project(&changed, &old, at(30), &LIMITS);
    assert_eq!(
        first.decision,
        Decision::Effect(Effect::Comment {
            goal: 7,
            repository: 12,
            key: Key::Milestone(MilestoneKey::PlanAccepted),
            body: "Plan accepted".into(),
        })
    );
    let second = project(&changed, &first.projected, at(30), &LIMITS);
    assert_eq!(
        second.decision,
        Decision::Effect(Effect::Comment {
            goal: 7,
            repository: 12,
            key: Key::Milestone(MilestoneKey::Revision(2)),
            body: "Revised because tests changed".into(),
        })
    );
    assert_eq!(project(&changed, &second.projected, at(30), &LIMITS).decision, Decision::Wait(at(61)));
}

#[test]
fn final_comment_precedes_close_and_close_is_stable() {
    let old = project(&goal(), &Projected::default(), at(0), &LIMITS).projected;
    let mut completed = goal();
    completed.finished = Some("Goal finished".into());
    let commented = project(&completed, &old, at(1), &LIMITS);
    assert_eq!(
        commented.decision,
        Decision::Effect(Effect::Comment {
            goal: 7,
            repository: 12,
            key: Key::Milestone(MilestoneKey::Finished),
            body: "Goal finished".into(),
        })
    );
    let closed = project(&completed, &commented.projected, at(1), &LIMITS);
    assert_eq!(closed.decision, Decision::Effect(Effect::Close { goal: 7, repository: 12, key: Key::Close }));
    assert_eq!(project(&completed, &closed.projected, at(2), &LIMITS).decision, Decision::None);
}

#[test]
fn changed_final_body_is_written_before_close() {
    let old = project(&goal(), &Projected::default(), at(0), &LIMITS).projected;
    let mut completed = goal();
    completed.plan = Box::new([PlanItem { text: "First step".into(), done: true }]);
    completed.finished = Some("Goal finished".into());
    assert_eq!(project(&completed, &old, at(1), &LIMITS).decision, Decision::Wait(at(60)));
    let rewritten = project(&completed, &old, at(60), &LIMITS);
    assert_eq!(
        rewritten.decision,
        Decision::Effect(Effect::Body {
            goal: 7,
            repository: 12,
            key: Key::Body(1),
            body: copy_of(b"A goal with a plan.\n- [x] First step\n"),
        })
    );
    let commented = project(&completed, &rewritten.projected, at(60), &LIMITS);
    assert_eq!(
        commented.decision,
        Decision::Effect(Effect::Comment {
            goal: 7,
            repository: 12,
            key: Key::Milestone(MilestoneKey::Finished),
            body: "Goal finished".into(),
        })
    );
    assert_eq!(
        project(&completed, &commented.projected, at(60), &LIMITS).decision,
        Decision::Effect(Effect::Close { goal: 7, repository: 12, key: Key::Close })
    );
}

#[test]
fn oversized_and_contradictory_history_is_held_without_mutation() {
    let old = project(&goal(), &Projected::default(), at(0), &LIMITS).projected;
    let mut oversized = goal();
    oversized.plan = Box::new([PlanItem { text: "A task that is far too long".into(), done: false }]);
    let tight = Limits { body_bytes: 4, ..LIMITS };
    let held = project(&oversized, &old, at(60), &tight);
    assert_eq!(held.decision, Decision::Hold);
    assert_eq!(held.projected, old);
    let mut duplicate = goal();
    duplicate.milestones = Box::new([
        Milestone { key: MilestoneKey::PlanAccepted, text: "accepted".into() },
        Milestone { key: MilestoneKey::PlanAccepted, text: "again".into() },
    ]);
    assert_eq!(project(&duplicate, &old, at(60), &LIMITS).decision, Decision::Hold);
    let mut retreated = old.clone();
    retreated.milestones = Box::new([MilestoneKey::Revision(2)]);
    assert_eq!(project(&goal(), &retreated, at(60), &LIMITS).decision, Decision::Hold);
}

#[test]
fn projection_steps_repeat_from_identical_snapshots() {
    for changed in [false, true] {
        for finished in [false, true] {
            for milestone in [false, true] {
                let mut view = goal();
                if changed {
                    view.plan = Box::new([PlanItem { text: "First step".into(), done: true }]);
                }
                if finished {
                    view.finished = Some("done".into());
                }
                if milestone {
                    view.milestones =
                        Box::new([Milestone { key: MilestoneKey::PlanAccepted, text: "accepted".into() }]);
                }
                let previous = project(&goal(), &Projected::default(), at(0), &LIMITS).projected;
                assert_eq!(project(&view, &previous, at(30), &LIMITS), project(&view, &previous, at(30), &LIMITS));
            }
        }
    }
}
