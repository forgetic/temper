//! Adversarial dependency amendments through the engine's public world.
use jig_domain_tasks as tasks;
use skein_lib::{Queue, ReplyTo, Token};
use temper_engine_domain as engine;
use temper_engine_domain_world::{direct::Driver, fixtures, protocol, store::Store, walking};

const CHANNEL: Token = Token::new(7);
// Chat plus up to four siblings; two pending dependencies exercise both sides of the bound.
const TASKS: u32 = 6;
const DEPENDENCIES: u32 = 2;
const DELEGATE_CALL: u64 = 10;
const AMEND_CALL: u64 = 20;
const FOLLOWUP_CALL: u64 = 21;
// One initial assignment, then exactly one more after the final wait lifts.
const INITIAL_RUNS: usize = 1;
const RESUMED_RUNS: usize = 2;
const ONE_MET: usize = 1;
const REPORT: &[u8] = b"report before release";

fn limits() -> engine::Limits {
    let mut bounds = walking::limits();
    bounds.tasks.tasks = TASKS;
    bounds.tasks.project_tasks = TASKS;
    bounds.tasks.tree_tasks = TASKS;
    bounds.tasks.depth = 2;
    bounds.tasks.delegates = 4;
    bounds.tasks.references = TASKS;
    bounds.tasks.batch = 3;
    bounds.tasks.dependencies = DEPENDENCIES;
    bounds.tasks.inbox_messages = 8;
    bounds.tasks.inbox_bytes = 512;
    bounds.people.inbox_tasks = TASKS * 2;
    bounds.people.inbox_recipients = TASKS * 4 + 1;
    bounds.authority.batch = 3;
    bounds.fleet.attempts = TASKS + 1;
    bounds.fleet.slots = TASKS - 1;
    bounds.call_records = 8;
    bounds.brief.sections = 5;
    bounds.journal.writes = walking::writes(&bounds).max(8192);
    bounds.journal.held = 300;
    bounds.journal.deliveries = 100;
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
    let mut out = Queue::with_capacity(jig_domain_authority::POLICY_MAX_OUT);
    jig_domain_authority::step(&mut authority, jig_domain_authority::Event::Policy { project: 1, policy }, &mut out);
    config.core.seed_projects[0].policy.policy = authority.policy(1).expect("configured seed policy").clone();
    config.core.defaults.rules = authority.rules().clone();
    config
}

fn story() -> (Driver, engine::Assignment) {
    let mut driver = Driver::configured(Store::new(), [configuration(), configuration()], &limits());
    driver.send(protocol::hello_slots(CHANNEL, limits().fleet.slots, Box::new([])));
    driver.settle();
    driver.sign_in();
    driver.settle();
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: ReplyTo::new(Token::new(1001)),
        sign_in: driver.session(),
        key: [31; 16],
        ask: driver.chat_ask(1, b"amend my live delegates"),
    }));
    driver.settle();
    let parent = driver.last_assignment();
    (driver, parent)
}

fn siblings(driver: &mut Driver, parent: &engine::Assignment, names: &[&[u8]]) -> Vec<u64> {
    const BATCH: usize = 3; // the configured task and authority batch bounds.
    let mut tasks = Vec::new();
    for (index, names) in names.chunks(BATCH).enumerate() {
        let call = DELEGATE_CALL + u64::try_from(index).expect("small batch count");
        fixtures::tool_call(
            driver,
            parent,
            call,
            engine::Tool::Delegate {
                batch: names.iter().map(|name| fixtures::report_delegate(name, Box::new([]))).collect(),
            },
        );
        let Some(engine::CallAnswer::Delegated(children)) = driver.answered(call) else {
            panic!("delegate fixture: {:?}", driver.answered(call));
        };
        tasks.extend(children.iter().map(|child| child.task));
    }
    tasks
}

