//! Adversarial dependency waits through temper's real engine root.
use jig_domain_tasks as tasks;
use skein_lib::Token;
use temper_engine_domain as engine;
use temper_engine_domain_world::{direct::Driver, fixtures, protocol, walking};

const CHANNEL: Token = Token::new(7);

const TASKS: u32 = 6;

fn limits() -> engine::Limits {
    let mut bounds = walking::limits();
    bounds.tasks.tasks = TASKS;
    bounds.tasks.project_tasks = TASKS;
    bounds.tasks.tree_tasks = TASKS;
    bounds.tasks.depth = 2;
    bounds.tasks.delegates = 4;
    bounds.tasks.references = TASKS;
    bounds.tasks.batch = 3;
    bounds.tasks.dependencies = 2;
    bounds.tasks.inbox_messages = 8;
    bounds.tasks.inbox_bytes = 512;
    bounds.people.inbox_tasks = TASKS * 2;
    bounds.people.inbox_recipients = TASKS * 4 + 1;
    bounds.authority.batch = 3;
    bounds.fleet.attempts = TASKS + 1;
    bounds.fleet.slots = TASKS - 1;
    bounds.call_records = 8;
    bounds.brief.sections = 5;
    bounds.brief.budgets.dependencies = 512;
    bounds.brief.read_bytes = 512;
    bounds.brief.brief_bytes = 1024;
    bounds.journal.result_bytes = 1024;
    bounds.journal.writes = walking::writes(&bounds).max(8192);
    bounds.journal.held = 300;
    bounds.journal.deliveries = 100;
    assert!(engine::worst_case(&bounds).is_some(), "QA bounds must admit the engine root");
    bounds
}

fn configuration() -> engine::Config {
    let mut config = walking::config(92);
    config.core.defaults.charters[0].policy.model.price_unit = 2000;
    config.core.settings.models[0] = config.core.defaults.charters[0].policy.model.clone();
    let mut rules = config.core.defaults.rules.clone();
    rules.maximum_run_spend = 5;
    rules.ceiling.delegation.tasks = TASKS;
    rules.ceiling.delegation.depth = 3;
    let mut policy = config.core.seed_projects[0].policy.policy.clone();
    policy.ceiling.delegation.tasks = TASKS;
    policy.ceiling.delegation.depth = 3;
    policy.roles[0].authority.delegation.tasks = TASKS;
    policy.roles[0].authority.delegation.depth = 3;
    policy.roles[0].authority.budget.spend = 150;
    policy.roles[0].period_spend = 500;
    let mut authority = jig_domain_authority::Domain::new(rules, limits().authority).expect("fixture authority");
    let mut out = skein_lib::Queue::with_capacity(jig_domain_authority::POLICY_MAX_OUT);
    jig_domain_authority::step(&mut authority, jig_domain_authority::Event::Policy { project: 1, policy }, &mut out);
    config.core.seed_projects[0].policy.policy = authority.policy(1).expect("configured seed policy").clone();
    config.core.defaults.rules = authority.rules().clone();
    config
}

struct Story {
    driver: Driver,
    parent: engine::Assignment,
    target: engine::Assignment,
    dependencies: Vec<engine::Assignment>,
}

fn assignment(driver: &Driver, task: u64) -> engine::Assignment {
    driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let engine::Delivery::Assigned { assignment, .. } = delivery
                && assignment.task == task
            {
                Some(assignment.clone())
            } else {
                None
            }
        })
        .expect("task has a live assignment")
}

fn story(dependency_count: usize) -> Story {
    let bounds = limits();
    let mut driver = Driver::configured(
        temper_engine_domain_world::store::Store::new(),
        [configuration(), configuration()],
        &bounds,
    );
    driver.send(protocol::hello_slots(CHANNEL, bounds.fleet.slots, Box::new([])));
    driver.settle();
    driver.sign_in();
    driver.settle();
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: skein_lib::ReplyTo::new(Token::new(1001)),
        sign_in: driver.session(),
        key: [31; 16],
        ask: driver.chat_ask(1, b"coordinate live work"),
    }));
    driver.settle();
    let parent = driver.last_assignment();
    let mut batch = vec![fixtures::report_delegate(b"waiting worker", Box::new([]))];
    for _ in 0..dependency_count {
        batch.push(fixtures::report_delegate(b"dependency worker", Box::new([])));
    }
    fixtures::tool_call(&mut driver, &parent, 1, engine::Tool::Delegate { batch: batch.into_boxed_slice() });
    driver.settle();
    let Some(engine::CallAnswer::Delegated(children)) = driver.answered(1) else {
        panic!("delegation: {:?}", driver.answered(1));
    };
    let target = assignment(&driver, children[0].task);
    let dependencies = children[1..].iter().map(|child| assignment(&driver, child.task)).collect();
    Story { driver, parent, target, dependencies }
}

