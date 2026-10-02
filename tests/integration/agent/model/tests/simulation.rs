//! End to end at the top level: the agent's runs, their conversations as
//! sessions and their tools, with a fake worker, a fake LLM provider and a
//! fake checkout, talking through a simulated world.

use temper_agent_model::run::outcome::Declared;
use temper_agent_model::run::{Answer, Exhausted, Failure, Push};
use temper_agent_model_tests::{Job, Run, Settings, Span, World};
use temper_fake_worker_model::Config;
use temper_fake_worker_model::api::Charter;
use temper_lib::Duration;
use temper_world::assert_replays;

const ITERATIONS: u32 = 200_000;

/// The one run of a world with one job.
fn only(world: &World) -> &Run {
    let runs: Vec<&Run> = world.runs().collect();
    let [run] = runs[..] else { panic!("expected one run, got {}", runs.len()) };
    run
}

fn trace(world: &World) -> String {
    world.trace().join("\n")
}

#[test]
fn a_coding_run_fails_its_checks_then_fixes_the_code_and_lands() {
    let settings = Settings::calm(1).drawing(|charter| charter.tools.write && charter.tools.shell);
    let mut world = World::new(settings);
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
    assert_eq!(world.file(run, b"src/lib.rs").as_deref(), Some(&b"pub fn answer() -> u32 { 43 }\n"[..]));
    // Six turns: a read beside a listing, an edit, a command beside a call to
    // a tool that is not offered, a finish, an edit and a finish; each edit a
    // load and a store.
    let (told, lost) = world.told();
    assert_eq!(lost, 0);
    assert_eq!((told.completions_answered, told.finishes, told.checks_failed, told.accepted), (6, 2, 1, 1));
    let stats = world.stats();
    assert_eq!((stats.ops, stats.reads, stats.probes, stats.checks, stats.pushed), (7, 1, 1, 2, 1));
}

#[test]
fn a_review_finishes_with_a_verdict_after_one_the_run_rejects() {
    let calm = Settings::calm(2);
    let worker = Config { changes: 0, verdicts: 1000, ..calm.worker };
    let settings = Settings { worker, jobs: &[Job::Review], ..calm };
    let mut world = World::new(settings);
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
}

#[test]
fn sub_agents_nest_and_their_answers_come_back_to_their_askers_as_results() {
    let settings = Settings { jobs: &[Job::Delegating], ..Settings::calm(3) }
        .drawing(|charter| charter.agents && charter.tools.write && charter.tools.shell);
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
    assert_eq!(world.file(run, b"src/lib.rs").as_deref(), Some(&b"pub fn answer() -> u32 { 43 }\n"[..]));
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
    let worker = Config { turns_min: 8, turns_max: 8, ..calm.worker };
    let settings = Settings { worker, jobs: &[Job::Spending], ..calm }.drawing(|charter| charter.agents);
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
}

#[test]
fn a_push_that_finds_the_branch_moved_ends_the_run_stale() {
    let calm = Settings::calm(5);
    let settings = Settings { worker: Config { moved: 1000, ..calm.worker }, ..calm }
        .drawing(|charter| charter.tools.write && charter.tools.shell);
    let mut world = World::new(settings);
    world.run(ITERATIONS);

    let run = only(&world);
    assert!(matches!(run.answer, Some(Answer::Failed { failure: Failure::Stale, .. })), "{:?}", run.answer);
    assert_eq!((&run.checked[..], &run.pushes[..]), (&[false, true][..], &[Push::Moved][..]));
    let (told, _) = world.told();
    assert_eq!((told.finishes, told.checks_failed, told.accepted), (2, 1, 0));
}

#[test]
fn a_push_that_fails_is_told_to_the_llm_which_finishes_again() {
    let calm = Settings::calm(9);
    let settings = Settings { worker: Config { push_failures: 1000, ..calm.worker }, ..calm }
        .drawing(|charter| charter.tools.write && charter.tools.shell);
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
}

