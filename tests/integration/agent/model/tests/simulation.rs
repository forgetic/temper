//! End to end, where the engine, the worker and the agent meet: issues
//! people hand in, whose steps the engine runs on the worker, each run
//! carried by an agent process with its conversations as sessions and their
//! tools at work in the checkout the worker prepared, with a fake LLM
//! provider and a fake forge, talking through a simulated world.

use std::collections::BTreeMap;

use temper_agent_model::run::outcome::Declared;
use temper_agent_model::run::{Answer, Exhausted, Failure, Push};
use temper_agent_model_tests::desk::{CODING, Hand, Reviewer, Work};
use temper_agent_model_tests::{CALM, Job, Run, Settings, Span, Stats, World};
use temper_engine_model::Outcome;
use temper_engine_model::plan::{Budget, Verdict};
use temper_engine_model::work::{Hold, Phase};
use temper_engine_model_tests::deployment;
use temper_lib::Duration;
use temper_world::assert_replays;

const ITERATIONS: u32 = 400_000;

/// The runs of a world, in the order their agents were spawned.
fn runs(world: &World) -> Vec<&Run> {
    world.runs().collect()
}

/// The first run of a world.
fn first(world: &World) -> &Run {
    world.runs().next().unwrap_or_else(|| panic!("a run started\n{}", trace(world)))
}

fn trace(world: &World) -> String {
    world.trace().join("\n")
}

/// The code as the checks want it.
const FIXED: &[u8] = b"pub fn answer() -> u32 { 43 }\n";

/// A change that a person reviews.
const CHANGE: Work = Work::Change { checks: true, reviewer: Reviewer::Person };

#[test]
fn a_coding_run_fails_its_checks_then_fixes_the_code_and_lands() {
    let mut world = World::new(Settings::calm(1));
    world.run(ITERATIONS);

    let runs = runs(&world);
    let [run] = runs[..] else { panic!("one run, got {}\n{}", runs.len(), trace(&world)) };
    assert!(
        matches!(run.answer, Some(Answer::Accepted { outcome: Declared::Change(_), .. })),
        "{:?}\n{}",
        run.answer,
        trace(&world)
    );
    assert_eq!(run.checked, [false, true], "the first finish failed its checks, the second passed");
    assert_eq!(run.pushes, [Push::Done]);
    // The worker answered the engine with the change, which landed on the
    // run's branch as the LLM fixed it.
    assert_eq!(run.reported, Some("ended"));
    assert_eq!(world.landed(run, b"src/lib.rs").as_deref(), Some(FIXED));
    // The engine opened its pull request, a person approved it once CI
    // passed, and the engine merged it: what landed on the default branch is
    // the code the LLM fixed.
    let item = world.items()[0];
    assert_eq!(world.merged(item, b"src/lib.rs").as_deref(), Some(FIXED));
    let stats = world.stats();
    assert_eq!((stats.handed, stats.assigned, stats.ended, stats.landed), (1, 1, 1, 1));
    assert_eq!((stats.merged, stats.reviews), (1, 1));
    // Six turns: a read beside a listing, an edit, a command beside a call to
    // a tool that is not offered, a finish, an edit and a finish; each edit a
    // load and a store.
    let (told, lost) = world.told();
    assert_eq!(lost, 0);
    assert_eq!((told.completions_answered, told.finishes, told.checks_failed, told.accepted), (6, 2, 1, 1));
    assert_eq!((stats.ops, stats.reads, stats.probes, stats.checks, stats.pushed), (7, 1, 1, 2, 1));
    assert_eq!((stats.spawns, stats.exits, stats.kills, stats.pushes_landed), (1, 1, 0, 1));
}