fn assignment(driver: &Driver, task: u64) -> engine::Assignment {
    driver
        .delivered
        .iter()
        .rev()
        .find_map(|delivery| {
            if let engine::Delivery::Assigned { assignment, .. } = delivery {
                (assignment.task == task).then(|| assignment.clone())
            } else {
                None
            }
        })
        .expect("delegate's live run")
}

fn amend(driver: &mut Driver, parent: &engine::Assignment, call: u64, target: u64, revision: u64, deps: &[u64]) {
    let mut amendment = fixtures::task_amendment(None, None, b"wait for prerequisites");
    amendment.dependencies = Some(deps.into());
    fixtures::tool_call(driver, parent, call, engine::Tool::Amend { target, revision, amendment });
}

fn finish(driver: &mut Driver, run: &engine::Assignment) {
    driver.answer(
        CHANNEL,
        run.task,
        run.attempt,
        0,
        tasks::End::Finished { result: tasks::TaskResult::Report { words: REPORT.into() }, cancel_delegates: false },
        engine::Parts::none(),
    );
    driver.settle();
}

fn starts(driver: &Driver, task: u64) -> usize {
    driver
        .delivered
        .iter()
        .filter(|delivery| {
            matches!(delivery,
        engine::Delivery::Assigned { assignment, .. } if assignment.task == task)
        })
        .count()
}

fn kept_story() -> (Driver, engine::Assignment, u64, u64) {
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"report", b"prerequisite"]);
    let [report, prerequisite] = children.as_slice() else { panic!("two delegates") };
    amend(&mut driver, &parent, AMEND_CALL, *report, 0, &[*prerequisite]);
    (driver, parent, *report, *prerequisite)
}

#[test]
fn done_live_delegate_parks_until_its_prerequisite_finishes() {
    // jig's domain/tasks.md 5.7; domain/engine.md 7.4: done while kept parks.
    let (mut driver, _, report, prerequisite) = kept_story();
    let run = assignment(&driver, report);
    finish(&mut driver, &run);
    assert!(
        matches!(driver.task(report).map(|row| &row.phase), Some(tasks::Phase::Active(tasks::Active::Idle))),
        "a kept done answer must leave the task active and parked: {:?}",
        driver.task(report)
    );
    assert_eq!(starts(&driver, report), INITIAL_RUNS, "no replacement run while prerequisite is pending");
    finish_task(&mut driver, prerequisite);
    assert_eq!(starts(&driver, report), RESUMED_RUNS, "last dependency completion wakes exactly one run");
}

fn finish_task(driver: &mut Driver, task: u64) {
    let run = assignment(driver, task);
    finish(driver, &run);
}

#[test]
fn replacing_the_list_removes_the_old_dependency() {
    // jig's domain/tasks.md 7: the list replaces, rather than accumulates.
    let (mut driver, parent, report, _) = kept_story();
    let run = assignment(&driver, report);
    finish(&mut driver, &run);
    amend(&mut driver, &parent, FOLLOWUP_CALL, report, 1, &[]);
    assert_eq!(starts(&driver, report), RESUMED_RUNS, "removing the final dependency resumes the parked delegate");
}

#[test]
fn ordinary_words_cannot_wake_a_kept_delegate() {
    // jig's domain/tasks.md 5.7: messages wait while kept, without a new run.
    let (mut driver, _, report, _) = kept_story();
    let run = assignment(&driver, report);
    finish(&mut driver, &run);
    driver.say(report, 72);
    assert_eq!(starts(&driver, report), INITIAL_RUNS, "words must stay queued until dependencies are met");
}

#[test]
fn ancestor_dependency_is_refused_without_mutation() {
    // jig's domain/tasks.md 6.7 and 7: requester waits for its delegate already.
    let (mut driver, parent, report, _) = kept_story();
    let before = driver.task(report).expect("live report").clone();
    amend(&mut driver, &parent, FOLLOWUP_CALL, report, 1, &[parent.task]);
    assert!(
        format!("{:?}", driver.answered(FOLLOWUP_CALL)).contains("Circle"),
        "ancestor dependency must explain the circle: {:?}",
        driver.answered(FOLLOWUP_CALL)
    );
    assert_eq!(driver.task(report), Some(&before), "refused amendment changes nothing");
}

