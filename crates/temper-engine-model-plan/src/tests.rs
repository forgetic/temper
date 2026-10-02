//! Give the plan data, inspect its decisions.

use alloc::boxed::Box;
use core::mem::size_of;

use temper_lib::{Duration, Env, List, Queue, Time};

use crate::{
    Accept, Action, AgentSpec, Applied, Batch, Budget, ChangeSpec, Charter, Ci, Commit, Config, Decision, Due, Entry,
    Envelope, Facts, Finish, Gate, Goal, Grants, Growing, Growth, Hold, Inbound, Key, Limits, Mergeable, Outcome, Plan,
    Problem, Problems, Progress, Pull, PullState, Record, Relations, Repair, Repo, Repository, Resume, Review,
    Reviewed, Run, Sections, SessionSpec, Source, Sources, Stale, Step, Target, Template, Then, Verdict, WaitSpec,
    Waits, Wake, Why, Woken, Work, Write, accept, apply, check, due, grow, max_out, wake, worst_case,
};

/// The most a run may ask for.
const BUDGET: Budget = Budget { tokens: 1_000, turns: 20, time: Duration::from_secs(600) };

const LIMITS: Limits = Limits {
    steps: 8,
    name_bytes: 16,
    dependencies: 4,
    gates: 2,
    targets: 2,
    instruction_bytes: 64,
    tasks: 3,
    events: 8,
    repairs: 2,
    budget: BUDGET,
};

fn env() -> Env<Limits> {
    Env { now: Time::from_nanos(1_000), limits: LIMITS }
}

fn bytes(text: &str) -> Box<[u8]> {
    Box::from(text.as_bytes())
}

/// Two repositories: changes land into `main` or `feat` of the first, and
/// `main` of the second. One template, `fix`.
fn config() -> Config {
    Config {
        repositories: Box::new([
            Repo { bases: Box::new([bytes("main"), bytes("feat")]) },
            Repo { bases: Box::new([bytes("main")]) },
        ]),
        templates: Box::new([Template { name: bytes("fix"), guidance: bytes("fix it") }]),
    }
}

const GRANTS: Grants = Grants { modify: false, shell: false, forge: true, subagents: false, note: false };

/// Woken by its own changes, its relations and messages, one event at a time.
const WAKE: Wake = Wake {
    on: Sources { own: true, related: true, subscribed: false, messages: true },
    every: None,
    batch: Batch { count: 1, age: None },
};

fn charter(tokens: u64) -> Charter {
    Charter { instructions: bytes("do it"), template: None, grants: GRANTS, budget: Budget { tokens, ..BUDGET } }
}

fn names(after: &[&str]) -> Box<[Box<[u8]>]> {
    let mut names = List::with_capacity(u32::try_from(after.len()).expect("a test's few names"));
    for name in after {
        names.push(bytes(name)).expect("room for each name");
    }
    names.into_boxed()
}

fn step(name: &str, work: Work, after: &[&str]) -> Step {
    Step { name: bytes(name), repository: Repository(0), work, after: names(after), gates: Box::new([]), wake: WAKE }
}

fn agent(name: &str, after: &[&str]) -> Step {
    step(name, Work::Agent(AgentSpec { charter: charter(100), grows: false }), after)
}

fn agent_with(name: &str, charter: Charter) -> Step {
    step(name, Work::Agent(AgentSpec { charter, grows: false }), &[])
}

/// Into `main`, reviewed by a person.
fn change_spec() -> ChangeSpec {
    ChangeSpec { base: bytes("main"), produce: charter(100), checks: true, review: Review::Person }
}

fn change(name: &str, after: &[&str]) -> Step {
    step(name, Work::Change(change_spec()), after)
}

fn change_with(name: &str, spec: ChangeSpec) -> Step {
    step(name, Work::Change(spec), &[])
}

fn wait(name: &str, spec: WaitSpec, after: &[&str]) -> Step {
    step(name, Work::Wait(spec), after)
}

fn session(name: &str) -> Step {
    step(name, Work::Session(SessionSpec { charter: charter(100), resume: Resume::Default }), &[])
}

fn gated(step: Step, gates: Box<[Gate]>) -> Step {
    Step { gates, ..step }
}

/// Room for two agent steps, two changes into `feat` and a wait.
fn envelope() -> Envelope {
    Envelope {
        agents: 2,
        changes: 2,
        waits: 1,
        sessions: 0,
        into: Box::new([Target { repository: Repository(0), base: bytes("feat") }]),
    }
}

fn plan(steps: Box<[Step]>) -> Plan {
    Plan { steps, envelope: envelope(), budget: 10_000 }
}

/// `count` agent steps with no name, coming after `after`.
fn nameless(count: u32, after: &[&str]) -> Box<[Step]> {
    let mut steps = List::with_capacity(count);
    for _ in 0..count {
        steps.push(agent("", after)).expect("room for each step");
    }
    steps.into_boxed()
}

/// The problems `plan` has, all of them listed.
fn problems(plan: &Plan) -> Box<[Problem]> {
    match check(&config(), &env(), plan) {
        Ok(estimate) => panic!("a plan with problems passed, estimated at {estimate}"),
        Err(Problems { listed, more }) => {
            assert_eq!(more, 0, "the tests' plans have few problems");
            listed
        }
    }
}

// Checks.

#[test]
fn a_plan_that_fits_passes_with_its_estimate() {
    let plan = plan(Box::new([
        agent("spike-a", &[]),
        agent("spike-b", &[]),
        wait("decide", WaitSpec::Decision, &["spike-a", "spike-b"]),
        change("design", &["decide"]),
        session("chat"),
    ]));
    assert_eq!(check(&config(), &env(), &plan), Ok(400));
}

#[test]
fn a_change_reviewed_by_an_agent_is_estimated_at_both_runs() {
    let design = change_with("design", ChangeSpec { review: Review::Agent(charter(50)), ..change_spec() });
    assert_eq!(check(&config(), &env(), &plan(Box::new([design]))), Ok(150));
}

#[test]
fn a_plan_without_steps_is_refused() {
    assert_eq!(*problems(&plan(Box::new([]))), [Problem::NoSteps]);
}

#[test]
fn a_plan_beyond_its_size_is_refused_before_its_steps_are_read() {
    let steps = nameless(LIMITS.steps + 1, &["nothing"]);
    assert_eq!(*problems(&plan(steps)), [Problem::TooManySteps { max: LIMITS.steps }]);
}

#[test]
fn names_are_present_short_and_unique() {
    let plan = plan(Box::new([agent("", &[]), agent("a-name-past-sixteen", &[]), agent("a", &[]), agent("a", &[])]));
    assert_eq!(
        *problems(&plan),
        [
            Problem::EmptyName { step: 0 },
            Problem::LongName { step: 1, max: LIMITS.name_bytes },
            Problem::NameTaken { step: 3 },
        ]
    );
}

