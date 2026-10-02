//! End to end, where the worker and the agent meet: the engine's runs, hosted
//! by the worker, each carried by an agent process with its conversations as
//! sessions and their tools at work in the checkout the worker prepared, with
//! a fake engine, a fake LLM provider and a fake forge, talking through a
//! simulated world.

use std::collections::BTreeMap;

use temper_agent_model::run::outcome::Declared;
use temper_agent_model::run::{Answer, Charter, Exhausted, Failure, Push};
use temper_agent_model_tests::{Job, Run, Settings, Span, World};
use temper_fake_engine_model::Config;
use temper_lib::Duration;
use temper_world::assert_replays;

const ITERATIONS: u32 = 400_000;

/// The one run of a world with one item.
fn only(world: &World) -> &Run {
    let runs: Vec<&Run> = world.runs().collect();
    let [run] = runs[..] else { panic!("expected one run, got {}", runs.len()) };
    run
}

fn trace(world: &World) -> String {
    world.trace().join("\n")
}

/// Whether `charter` lets its LLM write and run commands.
fn codes(charter: &Charter) -> bool {
    charter.grants.tools.modify && charter.grants.tools.shell
}

/// The code as the checks want it.
const FIXED: &[u8] = b"pub fn answer() -> u32 { 43 }\n";

#[test]
fn a_coding_run_fails_its_checks_then_fixes_the_code_and_lands() {
    let mut world = World::new(Settings::calm(1).drawing(codes));
    world.run(ITERATIONS);

    let run = only(&world);
    assert!(
        matches!(run.answer, Some(Answer::Accepted { outcome: Declared::Change(_), .. })),
        "{:?}\n{}",
        run.answer,
        trace(&world)
    );
    assert_eq!(run.checked, [false, true], "the first finish failed its checks, the second passed");
    assert_eq!(run.pushes, [Push::Done]);
    // The engine recorded the run as ended, and what landed on the forge is
    // the code the LLM fixed.
    assert_eq!(run.reported, Some("ended"));
    assert_eq!(world.landed(run, b"src/lib.rs").as_deref(), Some(FIXED));
    let tally = world.tally();
    assert_eq!((tally.ended, tally.landed), (1, 1));
    // Six turns: a read beside a listing, an edit, a command beside a call to
    // a tool that is not offered, a finish, an edit and a finish; each edit a
    // load and a store.
    let (told, lost) = world.told();
    assert_eq!(lost, 0);
    assert_eq!((told.completions_answered, told.finishes, told.checks_failed, told.accepted), (6, 2, 1, 1));
    let stats = world.stats();
    assert_eq!((stats.ops, stats.reads, stats.probes, stats.checks, stats.pushed), (7, 1, 1, 2, 1));
    assert_eq!((stats.spawns, stats.exits, stats.kills, stats.pushes_landed), (1, 1, 0, 1));
}

#[test]
fn a_review_finishes_with_a_verdict_after_one_the_run_rejects() {
    let calm = Settings::calm(2);
    let engine = Config { changes: 0, verdicts: 1000, ..calm.engine.clone() };
    let mut world = World::new(Settings { engine, ..calm }.doing(Job::Review));
    world.run(ITERATIONS);

    let run = only(&world);
    let Some(Answer::Accepted { outcome: Declared::Verdict(verdict), .. }) = &run.answer else {
        panic!("expected a verdict, got {:?}\n{}", run.answer, trace(&world));
    };
    assert_eq!((&*verdict.name, verdict.children.len()), (&b"request-changes"[..], 1));
    // The broken call never reached the run; the verdict without children
    // did, and was rejected.
    let (told, _) = world.told();
    assert_eq!((told.finishes, told.rejected, told.accepted), (2, 1, 1));
    assert!(run.pushes.is_empty() && run.checked.is_empty(), "a verdict is neither checked nor pushed");
    assert_eq!((run.reported, &run.landed[..]), (Some("ended"), &[][..]), "the engine records the verdict alone");
}