#[test]
fn duplicate_dependencies_cannot_create_two_waits() {
    // jig's domain/tasks.md 7: the amendment supplies a dependency list, not duplicate waits.
    let (mut driver, parent, report, prerequisite) = kept_story();
    let before = driver.task(report).expect("live report").clone();
    amend(&mut driver, &parent, FOLLOWUP_CALL, report, 1, &[prerequisite, prerequisite]);
    // jig's tasks.md 7 is silent on duplicates: either refuse atomically or count it once.
    match driver.answered(FOLLOWUP_CALL) {
        Some(engine::CallAnswer::ControlRefused(_)) => {
            assert_eq!(driver.task(report), Some(&before), "duplicate rejection preserves the old list and revision");
        }
        Some(engine::CallAnswer::Amended { .. }) => {
            let row = driver.task(report).expect("live report");
            assert_eq!(row.dependencies.as_ref(), &[prerequisite], "a duplicate represents a single prerequisite");
        }
        answer => panic!("duplicate list must be refused or normalized: {answer:?}"),
    }
    finish_task(&mut driver, report);
    finish_task(&mut driver, prerequisite);
    assert_eq!(starts(&driver, report), RESUMED_RUNS, "a duplicated prerequisite releases exactly one new run");
    let met = driver
        .task(report)
        .expect("resumed report")
        .inbox
        .iter()
        .filter(|word| matches!(word.kind, tasks::MessageKind::Met { dependency, .. } if dependency == prerequisite))
        .count();
    assert_eq!(met, ONE_MET, "a duplicated prerequisite produces one Met");
}

#[test]
fn stale_revision_cannot_remove_pending_dependencies() {
    // jig's domain/tasks.md 7: an amendment names the spec revision it saw.
    let (mut driver, parent, report, _) = kept_story();
    let before = driver.task(report).expect("live report").clone();
    amend(&mut driver, &parent, FOLLOWUP_CALL, report, 0, &[]);
    assert!(
        format!("{:?}", driver.answered(FOLLOWUP_CALL)).contains("Moved"),
        "stale removal must report the current revision: {:?}",
        driver.answered(FOLLOWUP_CALL)
    );
    assert_eq!(driver.task(report), Some(&before), "stale removal changes nothing");
}

#[test]
fn dependencies_at_the_bound_wait_for_the_last() {
    // jig's domain/tasks.md 5.7 and 7: each pending prerequisite keeps the task.
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"report", b"first", b"last"]);
    let [report, first, last] = children.as_slice() else { panic!("report and two prerequisites") };
    amend(&mut driver, &parent, AMEND_CALL, *report, 0, &[*first, *last]);
    finish_task(&mut driver, *report);
    finish_task(&mut driver, *first);
    assert_eq!(starts(&driver, *report), INITIAL_RUNS, "one remaining dependency still keeps the report");
    finish_task(&mut driver, *last);
    assert_eq!(
        starts(&driver, *report),
        RESUMED_RUNS,
        "a list at the two-dependency bound must be admitted and resume"
    );
}

#[test]
fn one_dependency_past_the_bound_changes_nothing() {
    // jig's domain/tasks.md 7: bounded dependency admission is whole.
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"report", b"first", b"second", b"excess"]);
    let [report, first, second, excess] = children.as_slice() else { panic!("report and three prerequisites") };
    let before = driver.task(*report).expect("live report").clone();
    amend(&mut driver, &parent, AMEND_CALL, *report, 0, &[*first, *second, *excess]);
    assert!(
        format!("{:?}", driver.answered(AMEND_CALL)).contains("Refused"),
        "one past the dependency bound must be refused: {:?}",
        driver.answered(AMEND_CALL)
    );
    assert_eq!(driver.task(*report), Some(&before), "over-bound amendment must not retain a prefix");
}

