//! End to end at the top level: the agent's runs, their conversations as
//! sessions and their tools, with a fake worker, a fake LLM provider and a
//! fake checkout, talking through a simulated world.

use temper_agent_model::run::outcome::Declared;
use temper_agent_model::run::{Answer, Push};
use temper_agent_model_tests::{Run, Settings, World};

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