fn replace_dependencies(story: &mut Story, dependencies: Box<[u64]>, revision: u64, call: u64) {
    let mut amendment = fixtures::task_amendment(None, None, b"dependency requirements changed");
    amendment.dependencies = Some(dependencies);
    fixtures::tool_call(
        &mut story.driver,
        &story.parent,
        call,
        engine::Tool::Amend { target: story.target.task, revision, amendment },
    );
    assert!(
        matches!(story.driver.answered(call), Some(engine::CallAnswer::Amended { task, revision: next, .. }) if *task == story.target.task && *next == revision + 1),
        "dependency amendment must commit: {:?}",
        story.driver.answered(call)
    );
}

fn keep(story: &mut Story) {
    story.driver.env.now = skein_lib::Time::from_nanos(1_000_000_000);
    story.driver.env.wall = skein_lib::Wall::from_nanos(1_000_000_000);
    let dependencies = story.dependencies.iter().map(|dep| dep.task).collect();
    replace_dependencies(story, dependencies, 0, 2);
}

fn finish(driver: &mut Driver, run: &engine::Assignment, words: &[u8]) {
    driver.answer(
        CHANNEL,
        run.task,
        run.attempt,
        0,
        tasks::End::Finished { result: tasks::TaskResult::Report { words: words.into() }, cancel_delegates: false },
        engine::Parts::none(),
    );
    driver.settle();
}

fn park(story: &mut Story) {
    let tries = story.driver.task(story.target.task).expect("live task").tries;
    finish(&mut story.driver, &story.target, b"premature result");
    let row = story.driver.task(story.target.task).expect("kept task remains live");
    assert_eq!(row.phase, tasks::Phase::Active(tasks::Active::Idle), "successful answer while kept must park");
    assert_eq!(row.attempt, story.target.attempt, "parking must not start another attempt");
    assert_eq!(row.tries, tries, "parking must spend no retry");
}

fn results(run: &engine::Assignment) -> String {
    run.sections
        .iter()
        .filter_map(|section| match (&section.kind, &section.body) {
            (engine::BriefKind::Core(jig_domain_brief::Core::Results), engine::BriefBody::Text(bytes)) => {
                Some(String::from_utf8(bytes.to_vec()).expect("brief text"))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn restart(story: &Story, live_dependencies: &[engine::Assignment]) -> Driver {
    let mut restored = Driver::cold(story.driver.store.fork(), configuration(), &limits());
    let hosting = std::iter::once(&story.parent)
        .chain(live_dependencies)
        .map(|run| protocol::hosting(run.task, run.attempt, jig_domain_host::Phase::Active))
        .collect();
    restored.send(protocol::hello_slots(CHANNEL, limits().fleet.slots, hosting));
    restored.settle();
    restored
}

#[test]
fn kept_success_parks_without_spending_a_try() {
    let mut story = story(1);
    keep(&mut story);
    let tries = story.driver.task(story.target.task).expect("live").tries;
    park(&mut story);
    assert_eq!(story.driver.task(story.target.task).expect("parked").tries, tries);
    assert_eq!(assignment(&story.driver, story.target.task).attempt, story.target.attempt, "kept task starts no run");
}

#[test]
fn one_met_of_two_cannot_resume_the_kept_run() {
    let mut story = story(2);
    keep(&mut story);
    park(&mut story);
    finish(&mut story.driver, &story.dependencies[1], b"second proof");
    let row = story.driver.task(story.target.task).expect("kept");
    assert_eq!(row.waiting_on.as_ref(), [story.dependencies[0].task], "only the unmet dependency keeps the task");
    assert_eq!(
        row.phase,
        tasks::Phase::Active(tasks::Active::Idle),
        "one Met must not wake a task with another pending dependency"
    );
    assert_eq!(assignment(&story.driver, story.target.task).attempt, story.target.attempt);
}

#[test]
fn last_met_brief_names_both_results_and_the_added_dependency_history() {
    let mut story = story(2);
    keep(&mut story);
    park(&mut story);
    finish(&mut story.driver, &story.dependencies[1], b"second proof");
    finish(&mut story.driver, &story.dependencies[0], b"first proof");
    let resumed = assignment(&story.driver, story.target.task);
    assert_ne!(resumed.attempt, story.target.attempt, "last Met must start the next run");
    let text = results(&resumed);
    for (dep, proof) in story.dependencies.iter().zip(["first proof", "second proof"]) {
        assert!(text.contains(proof), "dependency {} result missing from brief: {text}", dep.task);
        let waited = format!("waited for task {}, added 1000000000 by task {}", dep.task, story.parent.task);
        assert!(text.contains(&waited), "brief must say what, when and who kept the task: {text}");
    }
}

#[test]
fn a_kept_run_goes_on_after_engine_restart_and_last_met() {
    let mut story = story(2);
    keep(&mut story);
    park(&mut story);
    finish(&mut story.driver, &story.dependencies[0], b"first proof");
    let mut restored = restart(&story, &story.dependencies[1..]);
    assert_eq!(
        restored.task(story.target.task).expect("kept task restored").phase,
        tasks::Phase::Active(tasks::Active::Idle)
    );
    assert!(!restored.delivered.iter().any(|delivery| matches!(delivery, engine::Delivery::Assigned { assignment, .. } if assignment.task == story.target.task)), "restart must not start a still-kept task");
    finish(&mut restored, &story.dependencies[1], b"second proof");
    let resumed = assignment(&restored, story.target.task);
    let text = results(&resumed);
    assert!(
        text.contains("first proof") && text.contains("second proof"),
        "cold restore loses no dependency result: {text}"
    );
    for dep in &story.dependencies {
        assert!(
            text.contains(&format!("waited for task {}", dep.task)),
            "cold restore loses no waited-for history: {text}"
        );
    }
}

#[test]
fn met_before_the_live_run_answers_allows_that_run_to_finish() {
    let mut story = story(1);
    keep(&mut story);
    finish(&mut story.driver, &story.dependencies[0], b"dependency proof");
    assert_eq!(
        assignment(&story.driver, story.target.task).attempt,
        story.target.attempt,
        "Met must not replace a run that is still live"
    );
    finish(&mut story.driver, &story.target, b"completed after Met");
    let key = engine::Key::Core(jig_domain_core::Key::Tasks(tasks::Key::Ended(story.target.task)));
    let Some(engine::Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Ended(row)))) =
        story.driver.store.rows.get(&key)
    else {
        panic!("the live run must commit an ended record after Met")
    };
    assert_eq!(
        row.phase,
        tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Report {
            words: b"completed after Met".as_slice().into()
        })),
        "the live run may end done once Met"
    );
}