#[test]
fn steps_are_in_the_deployments_repositories() {
    let elsewhere = Step { repository: Repository(2), ..agent("a", &[]) };
    assert_eq!(*problems(&plan(Box::new([elsewhere]))), [Problem::UnknownRepository { step: 0 }]);
}

#[test]
fn changes_land_into_the_deployments_branches() {
    let other =
        Step { repository: Repository(1), ..change_with("a", ChangeSpec { base: bytes("feat"), ..change_spec() }) };
    let long = change_with("b", ChangeSpec { base: bytes("a-branch-past-sixteen"), ..change_spec() });
    assert_eq!(
        *problems(&plan(Box::new([other, long]))),
        [Problem::UnknownBase { step: 0 }, Problem::LongName { step: 1, max: LIMITS.name_bytes }]
    );
}

#[test]
fn charters_name_known_templates_and_fit_the_limits() {
    let named = agent_with("a", Charter { template: Some(bytes("fix")), ..charter(1) });
    let unknown = agent_with("b", Charter { template: Some(bytes("refactor")), ..charter(1) });
    let long = agent_with("c", Charter { instructions: Box::from([b'x'; 65].as_slice()), ..charter(1) });
    let large = change_with(
        "d",
        ChangeSpec {
            review: Review::Agent(Charter { budget: Budget { turns: 21, ..BUDGET }, ..charter(1) }),
            ..change_spec()
        },
    );
    assert_eq!(
        *problems(&plan(Box::new([named, unknown, long, large]))),
        [
            Problem::UnknownTemplate { step: 1 },
            Problem::LongInstructions { step: 2, max: LIMITS.instruction_bytes },
            Problem::LargeBudget { step: 3 },
        ]
    );
}

#[test]
fn dependencies_name_steps_of_the_plan() {
    let plan =
        plan(Box::new([agent("a", &["b"]), agent("b", &["a-step-it-lacks"]), agent("c", &["a", "b", "a", "b", "a"])]));
    assert_eq!(
        *problems(&plan),
        [
            Problem::UnknownDependency { step: 1, dependency: 0 },
            Problem::TooManyDependencies { step: 2, max: LIMITS.dependencies },
        ]
    );
}

#[test]
fn a_step_after_itself_is_a_cycle() {
    assert_eq!(*problems(&plan(Box::new([agent("a", &["a"])]))), [Problem::Cycle { step: 0 }]);
}

#[test]
fn a_cycle_is_reported_on_a_step_on_it_not_one_after_it() {
    // d comes after the cycle of b and c, which comes after a.
    let plan = plan(Box::new([agent("d", &["c"]), agent("a", &[]), agent("b", &["a", "c"]), agent("c", &["b"])]));
    let found = problems(&plan);
    assert_eq!(found.len(), 1, "one cycle: {found:?}");
    assert!(
        found[0] == Problem::Cycle { step: 2 } || found[0] == Problem::Cycle { step: 3 },
        "a step on the cycle: {found:?}"
    );
}

#[test]
fn steps_named_twice_are_ordered_by_the_first() {
    // The second a comes after b, which comes after the first a: no cycle.
    let plan = plan(Box::new([agent("a", &[]), agent("b", &["a"]), agent("a", &["b"])]));
    assert_eq!(*problems(&plan), [Problem::NameTaken { step: 2 }]);
}

#[test]
fn a_diamond_is_ordered() {
    let plan = plan(Box::new([agent("d", &["b", "c"]), agent("b", &["a"]), agent("c", &["a"]), agent("a", &[])]));
    assert_eq!(check(&config(), &env(), &plan), Ok(400));
}

#[test]
fn gates_fit_their_primitive_and_ask_for_something() {
    let plan = plan(Box::new([
        gated(change("a", &[]), Box::new([Gate::Approvals(2), Gate::Accepted])),
        gated(agent("b", &[]), Box::new([Gate::Accepted])),
        gated(agent("c", &[]), Box::new([Gate::Accepted, Gate::Approvals(1)])),
        gated(change("d", &[]), Box::new([Gate::Approvals(0)])),
        gated(wait("e", WaitSpec::Steps, &[]), Box::new([Gate::Accepted, Gate::Accepted, Gate::Accepted])),
    ]));
    assert_eq!(
        *problems(&plan),
        [
            Problem::Gate { step: 2, gate: 1 },
            Problem::Gate { step: 3, gate: 0 },
            Problem::TooManyGates { step: 4, max: LIMITS.gates },
        ]
    );
}

#[test]
fn a_wake_rule_batches_at_least_one_event() {
    let never = Step { wake: Wake { batch: Batch { count: 0, age: None }, ..WAKE }, ..session("a") };
    assert_eq!(*problems(&plan(Box::new([never]))), [Problem::Wake { step: 0 }]);
}

#[test]
fn an_envelope_lets_changes_land_only_where_the_deployment_does() {
    let unknown = Plan {
        envelope: Envelope {
            into: Box::new([
                Target { repository: Repository(1), base: bytes("main") },
                Target { repository: Repository(1), base: bytes("feat") },
            ]),
            ..envelope()
        },
        ..plan(Box::new([agent("a", &[])]))
    };
    assert_eq!(*problems(&unknown), [Problem::UnknownTarget { target: 1 }]);
    let wide = Plan {
        envelope: Envelope {
            into: Box::new([
                Target { repository: Repository(0), base: bytes("main") },
                Target { repository: Repository(0), base: bytes("feat") },
                Target { repository: Repository(1), base: bytes("main") },
            ]),
            ..envelope()
        },
        ..plan(Box::new([agent("a", &[])]))
    };
    assert_eq!(*problems(&wide), [Problem::TooManyTargets { max: LIMITS.targets }]);
}

#[test]
fn a_plan_estimated_over_its_budget_is_refused() {
    let over = Plan { budget: 199, ..plan(Box::new([agent("a", &[]), change("b", &[])])) };
    assert_eq!(*problems(&over), [Problem::OverBudget { estimate: 200, budget: 199 }]);
}

#[test]
fn problems_past_the_listed_are_counted() {
    let steps = nameless(LIMITS.steps, &[]);
    let Err(found) = check(&config(), &env(), &plan(steps)) else {
        panic!("a plan of nameless steps passed");
    };
    // Each of the eight steps has no name.
    assert_eq!(found.listed.len(), 8);
    assert_eq!(found.more, 0);
    let steps = nameless(LIMITS.steps, &["x"]);
    let Err(found) = check(&config(), &env(), &plan(steps)) else {
        panic!("a plan of nameless steps passed");
    };
    // And comes after a step the plan does not have.
    assert_eq!(found.listed.len(), 8);
    assert_eq!(found.more, 8);
}