#[test]
fn a_three_task_wait_circle_is_refused() {
    const CLOSE_CIRCLE: u64 = 22;
    // jig's domain/tasks.md 6.7: check transitive waits, not just reciprocal pairs.
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"first", b"second", b"third"]);
    let [first, second, third] = children.as_slice() else { panic!("three delegates") };
    amend(&mut driver, &parent, AMEND_CALL, *first, 0, &[*second]);
    amend(&mut driver, &parent, FOLLOWUP_CALL, *second, 0, &[*third]);
    let before = driver.task(*third).expect("live third delegate").clone();
    amend(&mut driver, &parent, CLOSE_CIRCLE, *third, 0, &[*first]);
    let answer = format!("{:?}", driver.answered(CLOSE_CIRCLE));
    assert!(answer.contains("Circle"), "transitive dependency circle must be named: {answer}");
    assert_eq!(driver.task(*third), Some(&before), "refused circle must not change the third task");
}

#[test]
fn removing_one_dependency_preserves_the_other() {
    // jig's domain/tasks.md 7: omitted prerequisites are removed; named ones remain.
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"report", b"removed", b"retained"]);
    let [report, removed, retained] = children.as_slice() else { panic!("three delegates") };
    amend(&mut driver, &parent, AMEND_CALL, *report, 0, &[*removed, *retained]);
    finish_task(&mut driver, *report);
    amend(&mut driver, &parent, FOLLOWUP_CALL, *report, 1, &[*retained]);
    finish_task(&mut driver, *removed);
    assert_eq!(
        starts(&driver, *report),
        INITIAL_RUNS,
        "completion of a removed dependency cannot release the retained one"
    );
    finish_task(&mut driver, *retained);
    assert_eq!(starts(&driver, *report), RESUMED_RUNS, "the retained dependency alone releases the task");
}

#[test]
fn a_sibling_cannot_amend_another_sibling() {
    // jig's domain/tasks.md 7, Who may: only requester or ancestors may amend.
    let (mut driver, _, report, prerequisite) = kept_story();
    let sibling = assignment(&driver, prerequisite);
    let before = driver.task(report).expect("live report").clone();
    amend(&mut driver, &sibling, FOLLOWUP_CALL, report, 1, &[]);
    assert!(
        format!("{:?}", driver.answered(FOLLOWUP_CALL)).contains("Refused"),
        "sibling amendment must be refused: {:?}",
        driver.answered(FOLLOWUP_CALL)
    );
    assert_eq!(driver.task(report), Some(&before), "a sibling cannot remove another's keep");
}

#[test]
fn a_dependency_outside_the_amenders_reach_is_refused() {
    // jig's domain/tasks.md 7: added prerequisites must be within the amender's reach.
    let (mut driver, parent, report, _) = kept_story();
    // A separately created top-level chat belongs to the party, outside this agent's tree/references.
    driver.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: ReplyTo::new(Token::new(1002)),
        sign_in: driver.session(),
        key: [32; 16],
        ask: driver.chat_ask(1, b"unrelated root"),
    }));
    driver.settle();
    let outside = driver.last_assignment().task;
    let before = driver.task(report).expect("live report").clone();
    amend(&mut driver, &parent, FOLLOWUP_CALL, report, 1, &[outside]);
    assert!(
        format!("{:?}", driver.answered(FOLLOWUP_CALL)).contains("Refused"),
        "unreferenced outside task must be refused: {:?}",
        driver.answered(FOLLOWUP_CALL)
    );
    assert_eq!(driver.task(report), Some(&before), "unreachable prerequisite changes nothing");
}

