//! Ask the rules, inspect what they answer and why.

use alloc::boxed::Box;

use skein_lib::{List, Queue, Rng};

use crate::{
    Act, Acts, Bound, Branch, Ci, Decision, Finding, Gate, Goal, Grant, Landing, Limits, Oversized, Permission, Plan,
    Repository, Request, Review, Rules, Run, Scope, Stance, Target, Write, check_request, check_run, check_write,
    max_out, worst_case,
};

const LIMITS: Limits =
    Limits { repositories: 4, protected: 4, branch_bytes: 16, grants: 4, reviews: 4, gates: 4, lands: 2 };

/// The engine's own forge user, and two people.
const ENGINE: u64 = 99;
const ALICE: u64 = 1;
const BOB: u64 = 2;

/// The head a landing is merged at, and an older one.
const HEAD: [u8; 32] = [7; 32];
const OLD: [u8; 32] = [6; 32];

const OURS: Repository = Repository::Deployment(0);
const OTHER: Repository = Repository::Deployment(1);

const ACCEPT_WRITE: Decision = Decision::Accept { permission: Permission::Write };
const UNREVIEWED: Finding = Finding::Unreviewed { permission: Permission::Write };

/// A deployment of two repositories, whose first protects `main`; reviews
/// count at write permission; plans of up to 8 steps and 1000 pass unasked;
/// notes of the deployment's scope wait for an admin.
fn rules() -> Rules {
    let mut protected = List::with_capacity(LIMITS.protected);
    protected.push(Branch { repository: 0, name: Box::from(*b"main") }).unwrap();
    Rules {
        repositories: 2,
        protected,
        engine: ENGINE,
        reviewer: Permission::Write,
        plan_steps: 8,
        plan_spend: 1000,
        plan_acceptance: Permission::Write,
        repository_notes: None,
        deployment_notes: Some(Permission::Admin),
        run_spend: 100,
        goal_spend: 1000,
        deployment_spend: 10_000,
        acts: Acts {
            open: Permission::Write,
            steer: Permission::Write,
            accept: Permission::Write,
            cancel: Permission::Write,
            release: Permission::Write,
            watch: Permission::Read,
        },
    }
}

/// What a check answered, and why.
fn drain(decision: Decision, mut out: Queue<Finding>) -> (Decision, Box<[Finding]>) {
    let mut found = List::with_capacity(out.len());
    while let Some(finding) = out.pop() {
        found.push(finding).unwrap();
    }
    (decision, found.into_boxed())
}

fn run_with(rules: &Rules, run: &Run, gates: &[Gate]) -> (Decision, Box<[Finding]>) {
    run_accepted(rules, run, None, gates)
}

fn run_accepted(rules: &Rules, run: &Run, accepted: Option<Permission>, gates: &[Gate]) -> (Decision, Box<[Finding]>) {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    let decision = check_run(rules, &LIMITS, run, accepted, gates, &mut out);
    drain(decision, out)
}

fn write_with(rules: &Rules, write: &Write, gates: &[Gate]) -> (Decision, Box<[Finding]>) {
    write_accepted(rules, write, None, gates)
}

fn write_accepted(
    rules: &Rules,
    write: &Write,
    accepted: Option<Permission>,
    gates: &[Gate],
) -> (Decision, Box<[Finding]>) {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    let decision = check_write(rules, &LIMITS, write, accepted, gates, &mut out);
    drain(decision, out)
}

fn waits(finding: Finding) -> (Decision, Box<[Finding]>) {
    (Decision::Wait, Box::new([finding]))
}

fn request(act: Act, permission: Permission) -> (Decision, Box<[Finding]>) {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    let decision = check_request(&rules(), &Request { repository: OURS, act, permission }, &mut out);
    drain(decision, out)
}

fn allowed() -> (Decision, Box<[Finding]>) {
    (Decision::Allow, Box::new([]))
}

fn refused(finding: Finding) -> (Decision, Box<[Finding]>) {
    (Decision::Refuse, Box::new([finding]))
}

/// A run under a goal that has spent 500, in a deployment that has spent
/// 5000, with a budget of `budget`, granted `grants`.
fn run(budget: u64, grants: Box<[Grant]>) -> Run {
    Run { budget, goal: Goal::Spent(500), deployment_spent: 5000, grants }
}

fn push(repository: Repository, branch: &[u8]) -> Grant {
    Grant::Push { repository, branch: Box::from(branch) }
}

fn review(person: u64, permission: Permission, head: [u8; 32], stance: Stance) -> Review {
    Review { person, permission, head, stance }
}

/// A landing on `base` of the first repository, with green CI on its head
/// and `reviews`.
fn landing(base: &[u8], reviews: Box<[Review]>) -> Landing {
    Landing { repository: OURS, base: Box::from(base), head: HEAD, ci: Ci::Passed, ci_head: HEAD, reviews }
}

fn no_grants() -> Box<[Grant]> {
    Box::new([])
}

/// A plan of `steps` and `spend` in the first repository, under a goal that
/// has spent nothing, landing on `lands` of it.
fn plan(steps: u32, spend: u64, lands: &[&[u8]]) -> Plan {
    let mut targets = List::with_capacity(4);
    for branch in lands {
        targets.push(Target { repository: OURS, branch: Box::from(*branch) }).unwrap();
    }
    Plan { repository: OURS, steps, spend, lands: targets.into_boxed(), goal: Goal::Spent(0) }
}

fn note(scope: Scope) -> Write {
    Write::Note { repository: OURS, scope }
}

fn approved() -> Box<[Review]> {
    Box::new([review(ALICE, Permission::Write, HEAD, Stance::Approve)])
}

// Runs.

#[test]
fn a_run_within_every_bound_that_pushes_to_its_own_branch_is_allowed() {
    let grants = Box::new([push(OURS, b"fix-1"), Grant::Read { repository: OTHER }]);
    assert_eq!(run_with(&rules(), &run(100, grants), &[]), allowed());
}