// Accepting and growing.

fn out() -> Queue<Write> {
    Queue::with_capacity(max_out(&LIMITS))
}

/// What `out` holds, taken out.
fn writes(out: &mut Queue<Write>) -> Box<[Write]> {
    let mut taken = List::with_capacity(max_out(&LIMITS));
    while let Some(write) = out.pop() {
        taken.push(write).expect("room for every write");
    }
    taken.into_boxed()
}

fn created(step: &Step) -> Write {
    Write::Create {
        key: Key::Step(step.name.clone()),
        record: Box::new(Record { step: step.clone(), progress: Progress::NEW, goal: None }),
    }
}

fn entry(name: &str, after: &[&str], parent: Option<u32>) -> Entry {
    Entry { name: bytes(name), after: names(after), parent }
}

/// The goal of the plan of `steps`, as accepted: an agent step and a change.
fn goal() -> Goal {
    Goal {
        steps: Box::new([entry("a", &[], None), entry("b", &["a"], None)]),
        envelope: envelope(),
        budget: 10_000,
        estimate: 200,
        growth: Growth::NONE,
    }
}

#[test]
fn accepting_a_plan_makes_an_item_for_each_step_then_its_goal() {
    let plan = plan(Box::new([agent("a", &[]), change("b", &["a"])]));
    let mut out = out();
    assert_eq!(accept(&config(), &env(), &plan, &mut out), Ok(()));
    assert_eq!(*writes(&mut out), [created(&plan.steps[0]), created(&plan.steps[1]), Write::Goal(goal())]);
}

#[test]
fn a_plan_with_problems_makes_nothing() {
    let mut out = out();
    let refused = accept(&config(), &env(), &plan(Box::new([agent("a", &["a"])])), &mut out);
    assert_eq!(refused, Err(Problems { listed: Box::new([Problem::Cycle { step: 0 }]), more: 0 }));
    assert!(out.is_empty());
}

#[test]
fn growth_within_the_envelope_needs_no_acceptance() {
    let feat = change_with("d", ChangeSpec { base: bytes("feat"), ..change_spec() });
    let steps = [agent("c", &["a"]), feat, wait("e", WaitSpec::Steps, &["c", "d", "b"])];
    let mut out = out();
    assert_eq!(grow(&config(), &env(), &goal(), None, &steps, &mut out), Ok(Growing::Within));
    let grown = Goal {
        steps: Box::new([
            entry("a", &[], None),
            entry("b", &["a"], None),
            entry("c", &["a"], None),
            entry("d", &[], None),
            entry("e", &["c", "d", "b"], None),
        ]),
        estimate: 400,
        growth: Growth { agents: 1, changes: 1, waits: 1, sessions: 0 },
        ..goal()
    };
    assert_eq!(*writes(&mut out), [created(&steps[0]), created(&steps[1]), created(&steps[2]), Write::Goal(grown)]);
}

#[test]
fn growth_beyond_the_envelope_needs_a_persons_acceptance() {
    let mut out = out();
    // A change into a branch the envelope does not name.
    let main = [change("c", &[])];
    assert_eq!(grow(&config(), &env(), &goal(), None, &main, &mut out), Ok(Growing::Beyond));
    assert_eq!(writes(&mut out).len(), 2);
    // A session, of which the envelope allows none.
    assert_eq!(grow(&config(), &env(), &goal(), None, &[session("c")], &mut out), Ok(Growing::Beyond));
    assert_eq!(writes(&mut out).len(), 2);
    // A third agent step, counting the growth so far.
    let grown = Goal { growth: Growth { agents: 2, ..Growth::NONE }, ..goal() };
    assert_eq!(grow(&config(), &env(), &grown, None, &[agent("c", &[])], &mut out), Ok(Growing::Beyond));
    assert_eq!(writes(&mut out).len(), 2);
}

#[test]
fn growth_may_not_come_after_the_step_that_adds_it() {
    // c comes after b, which comes after a: a step a adds that comes after c
    // waits for a, which waits for the steps it added.
    let plan =
        Goal { steps: Box::new([entry("a", &[], None), entry("b", &["a"], None), entry("c", &["b"], None)]), ..goal() };
    let mut out = out();
    let added = [agent("d", &[]), agent("e", &["d", "c"])];
    assert_eq!(
        grow(&config(), &env(), &plan, Some(0), &added, &mut out),
        Err(Problems { listed: Box::new([Problem::Cycle { step: 1 }]), more: 0 })
    );
    // Added by c, or by the goal's session, the same steps make no cycle.
    assert_eq!(grow(&config(), &env(), &plan, Some(2), &[agent("d", &["b"])], &mut out), Ok(Growing::Within));
    assert_eq!(grow(&config(), &env(), &plan, None, &added, &mut out), Ok(Growing::Within));
    // Nor through a step an earlier growth added to a.
    let grown = Goal {
        steps: Box::new([
            entry("a", &[], None),
            entry("b", &["a"], None),
            entry("c", &["b"], None),
            entry("d", &[], Some(2)),
        ]),
        ..goal()
    };
    let refused = grow(&config(), &env(), &grown, Some(3), &[agent("e", &["c"])], &mut out);
    assert_eq!(refused, Err(Problems { listed: Box::new([Problem::Cycle { step: 0 }]), more: 0 }));
}

#[test]
fn growth_is_checked_against_the_plan_it_joins() {
    let mut out = out();
    let refused = grow(&config(), &env(), &goal(), None, &[agent("a", &[]), agent("c", &["d", "c"])], &mut out);
    assert_eq!(
        refused,
        Err(Problems {
            listed: Box::new([
                Problem::NameTaken { step: 0 },
                Problem::UnknownDependency { step: 1, dependency: 0 },
                Problem::Cycle { step: 1 },
            ]),
            more: 0,
        })
    );
    assert_eq!(
        grow(&config(), &env(), &goal(), None, &[], &mut out),
        Err(Problems { listed: Box::new([Problem::NoSteps]), more: 0 })
    );
    let spent = Goal { estimate: 9_950, ..goal() };
    assert_eq!(
        grow(&config(), &env(), &spent, None, &[agent("c", &[])], &mut out),
        Err(Problems { listed: Box::new([Problem::OverBudget { estimate: 10_050, budget: 10_000 }]), more: 0 })
    );
    let full = nameless(LIMITS.steps - 1, &[]);
    assert_eq!(
        grow(&config(), &env(), &goal(), None, &full, &mut out),
        Err(Problems { listed: Box::new([Problem::TooManySteps { max: LIMITS.steps }]), more: 0 })
    );
    assert!(out.is_empty());
}