/// `count` settings from `settings`, each with a seed of its own whose first
/// charter is `wanted`.
fn sweep(settings: &Settings, count: usize, wanted: fn(&Charter) -> bool) -> Vec<Settings> {
    let mut found = Vec::new();
    let mut next = *settings;
    while found.len() < count {
        let settings = next.drawing(wanted);
        next = Settings { seed: settings.seed + 1, ..settings };
        found.push(settings);
    }
    found
}

fn delegates(charter: &Charter) -> bool {
    charter.agents && charter.tools.write && charter.tools.shell
}

#[test]
fn cancels_at_random_moments_close_the_whole_tree() {
    let calm = Settings::calm(6);
    let worker = Config {
        cancels: 1000,
        cancel_min: Duration::ZERO,
        cancel_max: Duration::from_secs(12),
        recancels: 500,
        late_cancels: 500,
        ..calm.worker
    };
    let mut cancelled = 0;
    let mut deep = 0;
    for settings in sweep(&Settings { worker, jobs: &[Job::Delegating], ..calm }, 40, delegates) {
        let mut world = World::new(settings);
        world.run(ITERATIONS);
        let run = only(&world);
        match run.answer {
            Some(Answer::Failed { failure: Failure::Cancelled, .. }) => {
                cancelled += 1;
                deep += u32::from(run.widest > 1);
            }
            Some(Answer::Accepted { .. }) => {}
            _ => panic!("seed {}: expected the run cancelled or done, got {:?}", settings.seed, run.answer),
        }
    }
    assert!(
        cancelled >= 20 && deep >= 10,
        "cancels came at every depth: {cancelled} cancelled, {deep} with sub-agents"
    );
}

#[test]
fn a_run_whose_time_runs_out_mid_tree_ends_once_everything_beneath_it_settled() {
    let calm = Settings::calm(7);
    let worker = Config { time_min: Duration::from_secs(4), time_max: Duration::from_secs(4), ..calm.worker };
    let mut tree = 0;
    for settings in sweep(&Settings { worker, jobs: &[Job::Delegating], ..calm }, 10, delegates) {
        let mut world = World::new(settings);
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
    }
    assert!(tree >= 5, "the deadline passed with sub-agents at work: {tree}");
}

#[test]
fn checks_that_run_past_their_deadline_are_stopped_and_fail() {
    let calm = Settings::calm(8);
    let settings = Settings { check: Span::millis(90_000, 120_000), ..calm }
        .drawing(|charter| charter.tools.write && charter.tools.shell);
    let mut world = World::new(settings);
    world.run(ITERATIONS);

    let run = only(&world);
    assert!(matches!(run.answer, Some(Answer::Failed { failure: Failure::Policy(_), .. })), "{:?}", run.answer);
    let stats = world.stats();
    // Each of the script's three finishes.
    assert_eq!((stats.checks, stats.check_timeouts, stats.pushes), (3, 3, 0));
    let (told, _) = world.told();
    assert_eq!((told.checks_failed, told.checks_finished), (3, 3));
}

/// How the runs of many random worlds ended, by kind.
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
}

impl Ends {
    fn count(&mut self, answer: &Answer) {
        let count = match answer {
            Answer::Accepted { outcome: Declared::Change(_), .. } => &mut self.changes,
            Answer::Accepted { outcome: Declared::Verdict(_), .. } => &mut self.verdicts,
            Answer::Refused(_) => &mut self.refused,
            Answer::Failed { failure, .. } => match failure {
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

#[test]
fn random_worlds_settle_with_every_invariant_held() {
    let mut ends = Ends::default();
    let mut lost = 0;
    for seed in 0..120 {
        let mut world = World::new(Settings::random(seed));
        world.run(ITERATIONS);
        for run in world.runs() {
            ends.count(run.answer.as_ref().expect("every run answers"));
        }
        lost += u32::from(world.told().1 > 0);
    }
    assert_eq!(lost, 0, "the facts kept up in every world");
    let Ends { changes, verdicts, refused, cancelled, stale, budget, policy, model } = ends;
    assert!(
        [changes, verdicts, refused, cancelled, stale, budget, policy, model].iter().all(|count| *count > 0),
        "runs ended every way: {ends:?}"
    );
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