#[test]
fn a_run_reads_anywhere_but_pushes_only_to_the_deployment_s_repositories() {
    let read = Box::new([Grant::Read { repository: Repository::Elsewhere }]);
    assert_eq!(run_with(&rules(), &run(10, read), &[]), allowed());
    let beyond = Box::new([push(Repository::Deployment(2), b"fix")]);
    assert_eq!(run_with(&rules(), &run(10, beyond), &[]), refused(Finding::Elsewhere), "past the configured list");
    let elsewhere = Box::new([push(Repository::Elsewhere, b"fix")]);
    assert_eq!(run_with(&rules(), &run(10, elsewhere), &[]), refused(Finding::Elsewhere));
}

#[test]
fn a_run_never_pushes_to_a_protected_branch() {
    let main = Box::new([push(OURS, b"main")]);
    assert_eq!(run_with(&rules(), &run(10, main), &[]), refused(Finding::Protected));
    let other = Box::new([push(OTHER, b"main")]);
    assert_eq!(run_with(&rules(), &run(10, other), &[]), allowed(), "main is protected in the first repository only");
}

#[test]
fn a_run_past_its_own_bound_its_goal_s_or_the_deployment_s_is_refused() {
    assert_eq!(run_with(&rules(), &run(101, no_grants()), &[]), refused(Finding::Overspent(Bound::Run)));
    let goal = Run { goal: Goal::Spent(950), ..run(51, no_grants()) };
    assert_eq!(run_with(&rules(), &goal, &[]), refused(Finding::Overspent(Bound::Goal)));
    let at_goal = Run { goal: Goal::Spent(950), ..run(50, no_grants()) };
    assert_eq!(run_with(&rules(), &at_goal, &[]), allowed(), "up to the bound is within it");
    let free = Run { goal: Goal::Outside, ..run(50, no_grants()) };
    assert_eq!(run_with(&rules(), &free, &[]), allowed(), "a run under no goal has no goal's bound");
    let deployment = Run { deployment_spent: 9_950, ..run(51, no_grants()) };
    assert_eq!(run_with(&rules(), &deployment, &[]), refused(Finding::Overspent(Bound::Deployment)));
    let wrapped = Run { deployment_spent: u64::MAX, ..run(1, no_grants()) };
    assert_eq!(run_with(&rules(), &wrapped, &[]), refused(Finding::Overspent(Bound::Deployment)), "past u64 is past");
    let all = Run { goal: Goal::Spent(1000), deployment_spent: 10_000, ..run(101, no_grants()) };
    let (decision, found) = run_with(&rules(), &all, &[]);
    assert_eq!(decision, Decision::Refuse);
    let bounds =
        [Finding::Overspent(Bound::Run), Finding::Overspent(Bound::Goal), Finding::Overspent(Bound::Deployment)];
    assert_eq!(*found, bounds, "every bound passed is said");
}

#[test]
fn a_read_only_step_s_run_may_read_but_not_push() {
    let read = Box::new([Grant::Read { repository: OURS }]);
    assert_eq!(run_with(&rules(), &run(10, read), &[Gate::ReadOnly]), allowed());
    let pushes = Box::new([push(OURS, b"a"), push(OURS, b"b")]);
    let gates = [Gate::ReadOnly, Gate::ReadOnly];
    assert_eq!(run_with(&rules(), &run(10, pushes), &gates), refused(Finding::ReadOnly), "said once");
}

#[test]
fn a_plan_s_spending_gates_tighten_the_bounds() {
    assert_eq!(run_with(&rules(), &run(50, no_grants()), &[Gate::Spend { most: 50 }]), allowed());
    let step = run_with(&rules(), &run(51, no_grants()), &[Gate::Spend { most: 50 }]);
    assert_eq!(step, refused(Finding::Overspent(Bound::Step)));
    let plan = run_with(&rules(), &run(51, no_grants()), &[Gate::GoalSpend { most: 550 }]);
    assert_eq!(plan, refused(Finding::Overspent(Bound::Plan)), "500 spent already");
    let free = Run { goal: Goal::Outside, ..run(51, no_grants()) };
    assert_eq!(run_with(&rules(), &free, &[Gate::GoalSpend { most: 50 }]), refused(Finding::Overspent(Bound::Plan)));
    let loose = run_with(&rules(), &run(101, no_grants()), &[Gate::Spend { most: 1000 }]);
    assert_eq!(loose, refused(Finding::Overspent(Bound::Run)), "a gate never loosens the deployment's bound");
    let landing_gates = [Gate::Ci, Gate::Review { permission: Permission::Admin }];
    assert_eq!(
        run_with(&rules(), &run(10, no_grants()), &landing_gates),
        allowed(),
        "a landing's gates are not a run's"
    );
}

#[test]
fn a_run_asked_about_more_than_the_limits_is_refused_at_the_entrance() {
    let grants: Box<[Grant]> = Box::new([const { Grant::Read { repository: OURS } }; 5]);
    let too_many = run_with(&rules(), &run(1000, grants), &[]);
    assert_eq!(too_many, refused(Finding::Oversized(Oversized::Grants)), "nothing else is checked");
    let long = Box::new([push(OURS, &[b'b'; 17])]);
    assert_eq!(run_with(&rules(), &run(10, long), &[]), refused(Finding::Oversized(Oversized::Branch)));
    let gates = [Gate::Ci; 5];
    let none = Box::new([]);
    assert_eq!(run_with(&rules(), &run(10, none), &gates), refused(Finding::Oversized(Oversized::Gates)));
}

// Writes.