#[test]
fn sub_agents_nest_and_their_answers_come_back_to_their_askers_as_results() {
    let settings = Settings::calm(3).doing(Job::Delegating).drawing(|charter| charter.grants.agents && codes(charter));
    let mut world = World::new(settings);
    world.run(ITERATIONS);

    let run = only(&world);
    assert!(
        matches!(run.answer, Some(Answer::Accepted { outcome: Declared::Change(_), .. })),
        "{:?}\n{}",
        run.answer,
        trace(&world)
    );
    // The fixer, a sub-agent, made the change main finished with.
    assert_eq!((&run.checked[..], &run.pushes[..]), (&[true][..], &[Push::Done][..]));
    assert_eq!(world.landed(run, b"src/lib.rs").as_deref(), Some(FIXED));
    // Two explorers side by side beside main, then the fixer, which asked an
    // explorer of its own, two deep.
    let (told, _) = world.told();
    assert_eq!((told.sub_agents, told.answered, told.deepest, told.opened), (4, 4, 2, 5));
    assert_eq!(told.finishes, 1, "the fixer's finish never reached the run");
    assert_eq!(run.widest, 3);
    assert_eq!(world.stats().sub_answers, 4, "each answer came back to its asker");
}

#[test]
fn a_budget_spent_across_sessions_ends_the_run_once_its_conversations_settle() {
    let calm = Settings::calm(4);
    let engine = Config { turns_min: 8, turns_max: 8, ..calm.engine.clone() };
    let settings = Settings { engine, ..calm }.doing(Job::Spending).drawing(|charter| charter.grants.agents);
    let mut world = World::new(settings);
    world.run(ITERATIONS);

    let run = only(&world);
    let Some(Answer::Failed { failure: Failure::Budget(Exhausted::Turns), spent }) = run.answer else {
        panic!("expected the turns spent, got {:?}\n{}", run.answer, trace(&world));
    };
    // Neither burner spent the run's turns alone: main asked for both, which
    // read side by side until their turns together went past the budget.
    let (live, after) = run.crossed.expect("the run went past its budget");
    assert_eq!((live, run.widest), (3, 3), "main and both burners lived when the run crossed");
    assert!(after <= live + 1, "past its budget, each conversation finishes what it had started");
    assert_eq!(spent.turns, 9 + after);
    let (told, _) = world.told();
    assert_eq!((told.sub_agents, told.opened), (2, 3));
    assert_eq!(run.reported, Some("run budget"));
}

#[test]
fn a_push_that_finds_the_branch_moved_ends_the_run_stale() {
    let settings = Settings { moved: 1000, ..Settings::calm(5) }.drawing(codes);
    let mut world = World::new(settings);
    world.run(ITERATIONS);

    let run = only(&world);
    assert!(matches!(run.answer, Some(Answer::Failed { failure: Failure::Stale, .. })), "{:?}", run.answer);
    assert_eq!((&run.checked[..], &run.pushes[..]), (&[false, true][..], &[Push::Moved][..]));
    let (told, _) = world.told();
    assert_eq!((told.finishes, told.checks_failed, told.accepted), (2, 1, 0));
    // Another party moved the branch on the forge, and nothing of the run's
    // landed there.
    assert_eq!((run.reported, &run.landed[..]), (Some("run stale"), &[][..]));
    assert_eq!((world.stats().advanced, world.stats().pushes_landed), (1, 0));
}

#[test]
fn a_push_the_forge_refuses_is_told_to_the_llm_which_finishes_again() {
    let settings = Settings { refusing: 1000, ..Settings::calm(9) }.drawing(codes);
    let mut world = World::new(settings);
    world.run(ITERATIONS);

    // The script finishes once more after the push fails, then stops; the
    // run nudges it, and fails it once it stops again.
    let run = only(&world);
    assert!(matches!(run.answer, Some(Answer::Failed { failure: Failure::Policy(_), .. })), "{:?}", run.answer);
    assert_eq!(run.checked, [false, true, true], "each finish is checked afresh");
    assert_eq!(run.pushes, [Push::Failed, Push::Failed]);
    let (told, _) = world.told();
    assert_eq!((told.finishes, told.unpushed, told.yielded), (3, 2, 2));
    assert_eq!(run.reported, Some("run policy"));
}