// What is due.

/// An item made at 100, with no relations, nothing pushed and nothing
/// decided, its inbox quiet.
fn facts() -> Facts {
    Facts {
        created: Time::from_nanos(100),
        dependencies: Relations::NONE,
        children: Relations::NONE,
        branch: None,
        pull: None,
        decision: None,
        snapshot: false,
        woken: false,
    }
}

fn record(step: Step) -> Record {
    Record { step, progress: Progress::NEW, goal: None }
}

fn head(count: u8) -> Commit {
    Commit([count; 32])
}

/// An open pull request at `head` that is ready to merge: CI passed, a person
/// approves, it merges cleanly.
fn ready(head: Commit) -> Pull {
    Pull {
        head,
        state: PullState::Open,
        ci: Ci::Passed,
        approvals: 1,
        changes_requested: false,
        merge: Mergeable::Clean,
        base_moved: false,
    }
}

fn pulled(pull: Pull) -> Facts {
    Facts { branch: Some(pull.head), pull: Some(pull), ..facts() }
}

/// What is due for `record`, and the writes it asks for.
fn decide(record: &Record, facts: &Facts) -> (Due, Box<[Write]>) {
    let mut out = out();
    let due = due(&config(), &env(), record, facts, &mut out);
    (due, writes(&mut out))
}

/// What is due, which writes nothing.
fn decided(record: &Record, facts: &Facts) -> Due {
    let (due, writes) = decide(record, facts);
    assert!(writes.is_empty(), "{due:?} writes nothing: {writes:?}");
    due
}

/// The run that is due.
fn ran(record: &Record, facts: &Facts) -> Run {
    let due = decided(record, facts);
    let Due::Run(run) = due else {
        panic!("a run is due, not {due:?}");
    };
    run
}

/// What every brief carries.
const SECTIONS: Sections = Sections {
    item: true,
    comments: true,
    dependencies: false,
    ci: false,
    reviews: false,
    pull: false,
    attempts: true,
    plan: false,
    notes: true,
    template: false,
};

const DONE: Relations = Relations { total: 2, done: 2, last_done: Some(Time::from_nanos(500)) };
const HALF: Relations = Relations { total: 2, done: 1, last_done: Some(Time::from_nanos(500)) };

#[test]
fn every_step_waits_for_its_dependencies() {
    let waiting = Facts { dependencies: HALF, ..facts() };
    for step in [agent("a", &[]), change("b", &[]), wait("c", WaitSpec::Steps, &[]), session("d")] {
        assert_eq!(decided(&record(step), &waiting), Due::Nothing(Waits::Dependencies));
    }
}

#[test]
fn an_agent_step_runs_with_its_charter() {
    let step = agent_with("a", Charter { template: Some(bytes("fix")), ..charter(70) });
    let run = ran(&record(step), &Facts { dependencies: DONE, ..facts() });
    assert_eq!(
        run,
        Run {
            why: Why::Work,
            sections: Sections { dependencies: true, template: true, ..SECTIONS },
            finish: Finish::Report { grows: false },
            grants: GRANTS,
            budget: Budget { tokens: 70, ..BUDGET },
            instructions: bytes("do it"),
            template: Some(0),
            resume: false,
        }
    );
}

#[test]
fn a_template_the_configuration_lost_is_left_out() {
    let step = agent_with("a", Charter { template: Some(bytes("refactor")), ..charter(70) });
    let run = ran(&record(step), &facts());
    assert_eq!((run.template, run.sections), (None, SECTIONS));
}

#[test]
fn an_agent_step_that_grows_its_plan_sees_the_plan() {
    let step = step("a", Work::Agent(AgentSpec { charter: charter(1), grows: true }), &[]);
    let run = ran(&record(step), &facts());
    assert_eq!((run.finish, run.sections), (Finish::Report { grows: true }, Sections { plan: true, ..SECTIONS }));
}

#[test]
fn a_gate_of_acceptance_waits_for_a_persons_decision() {
    for step in [agent("a", &[]), wait("b", WaitSpec::Steps, &[]), session("c")] {
        let gated = record(gated(step, Box::new([Gate::Accepted])));
        let woken = Facts { woken: true, ..facts() };
        assert_eq!(decided(&gated, &woken), Due::Nothing(Waits::Acceptance));
        let rejected = Facts { decision: Some(Decision::Rejected), ..woken };
        assert_eq!(decided(&gated, &rejected), Due::Hold(Hold::Rejected));
        let accepted = Facts { decision: Some(Decision::Accepted), ..woken };
        assert_ne!(decide(&gated, &accepted).0, Due::Nothing(Waits::Acceptance));
    }
}

#[test]
fn a_finished_agent_step_is_done_once_its_children_are() {
    let finished = Record { progress: Progress { finished: true, ..Progress::NEW }, ..record(agent("a", &[])) };
    assert_eq!(decided(&finished, &Facts { children: HALF, ..facts() }), Due::Nothing(Waits::Children));
    assert_eq!(decide(&finished, &Facts { children: DONE, ..facts() }), (Due::Done, Box::from([Write::Close])));
    assert_eq!(decide(&finished, &facts()), (Due::Done, Box::from([Write::Close])));
}

#[test]
fn a_wait_on_steps_is_done_with_them() {
    let after = record(wait("a", WaitSpec::Steps, &["b"]));
    assert_eq!(decide(&after, &Facts { dependencies: DONE, ..facts() }), (Due::Done, Box::from([Write::Close])));
}

#[test]
fn a_wait_on_a_decision_ends_when_a_person_accepts() {
    let decision = record(wait("a", WaitSpec::Decision, &[]));
    assert_eq!(decided(&decision, &facts()), Due::Nothing(Waits::Decision));
    let accepted = Facts { decision: Some(Decision::Accepted), ..facts() };
    assert_eq!(decide(&decision, &accepted), (Due::Done, Box::from([Write::Close])));
    let rejected = Facts { decision: Some(Decision::Rejected), ..facts() };
    assert_eq!(decided(&decision, &rejected), Due::Hold(Hold::Rejected));
}