#[test]
fn writes_go_only_to_the_deployment_s_repositories() {
    for repository in [OURS, OTHER] {
        assert_eq!(write_with(&rules(), &Write::Item { repository }, &[]), allowed());
    }
    for repository in [Repository::Deployment(2), Repository::Elsewhere] {
        let item = Write::Item { repository };
        assert_eq!(write_with(&rules(), &item, &[]), refused(Finding::Elsewhere));
        let open = Write::Open { repository, base: Box::from(*b"main") };
        assert_eq!(write_with(&rules(), &open, &[]), refused(Finding::Elsewhere));
        let note = Write::Note { repository, scope: Scope::Goal };
        assert_eq!(write_with(&rules(), &note, &[]), refused(Finding::Elsewhere));
        let elsewhere = Write::Plan(Plan { repository, ..plan(1, 1, &[]) });
        assert_eq!(write_with(&rules(), &elsewhere, &[]), refused(Finding::Elsewhere));
        let mut landing = plan(1, 1, &[b"fix"]);
        landing.lands = Box::new([Target { repository, branch: Box::from(*b"fix") }]);
        assert_eq!(write_with(&rules(), &Write::Plan(landing), &[]), refused(Finding::Elsewhere), "nor land there");
        let delete = Write::Delete { repository, branch: Box::from(*b"main") };
        assert_eq!(write_with(&rules(), &delete, &[]), refused(Finding::Elsewhere), "nothing is protected there");
    }
    let land = Write::Land(Landing { repository: Repository::Elsewhere, ..landing(b"main", approved()) });
    assert_eq!(write_with(&rules(), &land, &[]), refused(Finding::Elsewhere));
}

#[test]
fn a_change_lands_on_a_protected_branch_with_green_ci_and_an_approval_on_its_exact_head() {
    let land = Write::Land(landing(b"main", approved()));
    assert_eq!(write_with(&rules(), &land, &[]), allowed());
    let admin = Box::new([review(BOB, Permission::Admin, HEAD, Stance::Approve)]);
    assert_eq!(write_with(&rules(), &Write::Land(landing(b"main", admin)), &[]), allowed(), "or more permission");
}

#[test]
fn nothing_lands_on_a_protected_branch_without_green_ci_on_its_exact_head() {
    for ci in [Ci::None, Ci::Pending] {
        let land = Write::Land(Landing { ci, ..landing(b"main", approved()) });
        assert_eq!(write_with(&rules(), &land, &[]), waits(Finding::Unchecked { ci }), "CI is to come");
    }
    let failed = Write::Land(Landing { ci: Ci::Failed, ..landing(b"main", approved()) });
    assert_eq!(write_with(&rules(), &failed, &[]), refused(Finding::Unchecked { ci: Ci::Failed }));
    let stale = Write::Land(Landing { ci_head: OLD, ..landing(b"main", approved()) });
    let unchecked = waits(Finding::Unchecked { ci: Ci::None });
    assert_eq!(write_with(&rules(), &stale, &[]), unchecked, "green on another head is no CI on this one");
    // No person's acceptance stands in for CI: accepting waits too.
    let pending = Write::Land(Landing { ci: Ci::Pending, ..landing(b"main", approved()) });
    let accepted = write_accepted(&rules(), &pending, Some(Permission::Admin), &[]);
    assert_eq!(accepted, waits(Finding::Unchecked { ci: Ci::Pending }));
}

#[test]
fn a_landing_on_a_protected_branch_without_an_approval_waits_for_a_review_never_an_acceptance() {
    let waits = waits(UNREVIEWED);
    let cases: [Box<[Review]>; 4] = [
        Box::new([]),
        Box::new([review(ALICE, Permission::Read, HEAD, Stance::Approve)]),
        Box::new([review(ENGINE, Permission::Admin, HEAD, Stance::Approve)]),
        Box::new([review(ALICE, Permission::Write, OLD, Stance::Approve)]),
    ];
    for reviews in cases {
        let land = Write::Land(landing(b"main", reviews));
        assert_eq!(write_with(&rules(), &land, &[]), waits);
        assert_eq!(write_accepted(&rules(), &land, Some(Permission::Admin), &[]), waits, "an acceptance is no review");
    }
}

#[test]
fn a_request_for_changes_by_a_reviewer_holds_a_landing_back_until_they_approve() {
    let held = refused(Finding::ChangesRequested { permission: Permission::Write });
    let changes = Box::new([
        review(ALICE, Permission::Write, HEAD, Stance::Approve),
        review(BOB, Permission::Write, HEAD, Stance::RequestChanges),
    ]);
    assert_eq!(write_with(&rules(), &Write::Land(landing(b"main", changes)), &[]), held);
    // Standing on an older head, as on the forge, until the same reviewer
    // approves.
    let standing = Box::new([
        review(ALICE, Permission::Write, HEAD, Stance::Approve),
        review(3, Permission::Write, OLD, Stance::RequestChanges),
    ]);
    assert_eq!(write_with(&rules(), &Write::Land(landing(b"main", standing)), &[]), held);
    let outweighed = Box::new([
        review(ALICE, Permission::Write, HEAD, Stance::Approve),
        review(BOB, Permission::Read, HEAD, Stance::RequestChanges),
        review(ENGINE, Permission::Admin, HEAD, Stance::RequestChanges),
    ]);
    let land = Write::Land(landing(b"main", outweighed));
    assert_eq!(write_with(&rules(), &land, &[]), allowed(), "a reader's or the engine's count for nothing");
}

#[test]
fn a_landing_elsewhere_needs_what_its_step_s_gates_add() {
    let bare = Write::Land(Landing { ci: Ci::Failed, ..landing(b"feat", Box::new([])) });
    assert_eq!(write_with(&rules(), &bare, &[]), allowed(), "an unprotected branch, ungated");
    assert_eq!(write_with(&rules(), &bare, &[Gate::Ci]), refused(Finding::Unchecked { ci: Ci::Failed }));
    // A review gate counts at write at least: a reader's review never does.
    let gate = Gate::Review { permission: Permission::Read };
    assert_eq!(write_with(&rules(), &bare, &[gate]), waits(UNREVIEWED));
    assert_eq!(write_with(&rules(), &bare, &[Gate::ReadOnly]), refused(Finding::ReadOnly));
    let reader = Box::new([review(BOB, Permission::Read, HEAD, Stance::Approve)]);
    assert_eq!(write_with(&rules(), &Write::Land(landing(b"feat", reader)), &[gate]), waits(UNREVIEWED));
    assert_eq!(write_with(&rules(), &Write::Land(landing(b"feat", approved())), &[gate]), allowed());
}