#[test]
fn a_review_by_an_agent_asks_for_changes_after_a_verdict_the_run_rejects() {
    let work = Work::Change { checks: true, reviewer: Reviewer::Agent };
    let mut world = World::new(Settings::calm(2).doing(Job::Coding, work));
    world.run(ITERATIONS);

    // The change landed, and its pull request's head was reviewed by a run
    // of the review's charter.
    let review = world.runs().find(|run| run.job == Job::Review);
    let review = review.unwrap_or_else(|| panic!("a review ran\n{}", trace(&world)));
    let Some(Answer::Accepted { outcome: Declared::Verdict(verdict), .. }) = &review.answer else {
        panic!("expected a verdict, got {:?}\n{}", review.answer, trace(&world));
    };
    assert_eq!((&*verdict.name, verdict.children.len()), (&b"request-changes"[..], 1));
    assert!(review.pushes.is_empty() && review.checked.is_empty(), "a verdict is neither checked nor pushed");
    assert_eq!((review.reported, &review.landed[..]), (Some("ended"), &[][..]), "a verdict lands nothing");
    // The engine posted it on the item as the changes asked for.
    let item = world.items()[0];
    let outcomes = world.mirror().outcomes(deployment::name(item.repository), item.number);
    let asked = outcomes.iter().any(|posted| match &posted.outcome {
        Outcome::Verdict { verdict, .. } => *verdict == Verdict::Changes,
        Outcome::Change { .. }
        | Outcome::Report { .. }
        | Outcome::Plan { .. }
        | Outcome::Steps { .. }
        | Outcome::Tasks { .. }
        | Outcome::Reply { .. }
        | Outcome::Finished { .. }
        | Outcome::Release { .. }
        | Outcome::Escalation { .. } => false,
    });
    assert!(asked, "the engine posted the verdict: {outcomes:?}");
}

#[test]
fn an_agent_step_reports_and_its_item_is_done() {
    let mut world = World::new(Settings::calm(10).doing(Job::Reporting, Work::Agent));
    world.run(ITERATIONS);

    let runs = runs(&world);
    let [run] = runs[..] else { panic!("one run, got {}\n{}", runs.len(), trace(&world)) };
    let Some(Answer::Accepted { outcome: Declared::Verdict(verdict), .. }) = &run.answer else {
        panic!("expected a report, got {:?}\n{}", run.answer, trace(&world));
    };
    assert_eq!(&*verdict.name, b"report");
    assert_eq!(run.reported, Some("ended"));
    // The engine posted the report and closed the item.
    let item = world.items()[0];
    let repository = deployment::name(item.repository);
    let outcomes = world.mirror().outcomes(repository, item.number);
    assert!(matches!(outcomes[..], [ref posted] if matches!(posted.outcome, Outcome::Report { .. })), "{outcomes:?}");
    assert!(world.mirror().issue(repository, item.number).is_some_and(|issue| !issue.open), "the item is done");
}

#[test]
fn sub_agents_nest_and_their_answers_come_back_to_their_askers_as_results() {
    let mut world = World::new(Settings::calm(3).doing(Job::Delegating, CHANGE));
    world.run(ITERATIONS);

    let runs = runs(&world);
    let [run] = runs[..] else { panic!("one run, got {}\n{}", runs.len(), trace(&world)) };
    assert!(
        matches!(run.answer, Some(Answer::Accepted { outcome: Declared::Change(_), .. })),
        "{:?}\n{}",
        run.answer,
        trace(&world)
    );
    // The fixer, a sub-agent, made the change main finished with, and it
    // landed on the default branch.
    assert_eq!((&run.checked[..], &run.pushes[..]), (&[true][..], &[Push::Done][..]));
    assert_eq!(world.landed(run, b"src/lib.rs").as_deref(), Some(FIXED));
    assert_eq!(world.merged(world.items()[0], b"src/lib.rs").as_deref(), Some(FIXED));
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
    let budget = Budget { turns: 8, ..CALM };
    let mut world = World::new(Settings::calm(4).doing(Job::Spending, Work::Agent).spending(budget));
    world.run(ITERATIONS);

    let run = first(&world);
    let Some(Answer::Failed { failure: Failure::Budget(Exhausted::Turns), spent }) = run.answer else {
        panic!("expected the turns spent, got {:?}\n{}", run.answer, trace(&world));
    };
    // Neither burner spent the run's turns alone: main asked for both, which
    // read side by side until their turns together went past the budget.
    let (live, after) = run.crossed.expect("the run went past its budget");
    assert_eq!((live, run.widest), (3, 3), "main and both burners lived when the run crossed");
    assert!(after <= live + 1, "past its budget, each conversation finishes what it had started");
    assert_eq!(spent.turns, 9 + after);
    assert_eq!(run.reported, Some("run budget"));
    // The engine retried the step as its run's failures allow, each run
    // spending its budget alike, then held the item for a person.
    let runs = runs(&world);
    assert!(runs.len() > 1 && runs.iter().all(|run| run.reported == Some("run budget")), "{}", trace(&world));
    let (told, _) = world.told();
    let count = u32::try_from(runs.len()).expect("a few runs");
    assert_eq!((told.sub_agents, told.opened), (2 * count, 3 * count));
}