#[test]
fn words_received_while_kept_wait_for_the_resumed_run() {
    const WORD: u8 = 65;
    let mut story = story(1);
    keep(&mut story);
    park(&mut story);
    let message = story.driver.say(story.target.task, WORD);
    assert_eq!(
        story.driver.task(story.target.task).expect("kept").phase,
        tasks::Phase::Active(tasks::Active::Idle),
        "ordinary words must not bypass a pending dependency"
    );
    finish(&mut story.driver, &story.dependencies[0], b"dependency proof");
    let resumed = assignment(&story.driver, story.target.task);
    assert!(
        resumed.messages.iter().any(|word| word.number == message && word.words.as_ref() == [WORD]),
        "words received while kept must accompany the resumed assignment"
    );
}

#[test]
fn turned_conversation_survives_a_kept_wait_and_cold_restore() {
    const TURN: &[u8] = b"work done before the dependency";
    let mut story = story(1);
    story.driver.send(protocol::turn(CHANNEL, story.target.task, story.target.attempt, 1, 0, None, TURN));
    story.driver.settle();
    keep(&mut story);
    park(&mut story);
    let mut restored = restart(&story, &story.dependencies);
    finish(&mut restored, &story.dependencies[0], b"dependency proof");
    let resumed = assignment(&restored, story.target.task);
    assert_eq!(
        resumed.conversation, story.target.conversation,
        "the kept wait must continue the conversation that already turned"
    );
    assert!(
        resumed.transcript.iter().any(|turn| turn.as_ref() == TURN),
        "the resumed run must receive its pre-dependency turn"
    );
}

#[test]
fn removing_the_last_pending_dependency_resumes_the_parked_run() {
    let mut story = story(2);
    keep(&mut story);
    park(&mut story);
    finish(&mut story.driver, &story.dependencies[0], b"first proof");
    let dependencies = Box::new([story.dependencies[0].task]);
    replace_dependencies(&mut story, dependencies, 1, 3);
    story.driver.settle();
    let resumed = assignment(&story.driver, story.target.task);
    assert_ne!(resumed.attempt, story.target.attempt, "removing the last pending dependency is Met for the parked run");
    assert!(
        matches!(
            story.driver.task(story.dependencies[1].task).expect("removed dependency is still live").phase,
            tasks::Phase::Active(tasks::Active::Running { .. })
        ),
        "resumption cannot depend on the removed task ending"
    );
}

#[test]
fn a_second_wait_added_while_parked_keeps_both_addition_times() {
    let mut story = story(2);
    let dependencies = Box::new([story.dependencies[0].task]);
    story.driver.env.now = skein_lib::Time::from_nanos(1_000_000_000);
    story.driver.env.wall = skein_lib::Wall::from_nanos(1_000_000_000);
    replace_dependencies(&mut story, dependencies, 0, 2);
    park(&mut story);
    story.driver.env.now = skein_lib::Time::from_nanos(2_000_000_000);
    story.driver.env.wall = skein_lib::Wall::from_nanos(2_000_000_000);
    let dependencies = story.dependencies.iter().map(|dep| dep.task).collect();
    replace_dependencies(&mut story, dependencies, 1, 3);
    finish(&mut story.driver, &story.dependencies[0], b"first proof");
    assert_eq!(
        assignment(&story.driver, story.target.task).attempt,
        story.target.attempt,
        "the newly added wait must also be met"
    );
    finish(&mut story.driver, &story.dependencies[1], b"second proof");
    let text = results(&assignment(&story.driver, story.target.task));
    for (dep, when) in story.dependencies.iter().zip([1_000_000_000_u64, 2_000_000_000]) {
        let waited = format!("waited for task {}, added {when} by task {}", dep.task, story.parent.task);
        assert!(text.contains(&waited), "later amendments must preserve each dependency's original addition: {text}");
    }
}