#[test]
fn a_landing_gated_on_approvals_waits_for_as_many_people_on_its_exact_head() {
    let gate = [Gate::Approvals(2)];
    let one = Write::Land(landing(b"feat", approved()));
    assert_eq!(write_with(&rules(), &one, &gate), waits(Finding::Unapproved { have: 1, want: 2 }));
    let not_counted = Box::new([
        review(ALICE, Permission::Write, HEAD, Stance::Approve),
        review(ALICE, Permission::Write, HEAD, Stance::Approve),
        review(BOB, Permission::Write, OLD, Stance::Approve),
        review(3, Permission::Read, HEAD, Stance::Approve),
    ]);
    let land = Write::Land(landing(b"feat", not_counted));
    let found = write_with(&rules(), &land, &gate);
    assert_eq!(found, waits(Finding::Unapproved { have: 1, want: 2 }), "twice, an old head, a reader");
    let two = Box::new([
        review(ALICE, Permission::Write, HEAD, Stance::Approve),
        review(BOB, Permission::Admin, HEAD, Stance::Approve),
        review(ENGINE, Permission::Admin, HEAD, Stance::Approve),
    ]);
    assert_eq!(write_with(&rules(), &Write::Land(landing(b"feat", two)), &gate), allowed());
}

#[test]
fn a_step_gated_on_acceptance_waits_for_a_person_who_may_accept() {
    let gate = [Gate::Accepted];
    let unaccepted = (ACCEPT_WRITE, Box::from([Finding::Unaccepted { permission: Permission::Write }]));
    assert_eq!(run_with(&rules(), &run(10, no_grants()), &gate), unaccepted);
    assert_eq!(run_accepted(&rules(), &run(10, no_grants()), Some(Permission::Read), &gate), unaccepted);
    assert_eq!(run_accepted(&rules(), &run(10, no_grants()), Some(Permission::Write), &gate), allowed());
    let land = Write::Land(landing(b"main", approved()));
    assert_eq!(write_with(&rules(), &land, &gate), unaccepted);
    assert_eq!(write_accepted(&rules(), &land, Some(Permission::Admin), &gate), allowed());
    // An acceptance clears only what accepting clears.
    let unreviewed = Write::Land(landing(b"main", Box::new([])));
    let (decision, found) = write_with(&rules(), &unreviewed, &gate);
    assert_eq!(decision, ACCEPT_WRITE, "a person's acceptance is asked for first");
    assert_eq!(*found, [UNREVIEWED, Finding::Unaccepted { permission: Permission::Write }]);
    assert_eq!(write_accepted(&rules(), &unreviewed, Some(Permission::Write), &gate), waits(UNREVIEWED));
}

#[test]
fn a_gate_never_loosens_the_review_a_protected_branch_needs() {
    let reader = Box::new([review(BOB, Permission::Read, HEAD, Stance::Approve)]);
    let land = Write::Land(landing(b"main", reader));
    let gate = Gate::Review { permission: Permission::Read };
    let (decision, found) = write_with(&rules(), &land, &[gate]);
    assert_eq!(decision, Decision::Wait);
    assert_eq!(*found, [UNREVIEWED, UNREVIEWED]);
    let admin = Gate::Review { permission: Permission::Admin };
    let found = write_with(&rules(), &Write::Land(landing(b"main", approved())), &[admin]);
    assert_eq!(found, waits(Finding::Unreviewed { permission: Permission::Admin }), "a stricter gate adds to it");
}

#[test]
fn a_protected_branch_is_never_deleted_and_a_read_only_step_deletes_nothing() {
    let main = Write::Delete { repository: OURS, branch: Box::from(*b"main") };
    assert_eq!(write_with(&rules(), &main, &[]), refused(Finding::Protected));
    let fix = Write::Delete { repository: OURS, branch: Box::from(*b"fix") };
    assert_eq!(write_with(&rules(), &fix, &[]), allowed());
    assert_eq!(write_with(&rules(), &fix, &[Gate::ReadOnly]), refused(Finding::ReadOnly));
}

#[test]
fn a_read_only_step_opens_no_pull_request_but_writes_to_its_item() {
    let open = Write::Open { repository: OURS, base: Box::from(*b"main") };
    assert_eq!(write_with(&rules(), &open, &[]), allowed(), "opening one on main is not landing there");
    assert_eq!(write_with(&rules(), &open, &[Gate::ReadOnly]), refused(Finding::ReadOnly));
    let item = Write::Item { repository: OURS };
    assert_eq!(write_with(&rules(), &item, &[Gate::ReadOnly]), allowed());
}

#[test]
fn a_plan_past_a_size_or_an_estimate_waits_and_one_past_a_goal_s_bound_is_refused() {
    assert_eq!(write_with(&rules(), &Write::Plan(plan(8, 1000, &[b"fix"])), &[]), allowed());
    let large = (ACCEPT_WRITE, Box::from([Finding::PlanSize { permission: Permission::Write }]));
    assert_eq!(write_with(&rules(), &Write::Plan(plan(9, 10, &[])), &[]), large);
    let rules = Rules { goal_spend: 5000, ..rules() };
    let costly = (ACCEPT_WRITE, Box::from([Finding::PlanSpend { permission: Permission::Write }]));
    assert_eq!(write_with(&rules, &Write::Plan(plan(1, 1001, &[])), &[]), costly);
    let (decision, found) = write_with(&rules, &Write::Plan(plan(9, 5001, &[b"main"])), &[]);
    assert_eq!(decision, Decision::Refuse);
    let all = [
        Finding::PlanSize { permission: Permission::Write },
        Finding::PlanSpend { permission: Permission::Write },
        Finding::PlanLands { permission: Permission::Write },
        Finding::Overspent(Bound::Goal),
    ];
    assert_eq!(*found, all);
    let budget = [Gate::GoalSpend { most: 10 }];
    assert_eq!(write_with(&rules, &Write::Plan(plan(1, 11, &[])), &budget), refused(Finding::Overspent(Bound::Plan)));
}