#[test]
fn a_push_that_finds_the_branch_moved_ends_the_run_stale() {
    let mut world = World::new(Settings { moved: 1000, ..Settings::calm(5) });
    world.run(ITERATIONS);

    let run = first(&world);
    assert!(matches!(run.answer, Some(Answer::Failed { failure: Failure::Stale, .. })), "{:?}", run.answer);
    assert_eq!((&run.checked[..], &run.pushes[..]), (&[false, true][..], &[Push::Moved][..]));
    // Another party moved the branch on the forge, and nothing of the run's
    // landed there.
    assert_eq!((run.reported, &run.landed[..]), (Some("run stale"), &[][..]));
    assert!(world.stats().advanced >= 1);
}

#[test]
fn a_push_the_forge_refuses_is_told_to_the_llm_which_finishes_again() {
    let mut world = World::new(Settings { refusing: 1000, ..Settings::calm(9) });
    world.run(ITERATIONS);

    // The script finishes once more after the push fails, then stops; the
    // run nudges it, and fails it once it stops again.
    let run = first(&world);
    assert!(matches!(run.answer, Some(Answer::Failed { failure: Failure::Policy(_), .. })), "{:?}", run.answer);
    assert_eq!(run.checked, [false, true, true], "each finish is checked afresh");
    assert_eq!(run.pushes, [Push::Failed, Push::Failed]);
    assert_eq!(run.reported, Some("run policy"));
    assert_eq!(world.stats().pushes_landed, 0, "nothing lands on a forge that refuses it");
}

#[test]
fn people_stopping_runs_at_random_moments_close_the_whole_tree() {
    let (mut cancelled, mut deep, mut early, mut done) = (0, 0, 0, 0);
    for seed in 0..40 {
        let calm = Settings::calm(600 + seed).doing(Job::Delegating, CHANGE);
        let settings = Settings { stops: 1000, stop_after: Span::millis(0, 12_000), ..calm };
        let mut world = World::new(settings);
        world.run(ITERATIONS);
        let stats = world.stats();
        assert_eq!(stats.stops, 1, "seed {seed}: a person stopped the item's first run");
        // The engine stopped it, and holds the item for the person; unless
        // its run had finished and the item closed, and the stop was refused.
        let item = world.items()[0];
        let repository = deployment::name(item.repository);
        let closed = world.mirror().issue(repository, item.number).is_some_and(|issue| !issue.open);
        let phase = world.mirror().record(repository, item.number).map(|record| record.lifecycle.phase);
        let held = phase == Some(Phase::Held { why: Hold::Stopped, outcome: None });
        assert_eq!(
            (stats.stopped, held),
            (u32::from(!closed), !closed),
            "seed {seed}: the engine stopped the item and holds it, or it had closed: {phase:?}\n{}",
            trace(&world)
        );
        done += u32::from(closed);
        let Some(run) = world.runs().next() else {
            // The stop came while the worker prepared the run's checkout.
            early += 1;
            continue;
        };
        match run.answer {
            Some(Answer::Failed { failure: Failure::Cancelled, .. }) => {
                assert_eq!(run.reported, Some("cancelled engine"), "seed {seed}");
                cancelled += 1;
                deep += u32::from(run.widest > 1);
            }
            Some(Answer::Accepted { .. }) => {
                assert_eq!(run.reported, Some("ended"), "seed {seed}");
                assert!(closed, "seed {seed}: a run done before its stop closed its item");
            }
            _ => panic!("seed {seed}: expected the run cancelled or done, got {:?}", run.answer),
        }
    }
    assert!(
        cancelled >= 20 && deep >= 10 && done > 0,
        "stops came at every depth: {cancelled} cancelled, {deep} with sub-agents, {early} before their runs, \
         {done} after"
    );
}