#[test]
fn restarting_a_parked_kept_task_does_not_start_it() {
    // jig's domain/tasks.md 5.7 and 6.7: restoration preserves the pending keep.
    let (mut driver, parent, report, prerequisite) = kept_story();
    finish_task(&mut driver, report);
    let prerequisite_run = assignment(&driver, prerequisite);
    let mut restored = Driver::cold(driver.store.fork(), configuration(), &limits());
    restored.events.push_back(protocol::hello_slots(
        CHANNEL,
        limits().fleet.slots,
        Box::new([
            protocol::hosting(parent.task, parent.attempt, jig_domain_host::Phase::Active),
            protocol::hosting(prerequisite_run.task, prerequisite_run.attempt, jig_domain_host::Phase::Active),
        ]),
    ));
    restored.settle();
    assert_eq!(starts(&restored, report), 0, "restart must not claim a kept delegate");
    amend(&mut restored, &parent, FOLLOWUP_CALL, report, 1, &[]);
    assert_eq!(starts(&restored, report), INITIAL_RUNS, "removing its last restored dependency wakes it");
}

fn cancel(driver: &mut Driver, parent: &engine::Assignment, call: u64, target: u64) {
    fixtures::tool_call(
        driver,
        parent,
        call,
        engine::Tool::Cancel { target, reason: b"cancel prerequisite".as_slice().into() },
    );
}

#[test]
fn a_cancelled_dependency_is_refused_by_name() {
    // jig's domain/tasks.md 7: a cancelled prerequisite cannot be newly added.
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"report", b"cancelled prerequisite"]);
    let [report, prerequisite] = children.as_slice() else { panic!("two delegates") };
    let run = assignment(&driver, *prerequisite);
    cancel(&mut driver, &parent, AMEND_CALL, *prerequisite);
    driver.answer(
        CHANNEL,
        run.task,
        run.attempt,
        0,
        tasks::End::Finished { result: tasks::TaskResult::Report { words: REPORT.into() }, cancel_delegates: false },
        engine::Parts::none(),
    );
    driver.settle();
    let before = driver.task(*report).expect("live report").clone();
    amend(&mut driver, &parent, FOLLOWUP_CALL, *report, 0, &[*prerequisite]);
    let answer = format!("{:?}", driver.answered(FOLLOWUP_CALL));
    assert!(
        answer.contains("Refused") && answer.contains(&prerequisite.to_string()),
        "cancelled dependency refusal must name its task: {answer}"
    );
    assert_eq!(driver.task(*report), Some(&before), "ended prerequisite refusal is atomic");
}

#[test]
fn a_declared_failure_ends_a_kept_task() {
    // jig's domain/tasks.md 5.7: a failure ends it as ever, rather than parking.
    let (mut driver, _, report, _) = kept_story();
    let run = assignment(&driver, report);
    driver.answer(
        CHANNEL,
        run.task,
        run.attempt,
        0,
        tasks::End::Finished {
            result: tasks::TaskResult::Failure { reason: b"declared failure".as_slice().into() },
            cancel_delegates: false,
        },
        engine::Parts::none(),
    );
    driver.settle();
    assert!(driver.task(report).is_none(), "a declared failure must end a kept task: {:?}", driver.task(report));
    let ended = driver.store.rows.get(&engine::Key::Core(jig_domain_core::Key::Tasks(tasks::Key::Ended(report))));
    assert!(
        matches!(ended, Some(engine::Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Ended(row))))
        if matches!(&row.phase, tasks::Phase::Ended(tasks::Ending::Failed { .. }))),
        "the kept task must retain its failed result: {ended:?}"
    );
}

#[test]
fn a_cancel_ends_a_parked_kept_task_immediately() {
    // jig's domain/tasks.md 5.7 and 7: a kept task with no run can be cancelled.
    let (mut driver, parent, report, _) = kept_story();
    finish_task(&mut driver, report);
    cancel(&mut driver, &parent, FOLLOWUP_CALL, report);
    assert!(driver.task(report).is_none(), "cancel must end a parked kept task without waiting for its prerequisite");
    assert_eq!(starts(&driver, report), INITIAL_RUNS, "cancel must not start a new run to finish");
}