#[test]
fn a_wait_on_a_time_counts_from_its_last_dependency_or_its_making() {
    let alone = record(wait("a", WaitSpec::Time(Duration::from_nanos(1_000)), &[]));
    assert_eq!(decided(&alone, &facts()), Due::Nothing(Waits::Time(Time::from_nanos(1_100))));
    let later = Env { now: Time::from_nanos(1_100), limits: LIMITS };
    let mut out = out();
    assert_eq!(due(&config(), &later, &alone, &facts(), &mut out), Due::Done);
    assert_eq!(*writes(&mut out), [Write::Close]);
    let after = record(wait("a", WaitSpec::Time(Duration::from_nanos(1_000)), &["b"]));
    let done = Facts { dependencies: DONE, ..facts() };
    assert_eq!(decided(&after, &done), Due::Nothing(Waits::Time(Time::from_nanos(1_500))));
}

#[test]
fn a_chatting_session_runs_when_woken_and_resumes_its_snapshot() {
    let chatting = record(session("a"));
    assert_eq!(decided(&chatting, &facts()), Due::Nothing(Waits::Wake));
    let run = ran(&chatting, &Facts { woken: true, ..facts() });
    assert_eq!(
        (run.why, run.finish, run.sections, run.resume),
        (Why::Turn, Finish::Turn { supervising: false }, SECTIONS, false)
    );
    let parked = Facts { woken: true, snapshot: true, ..facts() };
    assert!(ran(&chatting, &parked).resume, "chatting, it resumes");
}

#[test]
fn a_supervising_session_starts_fresh_and_ends_with_its_goal() {
    let supervising = Record { goal: Some(goal()), ..record(session("a")) };
    let parked = Facts { woken: true, snapshot: true, children: HALF, ..facts() };
    let run = ran(&supervising, &parked);
    assert_eq!(
        (run.finish, run.sections, run.resume),
        (Finish::Turn { supervising: true }, Sections { plan: true, ..SECTIONS }, false)
    );
    let always = Record {
        step: step("a", Work::Session(SessionSpec { charter: charter(1), resume: Resume::Always }), &[]),
        ..supervising.clone()
    };
    assert!(ran(&always, &parked).resume, "a session that always resumes");
    let never = record(step("a", Work::Session(SessionSpec { charter: charter(1), resume: Resume::Never }), &[]));
    assert!(!ran(&never, &parked).resume, "a session that never resumes");
    let finished = Facts { children: DONE, ..parked };
    assert_eq!(decide(&supervising, &finished), (Due::Done, Box::from([Write::Close])));
}

#[test]
fn a_change_is_produced_then_its_pull_request_opened() {
    let produce = ran(&record(change("a", &[])), &facts());
    assert_eq!(
        (produce.why, produce.finish, produce.budget.tokens),
        (Why::Produce, Finish::Change { checks: true }, 100)
    );
    let pushed = Facts { branch: Some(head(1)), ..facts() };
    assert_eq!(
        decide(&record(change("a", &[])), &pushed),
        (Due::Act(Action::OpenPull), Box::from([Write::OpenPull { base: bytes("main") }]))
    );
}

#[test]
fn a_change_waits_for_ci_on_its_head() {
    let change = record(change("a", &[]));
    for ci in [Ci::None, Ci::Pending] {
        assert_eq!(decided(&change, &pulled(Pull { ci, ..ready(head(1)) })), Due::Nothing(Waits::Ci));
    }
}

#[test]
fn a_change_is_repaired_with_a_brief_that_says_why() {
    let change = record(change("a", &[]));
    let cases = [
        (Pull { ci: Ci::Failed, ..ready(head(1)) }, Repair::CiFailed, Sections { ci: true, ..SECTIONS }),
        (
            Pull { changes_requested: true, ..ready(head(1)) },
            Repair::ChangesRequested,
            Sections { reviews: true, ..SECTIONS },
        ),
        (Pull { base_moved: true, ..ready(head(1)) }, Repair::BaseMoved, Sections { pull: true, ..SECTIONS }),
        (
            Pull { merge: Mergeable::Conflicts, ci: Ci::Pending, ..ready(head(1)) },
            Repair::Conflicts,
            Sections { pull: true, ..SECTIONS },
        ),
    ];
    for (pull, repair, sections) in cases {
        let run = ran(&change, &pulled(pull));
        assert_eq!(
            (run.why, run.finish, run.sections),
            (Why::Repair(repair), Finish::Change { checks: true }, sections)
        );
    }
}

#[test]
fn a_change_repaired_to_the_limit_is_held_when_it_needs_more() {
    let repaired =
        Record { progress: Progress { repairs: LIMITS.repairs, ..Progress::NEW }, ..record(change("a", &[])) };
    let failed = pulled(Pull { ci: Ci::Failed, ..ready(head(1)) });
    assert_eq!(decided(&repaired, &failed), Due::Hold(Hold::Repairs));
    assert_eq!(decide(&repaired, &pulled(ready(head(1)))).0, Due::Act(Action::Merge));
}

#[test]
fn a_change_rebases_onto_a_moved_base_past_its_repairs() {
    let repaired =
        Record { progress: Progress { repairs: LIMITS.repairs, ..Progress::NEW }, ..record(change("a", &[])) };
    let moved = pulled(Pull { base_moved: true, ..ready(head(1)) });
    assert_eq!(ran(&repaired, &moved).why, Why::Repair(Repair::BaseMoved));
}

#[test]
fn a_change_reviewed_by_a_person_waits_for_an_approval_of_its_head() {
    let change = record(change("a", &[]));
    assert_eq!(decided(&change, &pulled(Pull { approvals: 0, ..ready(head(1)) })), Due::Nothing(Waits::Review));
}

#[test]
fn a_change_lands_at_exactly_its_head_once_it_merges_cleanly() {
    let change = record(change("a", &[]));
    let unknown = pulled(Pull { merge: Mergeable::Unknown, ..ready(head(1)) });
    assert_eq!(decided(&change, &unknown), Due::Nothing(Waits::Mergeable));
    assert_eq!(
        decide(&change, &pulled(ready(head(1)))),
        (Due::Act(Action::Merge), Box::from([Write::Merge { head: head(1) }]))
    );
    let merged = pulled(Pull { state: PullState::Merged, ..ready(head(1)) });
    assert_eq!(decide(&change, &merged), (Due::Done, Box::from([Write::Close, Write::DeleteBranch])));
    let closed = pulled(Pull { state: PullState::Closed, ..ready(head(1)) });
    assert_eq!(decided(&change, &closed), Due::Hold(Hold::PullClosed));
}