#[test]
fn a_plan_that_lands_on_a_protected_branch_waits_for_a_person_s_acceptance() {
    let lands = Write::Plan(plan(2, 10, &[b"fix", b"main"]));
    let waiting = (ACCEPT_WRITE, Box::from([Finding::PlanLands { permission: Permission::Write }]));
    assert_eq!(write_with(&rules(), &lands, &[]), waiting);
    assert_eq!(write_accepted(&rules(), &lands, Some(Permission::Read), &[]), waiting, "by someone who may");
    assert_eq!(write_accepted(&rules(), &lands, Some(Permission::Write), &[]), allowed());
    let rules = Rules { goal_spend: 5000, ..rules() };
    let large = Write::Plan(plan(9, 1001, &[b"main"]));
    assert_eq!(write_accepted(&rules, &large, Some(Permission::Write), &[]), allowed(), "accepted, all of it");
}

#[test]
fn a_plan_counts_what_its_goal_has_spent_already() {
    let spent = Write::Plan(Plan { goal: Goal::Spent(950), ..plan(1, 51, &[]) });
    assert_eq!(write_with(&rules(), &spent, &[]), refused(Finding::Overspent(Bound::Goal)));
    let accepted = write_accepted(&rules(), &spent, Some(Permission::Admin), &[]);
    assert_eq!(accepted, refused(Finding::Overspent(Bound::Goal)), "no acceptance passes a bound");
    let within = Write::Plan(Plan { goal: Goal::Spent(950), ..plan(1, 50, &[]) });
    assert_eq!(write_with(&rules(), &within, &[]), allowed());
    let budget = [Gate::GoalSpend { most: 999 }];
    assert_eq!(write_with(&rules(), &within, &budget), refused(Finding::Overspent(Bound::Plan)));
}

#[test]
fn a_note_wider_than_a_goal_waits_where_the_deployment_wants_it_to() {
    assert_eq!(write_with(&rules(), &note(Scope::Goal), &[]), allowed());
    assert_eq!(write_with(&rules(), &note(Scope::Repository), &[]), allowed(), "not wanted for a repository's");
    let waits = (
        Decision::Accept { permission: Permission::Admin },
        Box::from([Finding::WideNote { permission: Permission::Admin }]),
    );
    assert_eq!(write_with(&rules(), &note(Scope::Deployment), &[]), waits);
    let rules = Rules { repository_notes: Some(Permission::Write), ..rules() };
    let repository = (ACCEPT_WRITE, Box::from([Finding::WideNote { permission: Permission::Write }]));
    assert_eq!(write_with(&rules, &note(Scope::Repository), &[]), repository);
    assert_eq!(write_with(&rules, &note(Scope::Goal), &[]), allowed(), "never for a goal's");
    let accepted = write_accepted(&rules, &note(Scope::Deployment), Some(Permission::Write), &[]);
    let admin = (
        Decision::Accept { permission: Permission::Admin },
        Box::from([Finding::WideNote { permission: Permission::Admin }]),
    );
    assert_eq!(accepted, admin, "by someone who may");
    assert_eq!(write_accepted(&rules, &note(Scope::Deployment), Some(Permission::Admin), &[]), allowed());
}

#[test]
fn a_write_asked_about_more_than_the_limits_is_refused_at_the_entrance() {
    let reviews: Box<[Review]> = Box::new([review(ALICE, Permission::Write, HEAD, Stance::Approve); 5]);
    let land = Write::Land(landing(b"main", reviews));
    assert_eq!(write_with(&rules(), &land, &[]), refused(Finding::Oversized(Oversized::Reviews)));
    let long = Write::Open { repository: OURS, base: Box::from([b'b'; 17]) };
    assert_eq!(write_with(&rules(), &long, &[]), refused(Finding::Oversized(Oversized::Branch)));
    let long = Write::Delete { repository: Repository::Elsewhere, branch: Box::from([b'b'; 17]) };
    assert_eq!(write_with(&rules(), &long, &[]), refused(Finding::Oversized(Oversized::Branch)), "first");
    let gates = [Gate::ReadOnly; 5];
    assert_eq!(
        write_with(&rules(), &Write::Item { repository: OURS }, &gates),
        refused(Finding::Oversized(Oversized::Gates))
    );
    let lands = Write::Plan(plan(1, 1, &[b"a", b"b", b"c"]));
    assert_eq!(write_with(&rules(), &lands, &[]), refused(Finding::Oversized(Oversized::Lands)));
    let long = Write::Plan(plan(1, 1, &[&[b'b'; 17]]));
    assert_eq!(write_with(&rules(), &long, &[]), refused(Finding::Oversized(Oversized::Branch)));
}

// People.

#[test]
fn a_person_s_act_follows_their_permission_on_the_repository() {
    let acts = [Act::Open, Act::Steer, Act::Cancel, Act::Release];
    for act in acts {
        assert_eq!(request(act, Permission::Write), allowed());
        assert_eq!(request(act, Permission::Admin), allowed());
        let short = refused(Finding::Unpermitted { needs: Permission::Write, has: Permission::Read });
        assert_eq!(request(act, Permission::Read), short);
    }
    assert_eq!(request(Act::Watch, Permission::Read), allowed());
    let none = refused(Finding::Unpermitted { needs: Permission::Read, has: Permission::None });
    assert_eq!(request(Act::Watch, Permission::None), none);
}

#[test]
fn a_proposal_is_accepted_or_rejected_by_a_person_with_as_much_permission_as_it_waits_for() {
    for act in [Act::Accept { permission: Permission::Read }, Act::Reject { permission: Permission::Read }] {
        assert_eq!(request(act, Permission::Write), allowed(), "the rules' own need is the floor");
    }
    for act in [Act::Accept { permission: Permission::Admin }, Act::Reject { permission: Permission::Admin }] {
        let short = refused(Finding::Unpermitted { needs: Permission::Admin, has: Permission::Write });
        assert_eq!(request(act, Permission::Write), short);
        assert_eq!(request(act, Permission::Admin), allowed());
    }
}