#[test]
fn a_failed_dependency_is_refused_by_name() {
    // jig's domain/tasks.md 7: an already failed prerequisite is refused, naming it.
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"report", b"failed prerequisite"]);
    let [report, prerequisite] = children.as_slice() else { panic!("two delegates") };
    let run = assignment(&driver, *prerequisite);
    driver.answer(
        CHANNEL,
        run.task,
        run.attempt,
        0,
        tasks::End::Finished {
            result: tasks::TaskResult::Failure { reason: b"cannot do it".as_slice().into() },
            cancel_delegates: false,
        },
        engine::Parts::none(),
    );
    driver.settle();
    let before = driver.task(*report).expect("live report").clone();
    amend(&mut driver, &parent, AMEND_CALL, *report, 0, &[*prerequisite]);
    let answer = format!("{:?}", driver.answered(AMEND_CALL));
    assert!(
        answer.contains("Refused") && answer.contains(&prerequisite.to_string()),
        "failed dependency refusal must name its task: {answer}"
    );
    assert_eq!(driver.task(*report), Some(&before), "failed prerequisite refusal is atomic");
}

#[test]
fn an_already_done_retained_dependency_does_not_keep() {
    // jig's domain/tasks.md 7: a done prerequisite counts as met, even if its end raced the add.
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"first dependent", b"prerequisite", b"new dependent"]);
    let [first, prerequisite, report] = children.as_slice() else { panic!("three delegates") };
    // The first dependent retains the prerequisite in the working set after its end.
    amend(&mut driver, &parent, AMEND_CALL, *first, 0, &[*prerequisite]);
    finish_task(&mut driver, *prerequisite);
    amend(&mut driver, &parent, FOLLOWUP_CALL, *report, 0, &[*prerequisite]);
    assert!(
        matches!(driver.answered(FOLLOWUP_CALL), Some(engine::CallAnswer::Amended { revision: 1, .. })),
        "an already done prerequisite is admitted as met: {:?}",
        driver.answered(FOLLOWUP_CALL)
    );
    finish_task(&mut driver, *report);
    assert!(driver.task(*report).is_none(), "an already done dependency cannot park the new dependent");
}