#[test]
fn a_changes_gates_add_to_its_review() {
    let gates: Box<[Gate]> = Box::new([Gate::Approvals(2), Gate::Accepted]);
    let change = record(gated(change("a", &[]), gates));
    assert_eq!(decided(&change, &pulled(ready(head(1)))), Due::Nothing(Waits::Approvals));
    let approved = pulled(Pull { approvals: 2, ..ready(head(1)) });
    assert_eq!(decided(&change, &approved), Due::Nothing(Waits::Acceptance));
    let rejected = Facts { decision: Some(Decision::Rejected), ..approved };
    assert_eq!(decided(&change, &rejected), Due::Hold(Hold::Rejected));
    let accepted = Facts { decision: Some(Decision::Accepted), ..approved };
    assert_eq!(decide(&change, &accepted).0, Due::Act(Action::Merge));
    // Before it lands, the acceptance gate waits for nothing.
    assert_eq!(ran(&change, &facts()).why, Why::Produce);
}

/// `change`, reviewed by an agent at `head`.
fn reviewed(change: &Record, verdict: Verdict, head: Commit) -> Record {
    Record { progress: Progress { review: Some(Reviewed { head, verdict }), ..Progress::NEW }, ..change.clone() }
}

#[test]
fn a_change_reviewed_by_an_agent_runs_a_review_of_its_exact_head() {
    let spec = ChangeSpec { review: Review::Agent(charter(30)), ..change_spec() };
    let change = record(change_with("a", spec));
    let unapproved = pulled(Pull { approvals: 0, ..ready(head(2)) });
    let review = ran(&change, &unapproved);
    assert_eq!(
        (review.why, review.finish, review.sections, review.budget.tokens),
        (Why::Review { head: head(2) }, Finish::Verdict, Sections { reviews: true, ..SECTIONS }, 30)
    );
    assert_eq!(decide(&reviewed(&change, Verdict::Approve, head(2)), &unapproved).0, Due::Act(Action::Merge));
    let repair = ran(&reviewed(&change, Verdict::Changes, head(2)), &unapproved);
    assert_eq!(repair.why, Why::Repair(Repair::ChangesRequested));
    // A verdict on an earlier head counts for nothing.
    assert_eq!(ran(&reviewed(&change, Verdict::Approve, head(1)), &unapproved).why, Why::Review { head: head(2) });
}

// Wakes.

fn inbound(source: Source, at: u64) -> Inbound {
    Inbound { source, at: Time::from_nanos(at) }
}

/// Woken by its own changes and its relations, three at a time or once the
/// oldest is 200 old; by messages; and every 5000 after its last turn.
const BATCHED: Wake = Wake {
    on: Sources { own: true, related: true, subscribed: false, messages: true },
    every: Some(Duration::from_nanos(5_000)),
    batch: Batch { count: 3, age: Some(Duration::from_nanos(200)) },
};

/// The last turn, long enough ago for the timer not to matter.
const LAST: Time = Time::from_nanos(900);

#[test]
fn events_a_rule_does_not_name_wake_nothing() {
    assert_eq!(wake(&env(), &WAKE, &[], LAST), Woken::No);
    assert_eq!(wake(&env(), &WAKE, &[inbound(Source::Subscribed, 990)], LAST), Woken::No);
    let deaf = Wake { on: Sources { own: false, related: false, subscribed: false, messages: false }, ..WAKE };
    let all = [
        inbound(Source::Own, 990),
        inbound(Source::Dependency, 990),
        inbound(Source::Child, 990),
        inbound(Source::Subscribed, 990),
        inbound(Source::Message, 990),
    ];
    assert_eq!(wake(&env(), &deaf, &all, LAST), Woken::No);
}

#[test]
fn an_event_a_rule_names_wakes_at_once_without_a_batch() {
    for source in [Source::Own, Source::Dependency, Source::Child, Source::Message] {
        assert_eq!(wake(&env(), &WAKE, &[inbound(source, 990)], LAST), Woken::Now);
    }
    let subscribed = Wake { on: Sources { subscribed: true, ..WAKE.on }, ..WAKE };
    assert_eq!(wake(&env(), &subscribed, &[inbound(Source::Subscribed, 990)], LAST), Woken::Now);
}

#[test]
fn a_batch_wakes_once_it_is_full_or_its_oldest_is_old() {
    let two = [inbound(Source::Own, 900), inbound(Source::Child, 950)];
    assert_eq!(wake(&env(), &BATCHED, &two, LAST), Woken::At(Time::from_nanos(1_100)));
    let three = [inbound(Source::Own, 900), inbound(Source::Child, 950), inbound(Source::Dependency, 990)];
    assert_eq!(wake(&env(), &BATCHED, &three, LAST), Woken::Now);
    let old = [inbound(Source::Own, 800), inbound(Source::Child, 950)];
    assert_eq!(wake(&env(), &BATCHED, &old, LAST), Woken::Now);
    let counted = Wake { batch: Batch { count: 3, age: None }, ..BATCHED };
    assert_eq!(wake(&env(), &counted, &two, LAST), Woken::At(Time::from_nanos(5_900)));
    let untimed = Wake { every: None, ..counted };
    assert_eq!(wake(&env(), &untimed, &two, LAST), Woken::No);
}

#[test]
fn a_message_is_never_batched() {
    let message = [inbound(Source::Own, 990), inbound(Source::Message, 995)];
    assert_eq!(wake(&env(), &BATCHED, &message, LAST), Woken::Now);
    let unheard = Wake { on: Sources { messages: false, ..BATCHED.on }, ..BATCHED };
    assert_eq!(wake(&env(), &unheard, &message, LAST), Woken::At(Time::from_nanos(1_190)));
}

#[test]
fn a_timer_wakes_a_while_after_the_last_turn() {
    let timed = Wake { every: Some(Duration::from_nanos(500)), ..WAKE };
    assert_eq!(wake(&env(), &timed, &[], Time::from_nanos(600)), Woken::At(Time::from_nanos(1_100)));
    assert_eq!(wake(&env(), &timed, &[], Time::from_nanos(500)), Woken::Now);
    // The sooner of the timer and the batch.
    let both = Wake { every: Some(Duration::from_nanos(100)), ..BATCHED };
    let one = [inbound(Source::Own, 990)];
    assert_eq!(wake(&env(), &both, &one, Time::from_nanos(900)), Woken::Now);
    assert_eq!(wake(&env(), &both, &one, Time::from_nanos(980)), Woken::At(Time::from_nanos(1_080)));
}

#[test]
fn only_as_many_events_as_the_limits_say_are_read() {
    let mut inbox = [inbound(Source::Subscribed, 990); 10];
    inbox[8] = inbound(Source::Own, 990);
    inbox[9] = inbound(Source::Message, 990);
    assert_eq!(wake(&env(), &WAKE, &inbox, LAST), Woken::No);
    inbox[7] = inbound(Source::Own, 990);
    assert_eq!(wake(&env(), &WAKE, &inbox, LAST), Woken::Now);
}

// What outcomes write.