#[test]
fn a_run_whose_time_runs_out_mid_tree_ends_once_everything_beneath_it_settled() {
    let budget = Budget { time: Duration::from_secs(4), ..CALM };
    let mut tree = 0;
    for seed in 0..10 {
        let settings = Settings::calm(700 + seed).doing(Job::Delegating, CHANGE).spending(budget);
        let mut world = World::new(settings);
        world.run(ITERATIONS);
        let run = first(&world);
        assert!(
            matches!(run.answer, Some(Answer::Failed { failure: Failure::Budget(Exhausted::Time), .. })),
            "seed {seed}: {:?}\n{}",
            run.answer,
            trace(&world)
        );
        tree += u32::from(run.widest > 1);
        // Closing cascades down the tree a level an iteration, and what was
        // in flight is cancelled at once.
        let took = run.answered.expect("answered").saturating_since(run.started.expect("started"));
        assert!(took <= Duration::from_secs(5), "seed {seed}: answered {took:?} after it started");
        assert_eq!(run.reported, Some("run budget"));
    }
    assert!(tree >= 5, "the deadline passed with sub-agents at work: {tree}");
}

#[test]
fn checks_that_run_past_their_deadline_are_stopped_and_fail() {
    let mut world = World::new(Settings { check: Span::millis(90_000, 120_000), ..Settings::calm(8) });
    world.run(ITERATIONS);

    let run = first(&world);
    assert!(matches!(run.answer, Some(Answer::Failed { failure: Failure::Policy(_), .. })), "{:?}", run.answer);
    // Each of the script's three finishes, in every run the engine retried;
    // the worker's watchdog waits for checks the run says are running.
    let runs = runs(&world);
    assert!(runs.iter().all(|run| run.reported == Some("run policy")), "{}", trace(&world));
    let count = u32::try_from(runs.len()).expect("a few runs");
    let stats = world.stats();
    assert_eq!((stats.checks, stats.check_timeouts, stats.pushes), (3 * count, 3 * count, 0));
    let (told, _) = world.told();
    assert_eq!((told.checks_failed, told.checks_finished), (3 * count, 3 * count));
}

/// Two changes to the same code handed in at once, one that lands an answer
/// of 41 (its checks are not asked for) and one that lands 43: both pass CI
/// and are approved, and the engine merges them one after the other, before
/// it has read that the first moved the base under the second. The forge
/// refuses the second merge for a conflict, and the engine holds its item for
/// a write that failed for good, where a conflict is the change's to repair
/// (engine-model.md, 5.1; the plan's `Repair::Conflicts`), as it does when it
/// reads the conflict before it merges.
#[test]
#[ignore = "engine: a merge refused for a conflict holds the item for its writes instead of repairing the change"]
fn a_merge_refused_for_a_conflict_sends_the_change_back_for_repair() {
    let hand = |checks| Hand {
        at: Duration::ZERO,
        repository: 0,
        job: Job::Coding,
        work: Work::Change { checks, reviewer: Reviewer::Person },
        grants: CODING,
        budget: CALM,
    };
    let mut world = World::new(Settings { hands: vec![hand(false), hand(true)], ..Settings::calm(2) });
    world.run(ITERATIONS);

    assert_eq!(world.stats().merged, 1, "the second merge was refused\n{}", trace(&world));
    for item in world.items() {
        let record = world.mirror().record(deployment::name(item.repository), item.number);
        let phase = record.map(|record| record.lifecycle.phase);
        assert_ne!(phase, Some(Phase::Held { why: Hold::Writes, outcome: None }), "{item:?}");
    }
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
    sweep(true);
}

/// The sweep, in worlds where nothing is held for its writes where the
/// forge never fails, a merge refused for a conflict included: the change
/// is its run's to repair (engine-model.md, 5.1).
#[test]
#[ignore = "until merges refused for a conflict go to repair"]
fn random_worlds_hold_items_for_their_writes_only_where_the_forge_fails() {
    sweep(false);
}

