//! Give the plan data, inspect its decisions.

use alloc::boxed::Box;
use core::mem::size_of;

use temper_lib::{Duration, Env, List, Queue, Time};

use crate::{
    AgentSpec, Batch, Budget, ChangeSpec, Charter, Config, Envelope, Gate, Goal, Grants, Growing, Growth, Key, Limits,
    Plan, Problem, Problems, Progress, Record, Repo, Repository, Resume, Review, SessionSpec, Sources, Step, Target,
    Template, WaitSpec, Wake, Work, Write, accept, check, grow, max_out, worst_case,
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

/// The goal of the plan of `steps`, as accepted: an agent step and a change.
fn goal() -> Goal {
    Goal { steps: names(&["a", "b"]), envelope: envelope(), budget: 10_000, estimate: 200, growth: Growth::NONE }
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
    assert_eq!(grow(&config(), &env(), &goal(), &steps, &mut out), Ok(Growing::Within));
    let grown = Goal {
        steps: names(&["a", "b", "c", "d", "e"]),
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
    assert_eq!(grow(&config(), &env(), &goal(), &main, &mut out), Ok(Growing::Beyond));
    assert_eq!(writes(&mut out).len(), 2);
    // A session, of which the envelope allows none.
    assert_eq!(grow(&config(), &env(), &goal(), &[session("c")], &mut out), Ok(Growing::Beyond));
    assert_eq!(writes(&mut out).len(), 2);
    // A third agent step, counting the growth so far.
    let grown = Goal { growth: Growth { agents: 2, ..Growth::NONE }, ..goal() };
    assert_eq!(grow(&config(), &env(), &grown, &[agent("c", &[])], &mut out), Ok(Growing::Beyond));
    assert_eq!(writes(&mut out).len(), 2);
}

#[test]
fn growth_is_checked_against_the_plan_it_joins() {
    let mut out = out();
    let refused = grow(&config(), &env(), &goal(), &[agent("a", &[]), agent("c", &["d", "c"])], &mut out);
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
        grow(&config(), &env(), &goal(), &[], &mut out),
        Err(Problems { listed: Box::new([Problem::NoSteps]), more: 0 })
    );
    let spent = Goal { estimate: 9_950, ..goal() };
    assert_eq!(
        grow(&config(), &env(), &spent, &[agent("c", &[])], &mut out),
        Err(Problems { listed: Box::new([Problem::OverBudget { estimate: 10_050, budget: 10_000 }]), more: 0 })
    );
    let full = nameless(LIMITS.steps - 1, &[]);
    assert_eq!(
        grow(&config(), &env(), &goal(), &full, &mut out),
        Err(Problems { listed: Box::new([Problem::TooManySteps { max: LIMITS.steps }]), more: 0 })
    );
    assert!(out.is_empty());
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