#[test]
fn a_person_s_request_about_a_repository_not_the_deployment_s_is_refused() {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    let asked = Request { repository: Repository::Elsewhere, act: Act::Watch, permission: Permission::Admin };
    let decision = check_request(&rules(), &asked, &mut out);
    assert_eq!(drain(decision, out), refused(Finding::Elsewhere));
}

// The answers, the configuration and the worst case.

#[test]
fn a_refusal_is_stricter_than_any_acceptance_and_acceptances_go_by_permission() {
    assert!(Decision::Allow < Decision::Wait);
    assert!(Decision::Wait < Decision::Accept { permission: Permission::None });
    assert!(Decision::Accept { permission: Permission::Read } < ACCEPT_WRITE);
    assert!(ACCEPT_WRITE < Decision::Accept { permission: Permission::Admin });
    assert!(Decision::Accept { permission: Permission::Admin } < Decision::Refuse);
}

#[test]
fn rules_that_do_not_fit_the_limits_are_refused() {
    assert!(rules().fits(&LIMITS));
    assert!(!Rules { repositories: 5, ..rules() }.fits(&LIMITS));
    let mut protected = List::with_capacity(1);
    protected.push(Branch { repository: 2, name: Box::from(*b"main") }).unwrap();
    assert!(!Rules { protected, ..rules() }.fits(&LIMITS), "a protected branch of a repository not configured");
    let mut protected = List::with_capacity(1);
    protected.push(Branch { repository: 0, name: Box::from([b'm'; 17]) }).unwrap();
    assert!(!Rules { protected, ..rules() }.fits(&LIMITS));
    let mut protected = List::with_capacity(1);
    protected.push(Branch { repository: 0, name: Box::new([]) }).unwrap();
    assert!(!Rules { protected, ..rules() }.fits(&LIMITS));
    assert!(!Rules { protected: List::with_capacity(5), ..rules() }.fits(&LIMITS));
    assert!(!Rules { reviewer: Permission::Read, ..rules() }.fits(&LIMITS), "a reader's review never counts");
    assert!(Rules { reviewer: Permission::Admin, ..rules() }.fits(&LIMITS));
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the test limits fit");
    assert!(bound >= u64::from(LIMITS.protected * LIMITS.branch_bytes), "it counts the protected branches' names");
    let more = worst_case(&Limits { protected: 8, ..LIMITS }).expect("fits");
    assert!(more > bound, "a protected branch more is more");
    assert_eq!(worst_case(&Limits { protected: u32::MAX, branch_bytes: u32::MAX, ..LIMITS }), None);
    assert_eq!(max_out(&LIMITS), 13, "five of a check's own, and one for each grant and gate");
}

// Gates, at random.

/// What a sweep saw: answers refused without gates, and with them made to
/// refuse, made to wait for a person (or for one with more permission), made
/// to wait for facts, and left as they were; and allowed, judged against the
/// rules stated directly.
#[derive(Debug)]
struct Tightened {
    to_accept: u32,
    to_wait: u32,
    to_refuse: u32,
    unchanged: u32,
    refused: u32,
    allowed: u32,
}

fn permission(rng: &mut Rng) -> Permission {
    [Permission::None, Permission::Read, Permission::Write, Permission::Admin][index(rng, 4)]
}

/// A permission, or none, at even odds.
fn maybe(rng: &mut Rng) -> Option<Permission> {
    if rng.chance(500) { Some(permission(rng)) } else { None }
}

fn index(rng: &mut Rng, below: u64) -> usize {
    usize::try_from(rng.below(below)).unwrap()
}

fn repository(rng: &mut Rng) -> Repository {
    if rng.chance(100) { Repository::Elsewhere } else { Repository::Deployment(u32::try_from(rng.below(3)).unwrap()) }
}

fn branch(rng: &mut Rng) -> Box<[u8]> {
    let names: [&[u8]; 3] = [b"main", b"release", b"fix"];
    Box::from(names[index(rng, 3)])
}

fn goal(rng: &mut Rng) -> Goal {
    if rng.chance(700) { Goal::Spent(rng.below(1500)) } else { Goal::Outside }
}

fn random_rules(rng: &mut Rng) -> Rules {
    let mut protected = List::with_capacity(LIMITS.protected);
    let repositories = u32::try_from(rng.between(1, 3)).unwrap();
    for _ in 0..rng.below(u64::from(LIMITS.protected) + 1) {
        let repository = u32::try_from(rng.below(u64::from(repositories))).unwrap();
        protected.push(Branch { repository, name: branch(rng) }).unwrap();
    }
    Rules {
        repositories,
        protected,
        engine: ENGINE,
        reviewer: if rng.chance(700) { Permission::Write } else { Permission::Admin },
        plan_steps: u32::try_from(rng.below(10)).unwrap(),
        plan_spend: rng.below(1000),
        plan_acceptance: permission(rng),
        repository_notes: maybe(rng),
        deployment_notes: maybe(rng),
        run_spend: rng.between(300, 2000),
        goal_spend: rng.between(1000, 4000),
        deployment_spend: rng.between(2000, 8000),
        acts: rules().acts,
    }
}

fn random_review(rng: &mut Rng) -> Review {
    let person = [ALICE, BOB, 3, ENGINE][index(rng, 4)];
    let head = if rng.chance(800) { HEAD } else { OLD };
    let stance = if rng.chance(800) { Stance::Approve } else { Stance::RequestChanges };
    review(person, permission(rng), head, stance)
}