/// What `outcome` writes for `record`, under `goal`, and the writes.
fn apply_to(record: &Record, goal: Option<&Goal>, facts: &Facts, outcome: &Outcome) -> (Applied, Box<[Write]>) {
    let mut out = out();
    let applied = apply(&config(), &env(), record, goal, facts, outcome, &mut out);
    (applied, writes(&mut out))
}

const WRITES: Applied = Applied::Writes { accept: Accept::Rules, then: Then::Wait };

fn invalid(problem: Problem) -> (Applied, Box<[Write]>) {
    (Applied::Invalid(Problems { listed: Box::new([problem]), more: 0 }), Box::new([]))
}

fn stale(stale: Stale) -> (Applied, Box<[Write]>) {
    (Applied::Stale(stale), Box::new([]))
}

fn grower(name: &str) -> Step {
    step(name, Work::Agent(AgentSpec { charter: charter(100), grows: true }), &[])
}

#[test]
fn a_change_repaired_for_a_failure_counts_against_the_limit() {
    let change = record(change("a", &[]));
    let pushed = Facts { branch: Some(head(1)), ..facts() };
    let produced = Outcome::Change { head: head(1), repair: None };
    assert_eq!(apply_to(&change, None, &pushed, &produced), (WRITES, Box::from([])));
    let open = pulled(ready(head(2)));
    for repair in [Repair::CiFailed, Repair::ChangesRequested, Repair::Conflicts] {
        assert_eq!(
            apply_to(&change, None, &open, &Outcome::Change { head: head(2), repair: Some(repair) }),
            (WRITES, Box::from([Write::Progress(Progress { repairs: 1, ..Progress::NEW })]))
        );
    }
    let rebased = Outcome::Change { head: head(2), repair: Some(Repair::BaseMoved) };
    assert_eq!(apply_to(&change, None, &open, &rebased), (WRITES, Box::from([])));
}

#[test]
fn a_change_the_item_moved_on_from_is_stale() {
    let change = record(change("a", &[]));
    let outcome = Outcome::Change { head: head(1), repair: None };
    assert_eq!(apply_to(&change, None, &Facts { branch: Some(head(2)), ..facts() }, &outcome), stale(Stale::Moved));
    assert_eq!(apply_to(&change, None, &facts(), &outcome), stale(Stale::Moved));
    let merged = pulled(Pull { state: PullState::Merged, ..ready(head(1)) });
    assert_eq!(apply_to(&change, None, &merged, &outcome), stale(Stale::Landed));
    let closed = pulled(Pull { state: PullState::Closed, ..ready(head(1)) });
    assert_eq!(apply_to(&change, None, &closed, &outcome), stale(Stale::Closed));
}

#[test]
fn an_agents_verdict_is_kept_for_the_head_it_reviewed() {
    let spec = ChangeSpec { review: Review::Agent(charter(30)), ..change_spec() };
    let change = record(change_with("a", spec));
    let verdict = Outcome::Verdict { head: head(2), verdict: Verdict::Changes };
    let reviewed = Progress { review: Some(Reviewed { head: head(2), verdict: Verdict::Changes }), ..Progress::NEW };
    assert_eq!(
        apply_to(&change, None, &pulled(ready(head(2))), &verdict),
        (WRITES, Box::from([Write::Progress(reviewed)]))
    );
    assert_eq!(apply_to(&change, None, &pulled(ready(head(3))), &verdict), stale(Stale::Moved));
    assert_eq!(apply_to(&change, None, &facts(), &verdict), stale(Stale::Moved));
    let merged = pulled(Pull { state: PullState::Merged, ..ready(head(2)) });
    assert_eq!(apply_to(&change, None, &merged, &verdict), stale(Stale::Landed));
    let closed = pulled(Pull { state: PullState::Closed, ..ready(head(2)) });
    assert_eq!(apply_to(&change, None, &closed, &verdict), stale(Stale::Closed));
}

#[test]
fn a_report_finishes_an_agent_step_once() {
    let agent = record(agent("a", &[]));
    assert_eq!(
        apply_to(&agent, None, &facts(), &Outcome::Report),
        (WRITES, Box::from([Write::Progress(Progress { finished: true, ..Progress::NEW })]))
    );
    let finished = Record { progress: Progress { finished: true, ..Progress::NEW }, ..agent };
    assert_eq!(apply_to(&finished, None, &facts(), &Outcome::Report), stale(Stale::Finished));
}

#[test]
fn a_chatting_session_proposes_a_plan_and_becomes_its_goal() {
    let chatting = record(session("chat"));
    let proposed = plan(Box::new([agent("a", &[]), change("b", &["a"])]));
    let (applied, writes) = apply_to(&chatting, None, &facts(), &Outcome::Plan(proposed.clone()));
    assert_eq!(applied, WRITES);
    assert_eq!(*writes, [created(&proposed.steps[0]), created(&proposed.steps[1]), Write::Goal(goal())]);
    let cyclic = Outcome::Plan(plan(Box::new([agent("a", &["a"])])));
    assert_eq!(apply_to(&chatting, None, &facts(), &cyclic), invalid(Problem::Cycle { step: 0 }));
    let supervising = Record { goal: Some(goal()), ..chatting };
    let again = Outcome::Plan(proposed);
    assert_eq!(apply_to(&supervising, Some(&goal()), &facts(), &again), invalid(Problem::NotAllowed));
}

#[test]
fn a_growing_agent_step_adds_steps_and_finishes() {
    let build = record(grower("build"));
    let within = Outcome::Steps(Box::new([agent("c", &["a"])]));
    let (applied, writes) = apply_to(&build, Some(&goal()), &facts(), &within);
    assert_eq!(applied, WRITES);
    assert_eq!(writes.len(), 3, "{writes:?}");
    assert_eq!(writes[2], Write::Progress(Progress { finished: true, ..Progress::NEW }));
    let beyond = Outcome::Steps(Box::new([change("c", &[])]));
    let (applied, writes) = apply_to(&build, Some(&goal()), &facts(), &beyond);
    assert_eq!(applied, Applied::Writes { accept: Accept::Person, then: Then::Wait });
    assert_eq!(writes.len(), 3, "{writes:?}");
    assert_eq!(apply_to(&build, None, &facts(), &within), invalid(Problem::NoGoal));
    let finished = Record { progress: Progress { finished: true, ..Progress::NEW }, ..build };
    assert_eq!(apply_to(&finished, Some(&goal()), &facts(), &within), stale(Stale::Finished));
    let refused = Outcome::Steps(Box::new([agent("a", &[])]));
    let (applied, writes) = apply_to(&record(grower("build")), Some(&goal()), &facts(), &refused);
    assert_eq!(applied, Applied::Invalid(Problems { listed: Box::new([Problem::NameTaken { step: 0 }]), more: 0 }));
    assert!(writes.is_empty());
}