/// Settles many random worlds, an item held for its writes once a merge was
/// refused for a conflict if `conflicts_held`, and checks how their runs and
/// their issues ended.
fn sweep(conflicts_held: bool) {
    let mut ends = Ends::default();
    let mut reported = BTreeMap::new();
    let (mut lost, mut unprepared, mut invalid, mut landed, mut saves) = (0, 0, 0, 0, 0);
    let (mut merged, mut stopped, mut checks) = (0, 0, 0);
    let mut items = Items::default();
    for seed in 0..120 {
        let settings = Settings { conflicts_held, ..Settings::random(seed) };
        let faults = settings.faults();
        let mut world = World::new(settings);
        world.run(ITERATIONS);
        // The referees passed the world, having seen every assignment
        // answered in time, the outcome of every run that ended posted, and
        // every issue handed in end.
        let (checked, met) = world.judged();
        let stats = world.stats();
        assert_eq!(
            met,
            u64::from(stats.assigned + stats.ended + stats.handed),
            "seed {seed}: the referee saw every assignment answered, every outcome posted and every issue end"
        );
        items.count(&stats, faults);
        if !faults {
            let writes = if conflicts_held && stats.conflicts > 0 { 0 } else { stats.held_writes };
            assert_eq!(
                (writes, stats.held_record),
                (0, 0),
                "seed {seed}: nothing is held for its writes or its record where the forge and the store never fail"
            );
        }
        checks += checked;
        for run in world.runs() {
            ends.count(run);
            *reported.entry(run.reported.expect("every attempt is answered")).or_insert(0) += 1;
        }
        lost += u32::from(world.told().1 > 0);
        let stats = world.stats();
        (unprepared, landed, saves) = (unprepared + stats.unprepared, landed + stats.landed, saves + stats.saves);
        (invalid, merged, stopped) = (invalid + stats.invalid, merged + stats.merged, stopped + stats.stopped);
    }
    assert_eq!(lost, 0, "the facts kept up in every world");
    let Ends { changes, verdicts, refused, cancelled, stale, budget, policy, model, killed } = ends;
    assert!(
        [changes, verdicts, refused, cancelled, stale, budget, policy, model, killed].iter().all(|count| *count > 0),
        "runs ended every way: {ends:?}"
    );
    // A run stopped by a person reports the cancel as the worker's, and one
    // the worker's wall time cancelled as its agent's fault: no run reports
    // a cancel of its own.
    assert!(!reported.contains_key("run cancelled"), "{reported:?}");
    assert!(REPORTED.iter().all(|kind| reported.contains_key(kind)), "the worker answered every way: {reported:?}");
    assert!(unprepared > 0 && invalid > 0, "some runs never reached an agent: {unprepared}, {invalid}");
    assert!(landed > 0 && saves > 0, "changes landed, and unfinished work was saved: {landed}, {saves}");
    assert!(merged > 0 && stopped > 0, "changes were merged, and runs stopped: {merged}, {stopped}");
    assert!(checks > 0, "the referees checked what the engine, the worker and the agents did");
    let Items { closed, failures, stopped, sound, .. } = items;
    assert!(
        closed > 0 && failures > 0 && stopped > 0 && sound > 0,
        "issues were closed, and held for their runs' failures and for people's stops, in worlds whose forge and \
         store never fail too: {items:?}"
    );
}

/// How the issues handed in to many random worlds ended: closed, or held
/// for a person, by why; and how many worlds had a forge and a store that
/// never fail.
#[derive(Default, Debug)]
struct Items {
    closed: u32,
    plan: u32,
    failures: u32,
    stopped: u32,
    acceptance: u32,
    writes: u32,
    record: u32,
    sound: u32,
}

impl Items {
    fn count(&mut self, stats: &Stats, faults: bool) {
        self.closed += stats.closed;
        self.plan += stats.held_plan;
        self.failures += stats.held_failures;
        self.stopped += stats.held_stopped;
        self.acceptance += stats.held_acceptance;
        self.writes += stats.held_writes;
        self.record += stats.held_record;
        self.sound += u32::from(!faults);
    }
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