fn random_write(rng: &mut Rng) -> Write {
    match rng.below(6) {
        0 => Write::Item { repository: repository(rng) },
        1 => Write::Open { repository: repository(rng), base: branch(rng) },
        2 => {
            let mut reviews = List::with_capacity(LIMITS.reviews);
            for _ in 0..rng.below(u64::from(LIMITS.reviews) + 1) {
                reviews.push(random_review(rng)).unwrap();
            }
            let ci = [Ci::None, Ci::Pending, Ci::Passed, Ci::Failed][index(rng, 4)];
            let ci_head = if rng.chance(800) { HEAD } else { OLD };
            let landing = landing(&branch(rng), reviews.into_boxed());
            Write::Land(Landing { repository: repository(rng), ci, ci_head, ..landing })
        }
        3 => Write::Delete { repository: repository(rng), branch: branch(rng) },
        4 => {
            let mut lands = List::with_capacity(LIMITS.lands);
            for _ in 0..rng.below(u64::from(LIMITS.lands) + 1) {
                lands.push(Target { repository: repository(rng), branch: branch(rng) }).unwrap();
            }
            let steps = u32::try_from(rng.below(12)).unwrap();
            let (repository, spend, goal) = (repository(rng), rng.below(2500), goal(rng));
            Write::Plan(Plan { repository, steps, spend, lands: lands.into_boxed(), goal })
        }
        _ => {
            let scope = [Scope::Goal, Scope::Repository, Scope::Deployment][index(rng, 3)];
            Write::Note { repository: repository(rng), scope }
        }
    }
}

fn random_run(rng: &mut Rng) -> Run {
    let mut grants = List::with_capacity(LIMITS.grants);
    for _ in 0..rng.below(u64::from(LIMITS.grants) + 1) {
        let grant = if rng.chance(500) {
            Grant::Read { repository: repository(rng) }
        } else {
            Grant::Push { repository: repository(rng), branch: branch(rng) }
        };
        grants.push(grant).unwrap();
    }
    Run { budget: rng.below(600), goal: goal(rng), deployment_spent: rng.below(3000), grants: grants.into_boxed() }
}

fn random_gates(rng: &mut Rng) -> Box<[Gate]> {
    let mut gates = List::with_capacity(LIMITS.gates);
    for _ in 0..rng.below(u64::from(LIMITS.gates) + 1) {
        let gate = match rng.below(7) {
            0 => Gate::ReadOnly,
            1 => Gate::Ci,
            2 => Gate::Review { permission: permission(rng) },
            3 => Gate::Approvals(u32::try_from(rng.below(3)).unwrap()),
            4 => Gate::Accepted,
            5 => Gate::Spend { most: rng.below(1200) },
            _ => Gate::GoalSpend { most: rng.below(3000) },
        };
        gates.push(gate).unwrap();
    }
    gates.into_boxed()
}

/// Checks that gates only added to what was found without them, and counts
/// how the answer changed.
fn compare(bare: &(Decision, Box<[Finding]>), gated: &(Decision, Box<[Finding]>), seen: &mut Tightened) {
    assert!(gated.0 >= bare.0, "gates never loosen an answer: {bare:?} became {gated:?}");
    for finding in &bare.1 {
        assert!(gated.1.contains(finding), "gates take nothing away: {finding:?} is gone from {gated:?}");
    }
    let count = if bare.0 == Decision::Refuse {
        &mut seen.refused
    } else if gated.0 == Decision::Refuse {
        &mut seen.to_refuse
    } else if gated.0 == Decision::Wait && bare.0 == Decision::Allow {
        &mut seen.to_wait
    } else if gated.0 > bare.0 {
        &mut seen.to_accept
    } else {
        &mut seen.unchanged
    };
    *count = count.checked_add(1).unwrap();
}

// The rules, stated directly: what an allowed run or write must satisfy,
// checked against every one the sweep allows.

fn ours(rules: &Rules, repository: Repository) -> bool {
    match repository {
        Repository::Deployment(index) => index < rules.repositories,
        Repository::Elsewhere => false,
    }
}

fn is_protected(rules: &Rules, repository: Repository, branch: &[u8]) -> bool {
    let Repository::Deployment(index) = repository else { return false };
    let mut found = false;
    for protected in &rules.protected {
        found |= protected.repository == index && *protected.name == *branch;
    }
    found
}

fn gated(gates: &[Gate], wanted: Gate) -> bool {
    gates.contains(&wanted)
}

fn within(spent: u64, more: u64, most: u64) -> bool {
    match spent.checked_add(more) {
        Some(total) => total <= most,
        None => false,
    }
}

/// An approval on the exact head by someone other than the engine, with
/// `permission` and write at least; and no request for changes standing by
/// such a person, on any head.
fn approved_at(rules: &Rules, landing: &Landing, permission: Permission) -> bool {
    let (mut approval, mut standing) = (false, false);
    for review in &landing.reviews {
        if review.person == rules.engine || review.permission < permission.max(Permission::Write) {
            continue;
        }
        approval |= review.stance == Stance::Approve && review.head == landing.head;
        standing |= review.stance == Stance::RequestChanges;
    }
    approval && !standing
}

/// How many distinct people, other than the engine, with write at least,
/// approve the exact head.
fn people_approving(rules: &Rules, landing: &Landing) -> u32 {
    let mut people = [false; 4];
    for review in &landing.reviews {
        let counts = review.person != rules.engine
            && review.permission >= Permission::Write
            && review.head == landing.head
            && review.stance == Stance::Approve;
        if counts {
            people[usize::try_from(review.person).unwrap()] = true;
        }
    }
    let mut count = 0_u32;
    for person in people {
        count = count.checked_add(u32::from(person)).unwrap();
    }
    count
}

fn assert_run_allowed(rules: &Rules, run: &Run, accepted: Option<Permission>, gates: &[Gate]) {
    for grant in &run.grants {
        if let Grant::Push { repository, branch } = grant {
            assert!(ours(rules, *repository), "a push only to the deployment's repositories");
            assert!(!is_protected(rules, *repository, branch), "never to a protected branch");
            assert!(!gated(gates, Gate::ReadOnly), "no push for a read-only step");
        }
    }
    assert!(run.budget <= rules.run_spend, "within the run's bound");
    if let Goal::Spent(spent) = run.goal {
        assert!(within(spent, run.budget, rules.goal_spend), "within the goal's bound");
    }
    assert!(within(run.deployment_spent, run.budget, rules.deployment_spend), "within the deployment's bound");
    assert_gates_allowed(rules, run.budget, run.goal, accepted, gates);
}