/// `count` settings from `settings`, each with a seed of its own whose first
/// charter is `wanted`.
fn sweep(settings: &Settings, count: usize, wanted: fn(&Charter) -> bool) -> Vec<Settings> {
    let mut found = Vec::new();
    let mut next = settings.clone();
    while found.len() < count {
        let settings = next.drawing(wanted);
        next = Settings { seed: settings.seed + 1, ..settings.clone() };
        found.push(settings);
    }
    found
}

fn delegates(charter: &Charter) -> bool {
    charter.grants.agents && codes(charter)
}

#[test]
fn cancels_from_the_engine_at_random_moments_close_the_whole_tree() {
    let calm = Settings::calm(6).doing(Job::Delegating);
    let engine = Config {
        cancels: 1000,
        cancel_min: Duration::ZERO,
        cancel_max: Duration::from_secs(12),
        late_cancels: 500,
        ..calm.engine.clone()
    };
    let (mut cancelled, mut deep, mut early) = (0, 0, 0);
    for settings in sweep(&Settings { engine, ..calm }, 40, delegates) {
        let mut world = World::new(settings.clone());
        world.run(ITERATIONS);
        let runs: Vec<&Run> = world.runs().collect();
        let [run] = runs[..] else {
            // The cancel came while the worker prepared the run's checkout.
            assert!(runs.is_empty(), "seed {}: one item, one run at most", settings.seed);
            assert_eq!(world.tally().endings.cancelled, 1, "seed {}: the item was cancelled", settings.seed);
            early += 1;
            continue;
        };
        match run.answer {
            Some(Answer::Failed { failure: Failure::Cancelled, .. }) => {
                assert_eq!(run.reported, Some("cancelled engine"), "seed {}", settings.seed);
                cancelled += 1;
                deep += u32::from(run.widest > 1);
            }
            Some(Answer::Accepted { .. }) => assert_eq!(run.reported, Some("ended"), "seed {}", settings.seed),
            _ => panic!("seed {}: expected the run cancelled or done, got {:?}", settings.seed, run.answer),
        }
    }
    assert!(
        cancelled >= 20 && deep >= 10,
        "cancels came at every depth: {cancelled} cancelled, {deep} with sub-agents, {early} before their runs"
    );
}

#[test]
fn a_run_whose_time_runs_out_mid_tree_ends_once_everything_beneath_it_settled() {
    let calm = Settings::calm(7).doing(Job::Delegating);
    let engine = Config { time_min: Duration::from_secs(4), time_max: Duration::from_secs(4), ..calm.engine.clone() };
    let mut tree = 0;
    for settings in sweep(&Settings { engine, ..calm }, 10, delegates) {
        let mut world = World::new(settings.clone());
        world.run(ITERATIONS);
        let run = only(&world);
        assert!(
            matches!(run.answer, Some(Answer::Failed { failure: Failure::Budget(Exhausted::Time), .. })),
            "seed {}: {:?}\n{}",
            settings.seed,
            run.answer,
            trace(&world)
        );
        tree += u32::from(run.widest > 1);
        // Closing cascades down the tree a level an iteration, and what was
        // in flight is cancelled at once.
        let took = run.answered.expect("answered").saturating_since(run.started.expect("started"));
        assert!(took <= Duration::from_secs(5), "seed {}: answered {took:?} after it started", settings.seed);
        assert_eq!(run.reported, Some("run budget"));
    }
    assert!(tree >= 5, "the deadline passed with sub-agents at work: {tree}");
}

#[test]
fn checks_that_run_past_their_deadline_are_stopped_and_fail() {
    let settings = Settings { check: Span::millis(90_000, 120_000), ..Settings::calm(8) }.drawing(codes);
    let mut world = World::new(settings);
    world.run(ITERATIONS);

    let run = only(&world);
    assert!(matches!(run.answer, Some(Answer::Failed { failure: Failure::Policy(_), .. })), "{:?}", run.answer);
    let stats = world.stats();
    // Each of the script's three finishes; the worker's watchdog waits for
    // checks the run says are running.
    assert_eq!((stats.checks, stats.check_timeouts, stats.pushes), (3, 3, 0));
    let (told, _) = world.told();
    assert_eq!((told.checks_failed, told.checks_finished), (3, 3));
    assert_eq!(run.reported, Some("run policy"));
}