#[test]
fn a_supervising_session_grows_its_goals_plan() {
    let supervising = Record { goal: Some(goal()), ..record(session("chat")) };
    let steps = Outcome::Steps(Box::new([agent("c", &["a"])]));
    let (applied, writes) = apply_to(&supervising, Some(&goal()), &facts(), &steps);
    assert_eq!(applied, WRITES);
    assert_eq!(writes.len(), 2, "an item and the goal: {writes:?}");
    let chatting = record(session("chat"));
    assert_eq!(apply_to(&chatting, None, &facts(), &steps), invalid(Problem::NotAllowed));
}

#[test]
fn only_a_growing_agent_step_or_a_supervising_session_adds_steps() {
    let steps = Outcome::Steps(Box::new([agent("c", &[])]));
    for step in [agent("a", &[]), change("b", &[]), wait("c", WaitSpec::Steps, &[])] {
        assert_eq!(apply_to(&record(step), Some(&goal()), &facts(), &steps), invalid(Problem::NotAllowed));
    }
}

#[test]
fn a_session_makes_tasks_keyed_by_their_place() {
    let chatting = record(session("chat"));
    let tasks = Outcome::Tasks(Box::new([agent("a", &[]), change("b", &[])]));
    let (applied, writes) = apply_to(&chatting, None, &facts(), &tasks);
    assert_eq!(applied, WRITES);
    assert_eq!(
        *writes,
        [
            Write::Create { key: Key::Task(0), record: Box::new(record(agent("a", &[]))) },
            Write::Create { key: Key::Task(1), record: Box::new(record(change("b", &[]))) },
        ]
    );
    let after = Outcome::Tasks(Box::new([agent("a", &[]), agent("b", &["a"])]));
    assert_eq!(
        apply_to(&chatting, None, &facts(), &after),
        invalid(Problem::UnknownDependency { step: 1, dependency: 0 })
    );
    let many = Outcome::Tasks(nameless(LIMITS.tasks + 1, &[]));
    assert_eq!(apply_to(&chatting, None, &facts(), &many), invalid(Problem::TooManyTasks { max: LIMITS.tasks }));
    assert_eq!(apply_to(&chatting, None, &facts(), &Outcome::Tasks(Box::new([]))), invalid(Problem::NoSteps));
    assert_eq!(apply_to(&record(agent("a", &[])), None, &facts(), &tasks), invalid(Problem::NotAllowed));
}

#[test]
fn a_session_replies_and_any_run_escalates() {
    assert_eq!(apply_to(&record(session("chat")), None, &facts(), &Outcome::Reply), (WRITES, Box::from([])));
    assert_eq!(apply_to(&record(agent("a", &[])), None, &facts(), &Outcome::Reply), invalid(Problem::NotAllowed));
    let held = Applied::Writes { accept: Accept::Rules, then: Then::Hold(Hold::Escalated) };
    for step in [agent("a", &[]), change("b", &[]), session("c")] {
        assert_eq!(apply_to(&record(step), None, &facts(), &Outcome::Escalation), (held.clone(), Box::from([])));
    }
}

#[test]
fn an_outcome_of_another_primitives_is_invalid() {
    let wait = record(wait("a", WaitSpec::Steps, &[]));
    let person = record(change("b", &[]));
    let verdict = Outcome::Verdict { head: head(1), verdict: Verdict::Approve };
    let cases = [
        (record(agent("a", &[])), Outcome::Change { head: head(1), repair: None }),
        (record(agent("a", &[])), verdict.clone()),
        (person, verdict),
        (record(session("c")), Outcome::Report),
        (wait.clone(), Outcome::Report),
        (record(agent("a", &[])), Outcome::Plan(plan(Box::new([agent("x", &[])])))),
        (wait, Outcome::Reply),
    ];
    for (record, outcome) in cases {
        assert_eq!(apply_to(&record, None, &facts(), &outcome), invalid(Problem::NotAllowed), "{outcome:?}");
    }
}

// The output's bounds.

/// Names for steps, as many as the test limits allow a plan.
const NAMES: [&str; 8] = ["s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7"];

/// Agent steps named `NAMES[from..to]`, each after the one before it, or
/// each on its own.
fn numbered(from: usize, to: usize, chained: bool) -> Box<[Step]> {
    let mut steps = List::with_capacity(u32::try_from(to.saturating_sub(from)).expect("a test's few steps"));
    for index in from..to {
        let after: &[&str] = match index.checked_sub(1) {
            Some(before) if chained => &NAMES[before..index],
            Some(_) | None => &[],
        };
        steps.push(agent(NAMES[index], after)).expect("room for each step");
    }
    steps.into_boxed()
}

#[test]
fn the_output_has_room_for_the_largest_decision() {
    let steps = usize::try_from(LIMITS.steps).expect("fits");
    let mut out = out();
    assert_eq!(accept(&config(), &env(), &plan(numbered(0, steps, true)), &mut out), Ok(()));
    assert_eq!(out.len(), LIMITS.steps + 1, "an item for each step, and the goal");
    let mut out = super::tests::out();
    let started = Goal {
        steps: Box::new([entry("s0", &[], None)]),
        envelope: Envelope { agents: LIMITS.steps, ..envelope() },
        ..goal()
    };
    let added = Outcome::Steps(numbered(1, steps, true));
    let applied = apply(&config(), &env(), &record(grower("build")), Some(&started), &facts(), &added, &mut out);
    assert_eq!(applied, WRITES);
    assert_eq!(out.len(), LIMITS.steps + 1, "the items, the goal, and the step that grew it");
    let tasks = Outcome::Tasks(numbered(0, usize::try_from(LIMITS.tasks).expect("fits"), false));
    let mut out = super::tests::out();
    assert_eq!(apply(&config(), &env(), &record(session("chat")), None, &facts(), &tasks, &mut out), WRITES);
    assert_eq!(out.len(), LIMITS.tasks);
    assert!(max_out(&LIMITS) >= LIMITS.steps + 2 && max_out(&LIMITS) >= LIMITS.tasks);
}

// Limits.

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bytes = worst_case(&LIMITS).expect("the test limits fit");
    let more = worst_case(&Limits { steps: LIMITS.steps + 1, ..LIMITS }).expect("the test limits fit");
    let word = u64::try_from(size_of::<u32>()).expect("a size fits");
    assert_eq!(more - bytes, 2 * word, "a count and a place in the order for each step checked");
    assert_eq!(worst_case(&Limits { steps: 0, ..LIMITS }), None);
    assert_eq!(max_out(&LIMITS), LIMITS.steps + 2);
}
