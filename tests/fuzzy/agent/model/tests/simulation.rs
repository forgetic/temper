//! The agent's top-level world at random: many random worlds, the engine,
//! the worker and the agents together, each settled with every invariant
//! held, and every way a run and an issue can end reached among them.

use std::collections::BTreeMap;

use temper_agent_model::run::outcome::Declared;
use temper_agent_model::run::{Answer, Failure};
use temper_agent_model_tests::{Run, Settings, Stats, World};

const ITERATIONS: u32 = 400_000;

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
    let (mut editors, mut ci_repairs) = (0, 0);
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
        ci_repairs += stats.ci_repairs;
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
            let verdict = matches!(run.answer, Some(Answer::Accepted { outcome: Declared::Verdict(_), .. }));
            editors += u32::from(verdict && run.allowed.writable);
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
    assert!(editors > 0, "some runs finished with a verdict in a checkout they could write");
    assert!(ci_repairs > 0, "some changes went back for repair once their CI failed");
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