#[test]
fn a_live_run_hears_the_amendment_that_keeps_it() {
    // jig's domain/tasks.md 5.7 and 7: a run already live hears the amendment as it arrives.
    let (driver, _, report, _) = kept_story();
    let run = assignment(&driver, report);
    let messages: Vec<_> = driver
        .delivered
        .iter()
        .filter_map(|delivery| {
            if let engine::Delivery::Link {
                down: jig_domain_host::LinkDown::Message { run: task, attempt, words, .. },
                ..
            } = delivery
            {
                (*task == Token::new(report) && *attempt == Token::new(run.attempt)).then_some(words.as_ref())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        messages,
        vec![b"wait for prerequisites".as_slice()],
        "the current live run must receive the keeping amendment once"
    );
    assert_eq!(starts(&driver, report), INITIAL_RUNS, "keeping a live run must not replace it");
}

fn met_notices(run: &engine::Assignment) -> Vec<(u64, bool)> {
    run.messages
        .iter()
        .filter_map(|word| {
            if let tasks::MessageKind::Met { dependency, removed } = word.kind {
                Some((dependency, removed))
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn final_dependency_met_is_in_the_resumed_runs_start() {
    // jig's domain/tasks.md 5.7, 8.1: completion wakes once, carrying the Met notice.
    let (mut driver, _, report, prerequisite) = kept_story();
    finish_task(&mut driver, report);
    finish_task(&mut driver, prerequisite);
    let resumed = assignment(&driver, report);
    let met: Vec<_> = met_notices(&resumed);
    assert_eq!(met, vec![(prerequisite, false)], "Met must be available when the resumed run starts");
}

#[test]
fn removing_the_last_dependency_emits_a_removed_met() {
    // jig's domain/tasks.md 7 and 8.1: removal wakes with a notice as completion does.
    let (mut driver, parent, report, prerequisite) = kept_story();
    finish_task(&mut driver, report);
    amend(&mut driver, &parent, FOLLOWUP_CALL, report, 1, &[]);
    let resumed = assignment(&driver, report);
    let met: Vec<_> = met_notices(&resumed);
    assert_eq!(met, vec![(prerequisite, true)], "the resumed start must distinguish removal from completion");
}

#[test]
fn removing_a_dependency_detaches_its_later_cancellation() {
    const CANCEL_REMOVED: u64 = 22;
    // jig's domain/tasks.md 7: removing a prerequisite lifts its wait entirely.
    let (mut driver, parent, report, prerequisite) = kept_story();
    finish_task(&mut driver, report);
    amend(&mut driver, &parent, FOLLOWUP_CALL, report, 1, &[]);
    cancel(&mut driver, &parent, CANCEL_REMOVED, prerequisite);
    finish_task(&mut driver, prerequisite);
    assert!(
        matches!(driver.task(report).map(|row| &row.phase), Some(tasks::Phase::Active(tasks::Active::Running { .. }))),
        "a removed prerequisite's cancellation must not hold the resumed report: {:?}",
        driver.task(report)
    );
}

#[test]
fn parking_a_kept_done_run_spends_no_failure_try() {
    // jig's domain/tasks.md 5.7; engine.md 7.4: a done run parked while kept spends no try.
    let (mut driver, _, report, _) = kept_story();
    let before = driver.task(report).expect("live report").tries;
    finish_task(&mut driver, report);
    assert_eq!(
        driver.task(report).expect("kept report").tries,
        before,
        "parking for dependencies must leave every failure counter unchanged"
    );
}

#[test]
fn a_turn_committed_while_kept_resumes_in_the_transcript() {
    // jig's domain/tasks.md 5.7; engine.md 7.2, 7.4: the current turn and saved state survive parking.
    const TURN: &[u8] = b"work finished while kept";
    const FIRST_TURN: u32 = 1;
    let (mut driver, _, report, prerequisite) = kept_story();
    let run = assignment(&driver, report);
    let read = driver.task(report).expect("live report").inbox.last().map(|word| word.number);
    driver.send(protocol::turn(CHANNEL, report, run.attempt, FIRST_TURN, 0, read, TURN));
    driver.settle();
    let mut event = driver.protocol.answer(
        CHANNEL,
        report,
        run.attempt,
        0,
        tasks::End::Finished { result: tasks::TaskResult::Report { words: REPORT.into() }, cancel_delegates: false },
        engine::Parts::none(),
    );
    let engine::Event::Worker(engine::WorkerInput::Link {
        link: engine::Link::Up { up: jig_domain_host::LinkUp::Answer { answer, .. }, .. },
        ..
    }) = &mut event
    else {
        panic!("the world's answer is a worker link")
    };
    answer.turns = FIRST_TURN;
    driver.send(event);
    driver.settle();
    finish_task(&mut driver, prerequisite);
    let resumed = assignment(&driver, report);
    assert!(
        resumed.transcript.iter().any(|turn| turn.as_ref() == TURN),
        "the resumed run must receive the turn committed while kept: {:?}",
        resumed.transcript
    );
}

#[test]
fn only_the_final_pending_dependency_produces_met() {
    // jig's domain/tasks.md 8.1: the notice is for the last pending dependency.
    let (mut driver, parent) = story();
    let children = siblings(&mut driver, &parent, &[b"report", b"first", b"last"]);
    let [report, first, last] = children.as_slice() else { panic!("three delegates") };
    amend(&mut driver, &parent, AMEND_CALL, *report, 0, &[*first, *last]);
    finish_task(&mut driver, *report);
    finish_task(&mut driver, *first);
    finish_task(&mut driver, *last);
    let resumed = assignment(&driver, *report);
    let notices: Vec<_> = met_notices(&resumed);
    assert_eq!(notices, vec![(*last, false)], "an earlier completion must not produce a premature Met");
}