/// How the runs of many random worlds ended, by kind, as their agents
/// answered; and those whose agents were killed before they did.
#[derive(Default, Debug)]
struct Ends {
    changes: u32,
    verdicts: u32,
    refused: u32,
    cancelled: u32,
    stale: u32,
    budget: u32,
    policy: u32,
    model: u32,
    killed: u32,
}

impl Ends {
    fn count(&mut self, run: &Run) {
        let count = match &run.answer {
            None => &mut self.killed,
            Some(Answer::Accepted { outcome: Declared::Change(_), .. }) => &mut self.changes,
            Some(Answer::Accepted { outcome: Declared::Verdict(_), .. }) => &mut self.verdicts,
            Some(Answer::Refused(_)) => &mut self.refused,
            Some(Answer::Failed { failure, .. }) => match failure {
                Failure::Cancelled => &mut self.cancelled,
                Failure::Stale => &mut self.stale,
                Failure::Budget(_) => &mut self.budget,
                Failure::Policy(_) => &mut self.policy,
                Failure::Model(_) => &mut self.model,
            },
        };
        *count += 1;
    }
}

/// How the worker answered the engine for the runs of random worlds, by
/// kind, as the agents' runs ended or their agents failed.
const REPORTED: [&str; 8] = [
    "ended",
    "run model",
    "run budget",
    "run policy",
    "run stale",
    "agent no progress",
    "agent wall time",
    "cancelled engine",
];

#[test]
fn random_worlds_settle_with_every_invariant_held() {
    let mut ends = Ends::default();
    let mut reported = BTreeMap::new();
    let (mut lost, mut unprepared, mut invalid, mut landed, mut saves) = (0, 0, 0, 0, 0);
    let mut checks = 0;
    for seed in 0..120 {
        let mut world = World::new(Settings::random(seed));
        world.run(ITERATIONS);
        // The referee passed the world, having seen every assignment
        // answered in time.
        let (checked, met) = world.judged();
        assert_eq!(met, u64::from(world.stats().assigned), "the referee saw every assignment answered");
        checks += checked;
        for run in world.runs() {
            ends.count(run);
            *reported.entry(run.reported.expect("every attempt is answered")).or_insert(0) += 1;
        }
        lost += u32::from(world.told().1 > 0);
        let stats = world.stats();
        (unprepared, landed, saves) = (unprepared + stats.unprepared, landed + stats.landed, saves + stats.saves);
        invalid += world.tally().invalid;
    }
    assert_eq!(lost, 0, "the facts kept up in every world");
    let Ends { changes, verdicts, refused, cancelled, stale, budget, policy, model, killed } = ends;
    assert!(
        [changes, verdicts, refused, cancelled, stale, budget, policy, model, killed].iter().all(|count| *count > 0),
        "runs ended every way: {ends:?}"
    );
    // A run cancelled by the engine reports the cancel as the worker's, and
    // one the worker's wall time cancelled as its agent's fault: no run
    // reports a cancel of its own.
    assert!(!reported.contains_key("run cancelled"), "{reported:?}");
    assert!(REPORTED.iter().all(|kind| reported.contains_key(kind)), "the worker answered every way: {reported:?}");
    assert!(unprepared > 0 && invalid > 0, "some runs never reached an agent: {unprepared}, {invalid}");
    assert!(landed > 0 && saves > 0, "changes landed, and unfinished work was saved: {landed}, {saves}");
    assert!(checks > 0, "the referee checked what the worker and the agents did");
}

#[test]
fn a_world_replays_from_its_seed() {
    let run = |seed: u64| {
        let mut world = World::new(Settings::random(seed));
        world.run(ITERATIONS);
        (world.trace().to_vec(), (world.stats(), world.told(), world.now()))
    };
    let trace = assert_replays(7, 8, run);
    assert!(trace.len() > 100, "the world did something");
}