fn assert_gates_allowed(rules: &Rules, cost: u64, goal: Goal, accepted: Option<Permission>, gates: &[Gate]) {
    let before = match goal {
        Goal::Spent(before) => before,
        Goal::Outside => 0,
    };
    for gate in gates {
        match gate {
            Gate::Spend { most } => assert!(cost <= *most, "within the step's bound"),
            Gate::GoalSpend { most } => assert!(within(before, cost, *most), "within the plan's budget"),
            Gate::Accepted => assert!(accepted >= Some(rules.acts.accept), "accepted by someone who may"),
            Gate::ReadOnly | Gate::Ci | Gate::Review { .. } | Gate::Approvals(_) => {}
        }
    }
}

fn assert_write_allowed(rules: &Rules, write: &Write, accepted: Option<Permission>, gates: &[Gate]) {
    let read_only = gated(gates, Gate::ReadOnly);
    match write {
        Write::Item { repository } => assert!(ours(rules, *repository)),
        Write::Open { repository, .. } => assert!(ours(rules, *repository) && !read_only),
        Write::Delete { repository, branch } => {
            assert!(ours(rules, *repository) && !read_only);
            assert!(!is_protected(rules, *repository, branch), "a protected branch is never deleted");
        }
        Write::Land(landing) => {
            assert!(ours(rules, landing.repository) && !read_only);
            let protected = is_protected(rules, landing.repository, &landing.base);
            if protected || gated(gates, Gate::Ci) {
                assert!(landing.ci == Ci::Passed && landing.ci_head == landing.head, "green CI on the exact head");
            }
            if protected {
                assert!(approved_at(rules, landing, rules.reviewer), "an approval on the exact head, none against");
            }
            for gate in gates {
                match gate {
                    Gate::Review { permission } => assert!(approved_at(rules, landing, *permission)),
                    Gate::Approvals(want) => {
                        assert!(people_approving(rules, landing) >= *want);
                    }
                    Gate::ReadOnly | Gate::Ci | Gate::Accepted | Gate::Spend { .. } | Gate::GoalSpend { .. } => {}
                }
            }
        }
        Write::Plan(plan) => {
            assert!(ours(rules, plan.repository));
            let mut lands = false;
            for target in &plan.lands {
                assert!(ours(rules, target.repository), "a plan lands only in the deployment's repositories");
                lands |= is_protected(rules, target.repository, &target.branch);
            }
            if plan.steps > rules.plan_steps || plan.spend > rules.plan_spend || lands {
                assert!(
                    accepted >= Some(rules.plan_acceptance),
                    "a large plan, or one landing on a protected branch, accepted"
                );
            }
            let spent = match plan.goal {
                Goal::Spent(spent) => spent,
                Goal::Outside => 0,
            };
            assert!(within(spent, plan.spend, rules.goal_spend), "within the goal's bound");
        }
        Write::Note { repository, scope } => {
            assert!(ours(rules, *repository));
            let wanted = match scope {
                Scope::Goal => None,
                Scope::Repository => rules.repository_notes,
                Scope::Deployment => rules.deployment_notes,
            };
            if let Some(permission) = wanted {
                assert!(accepted >= Some(permission), "a wide note accepted");
            }
        }
    }
    let spend = match write {
        Write::Plan(plan) => plan.spend,
        Write::Item { .. } | Write::Open { .. } | Write::Land(_) | Write::Delete { .. } | Write::Note { .. } => 0,
    };
    let goal = match write {
        Write::Plan(plan) => plan.goal,
        Write::Item { .. } | Write::Open { .. } | Write::Land(_) | Write::Delete { .. } | Write::Note { .. } => {
            Goal::Outside
        }
    };
    // A step's own spending bound is its run's, not its writes'.
    let mut writes = List::with_capacity(LIMITS.gates);
    for gate in gates {
        match gate {
            Gate::Spend { .. } => {}
            Gate::ReadOnly
            | Gate::Ci
            | Gate::Review { .. }
            | Gate::Approvals(_)
            | Gate::Accepted
            | Gate::GoalSpend { .. } => writes.push(*gate).unwrap(),
        }
    }
    assert_gates_allowed(rules, spend, goal, accepted, writes.as_slice());
}

#[test]
fn no_combination_of_gates_loosens_a_rule_and_every_allow_keeps_the_rules() {
    let mut rng = Rng::new(0x5EED_0000_0000_6A7E);
    let mut seen = Tightened { to_accept: 0, to_wait: 0, to_refuse: 0, unchanged: 0, refused: 0, allowed: 0 };
    for _ in 0..40_000_u32 {
        let rules = random_rules(&mut rng);
        assert!(rules.fits(&LIMITS));
        let conditions = random_gates(&mut rng);
        let accepted = maybe(&mut rng);
        if rng.chance(500) {
            let write = random_write(&mut rng);
            let answer = write_accepted(&rules, &write, accepted, &conditions);
            compare(&write_accepted(&rules, &write, accepted, &[]), &answer, &mut seen);
            if answer.0 == Decision::Allow {
                assert_write_allowed(&rules, &write, accepted, &conditions);
                seen.allowed += 1;
            }
        } else {
            let run = random_run(&mut rng);
            let answer = run_accepted(&rules, &run, accepted, &conditions);
            compare(&run_accepted(&rules, &run, accepted, &[]), &answer, &mut seen);
            if answer.0 == Decision::Allow {
                assert_run_allowed(&rules, &run, accepted, &conditions);
                seen.allowed += 1;
            }
        }
    }
    let Tightened { to_accept, to_wait, to_refuse, unchanged, refused, allowed } = seen;
    for count in [to_accept, to_wait, to_refuse, unchanged, refused, allowed] {
        assert!(count > 20, "gates tightened answers every way, and left some: {seen:?}");
    }
}
