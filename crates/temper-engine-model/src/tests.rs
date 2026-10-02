//! The top level's step tests: its routing and translations, through its
//! boundary, with the forge, workers and people scripted inline.

use alloc::boxed::Box;

use temper_engine_model_brief::{self as brief, Budgets};
use temper_engine_model_fleet as fleet;
use temper_engine_model_forge::{self as forge, api};
use temper_engine_model_notes as notes;
use temper_engine_model_plan::{self as plan, Budget};
use temper_engine_model_rules::{self as rules, Acts, Permission, Rules};
use temper_engine_model_views::{self as views, Capture, Policy};
use temper_engine_model_work::{self as work, Retries, Retry};
use temper_lib::bytes::copy_of;
use temper_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token};

use crate::boundary::{Ask, Event, Hello, Item, Refusal, Reply, Request};
use crate::config::Config;
use crate::limits::{Limits, accepts, worst_case};
use crate::model::{Model, fire, max_out, step};
use crate::translate;

const RETRY: Retry = Retry { retries: 2, base: Duration::from_secs(1), max: Duration::from_secs(8) };

const BUDGET: Budget = Budget { tokens: 1_000, turns: 20, time: Duration::from_secs(600) };

const LIMITS: Limits = Limits {
    work: work::Limits {
        items: 4,
        retries: Retries { transient: RETRY, permanent: RETRY, run: RETRY, agent: RETRY, lost: RETRY, invalid: RETRY },
        undelivered: 2,
        facts: 64,
    },
    plan: plan::Limits {
        steps: 8,
        name_bytes: 16,
        dependencies: 4,
        gates: 2,
        targets: 2,
        instruction_bytes: 64,
        tasks: 3,
        events: 8,
        repairs: 2,
        rebases: 4,
        rejections: 2,
        stall: Duration::from_secs(3600),
        budget: BUDGET,
    },
    rules: rules::Limits { repositories: 2, protected: 2, branch_bytes: 16, grants: 4, reviews: 4, gates: 4, lands: 4 },
    forge: forge::Limits {
        repositories: 2,
        items: 4,
        labels: 3,
        members: 3,
        inbox: 4,
        reviewers: 4,
        reads: 16,
        writes: 16,
        calls: 16,
        page: 4,
        name_bytes: 32,
        title_bytes: 64,
        body_bytes: 256,
        rate: 1000,
        window: Duration::from_secs(60),
        reserve: 0,
        poll: Duration::from_secs(30),
        hinted: Duration::from_secs(2),
        resolution: Duration::from_secs(1),
        slow: Duration::from_secs(600),
        probes: 2,
        backoff: Duration::from_secs(1),
        backoff_max: Duration::from_secs(8),
        attempts: 3,
        lifetime: Duration::ZERO,
        facts: 256,
    },
    fleet: fleet::Limits {
        workers: 2,
        slots: 2,
        workstreams: 2,
        workstream_bytes: 16,
        attempts: 8,
        calls: 4,
        grace: Duration::from_secs(10),
        facts: 64,
    },
    brief: brief::Limits {
        briefs: 4,
        sections: 10,
        items: 4,
        parts: 4,
        read_bytes: 256,
        budgets: Budgets {
            item: 64,
            comments: 64,
            dependencies: 64,
            ci: 64,
            reviews: 64,
            pull: 64,
            attempts: 64,
            plan: 64,
            notes: 64,
            template: 64,
        },
        brief_bytes: 640,
        gather: Duration::from_secs(10),
        facts: 64,
    },
    notes: notes::Limits {
        scopes: 3,
        entries: 4,
        name_bytes: 16,
        description_bytes: 32,
        body_bytes: 64,
        references: 2,
        calls: 3,
        lines: 4,
        recalled: 2,
        facts: 64,
    },
    views: views::Limits {
        runs: 4,
        watchers: 4,
        backlog: 4,
        report_bytes: 64,
        records: 4,
        batch_bytes: 256,
        appends: 2,
        flush: Duration::from_secs(1),
        retention: Duration::from_secs(600),
        sweep: Duration::from_secs(60),
        facts: 64,
    },
    asks: 4,
    text_bytes: 256,
    steps: 16,
    facts: 64,
};

/// The engine's forge user, and a person.
const ENGINE: u64 = 99;
const ALICE: u64 = 1;

const ITEM: Item = Item { repository: 0, number: 7 };

/// A deployment of two repositories, the first protecting `main`.
fn config() -> Config {
    let mut protected = List::with_capacity(LIMITS.rules.protected);
    protected.push(rules::Branch { repository: 0, name: copy_of(b"main") }).unwrap();
    let repo = plan::Repo { bases: Box::new([copy_of(b"main")]) };
    let charter = plan::Charter {
        instructions: copy_of(b"talk"),
        template: None,
        grants: plan::Grants { modify: false, shell: false, forge: true, subagents: false, note: true },
        budget: BUDGET,
    };
    let wake = plan::Wake {
        on: plan::Sources { own: true, related: true, subscribed: false, messages: true },
        every: None,
        batch: plan::Batch { count: 1, age: None },
    };
    Config {
        plan: plan::Config { repositories: Box::new([repo.clone(), repo]), templates: Box::new([]) },
        home: 0,
        forge: forge::Config {
            engine: ENGINE,
            tracking: copy_of(b"temper"),
            hand_in: copy_of(b"temper:hand-in"),
            projected: Box::new([]),
        },
        rules: Rules {
            repositories: 2,
            protected,
            engine: ENGINE,
            reviewer: Permission::Write,
            plan_steps: 8,
            plan_spend: 1000,
            plan_acceptance: Permission::Write,
            repository_notes: None,
            deployment_notes: Some(Permission::Admin),
            run_spend: 1000,
            goal_spend: 10_000,
            deployment_spend: 100_000,
            acts: Acts {
                open: Permission::Write,
                steer: Permission::Write,
                accept: Permission::Write,
                cancel: Permission::Write,
                release: Permission::Write,
                watch: Permission::Read,
            },
        },
        session: plan::SessionSpec { charter, resume: plan::Resume::Default, wake },
        models: copy_of(b"model"),
        policy: Policy {
            text: Capture::Content,
            progress: Capture::Shape,
            calls: Capture::Shape,
            tools: Capture::Shape,
            usage: Capture::Shape,
        },
        branches: copy_of(b"temper/"),
        saved: copy_of(b"temper-saved/"),
    }
}

const fn env(secs: u64) -> Env<Limits> {
    Env { now: Time::from_nanos(secs.saturating_mul(1_000_000_000)), limits: LIMITS }
}

fn model() -> Model {
    assert!(worst_case(&LIMITS).is_some(), "the limits are bounded");
    Model::new(config(), &LIMITS, 1)
}

/// What one event leads to.
fn stepped(model: &mut Model, env: &Env<Limits>, event: Event) -> List<Request> {
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    step(model, env, event, &mut out);
    drained(&mut out)
}

/// What the loop does between events at `env.now`: fire what is due, and go
/// on with the ready lists, until neither has anything left.
fn idle(model: &mut Model, env: &Env<Limits>) -> List<Request> {
    let mut requests = List::with_capacity(256);
    for _ in 0_u32..64 {
        let mut out = Queue::with_capacity(max_out(&LIMITS));
        if model.is_ready() {
            crate::model::resume(model, env, &mut out);
        } else if model.is_due(env.now) {
            fire(model, env, &mut out);
        } else {
            return requests;
        }
        while let Some(request) = out.pop() {
            requests.push(request).unwrap();
        }
    }
    panic!("the loop settles");
}

fn drained(out: &mut Queue<Request>) -> List<Request> {
    let mut requests = List::with_capacity(out.len());
    while let Some(request) = out.pop() {
        requests.push(request).unwrap();
    }
    requests
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the limits are bounded");
    assert!(bound > 0, "the model holds something");
    let more = worst_case(&Limits { steps: 32, ..LIMITS }).expect("more steps are bounded");
    assert!(more > bound, "more steps hold more");
    assert_eq!(worst_case(&Limits { steps: 1, ..LIMITS }), None, "a step bound leaves room for hand-offs");
    let fewer = forge::Limits { items: 2, ..LIMITS.forge };
    assert_eq!(worst_case(&Limits { forge: fewer, ..LIMITS }), None, "the hub and the forge hold one working set");
    let narrow = fleet::Limits { workstream_bytes: 8, ..LIMITS.fleet };
    assert_eq!(worst_case(&Limits { fleet: narrow, ..LIMITS }), None, "a workstream key fits the fleet");
    assert!(accepts(&config(), &LIMITS), "the configuration fits");
    let elsewhere = Config { home: 2, ..config() };
    assert!(!accepts(&elsewhere, &LIMITS), "the home is one of the deployment's repositories");
}

#[test]
fn a_run_token_packs_its_item() {
    let run = translate::run(Item { repository: 3, number: 41 }).unwrap();
    assert_eq!(translate::item(run), Item { repository: 3, number: 41 });
    assert_eq!(translate::run(Item { repository: 1 << 16, number: 1 }), None);
    assert_eq!(translate::run(Item { repository: 0, number: 1 << 48 }), None);
    let workstream = translate::workstream(Item { repository: 1, number: 2 });
    assert_eq!(&workstream[..], &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2][..]);
}

#[test]
fn numbers_and_branches_are_written_in_decimal() {
    assert_eq!(&translate::decimal(0)[..], b"0");
    assert_eq!(&translate::decimal(1207)[..], b"1207");
    assert_eq!(&translate::decimal(u64::MAX)[..], b"18446744073709551615");
    assert_eq!(&translate::branch(b"temper/", ITEM)[..], b"temper/7");
}

#[test]
fn hold_reasons_keep_their_codes() {
    let codes = [
        translate::hold(plan::Hold::Rejected),
        translate::hold(plan::Hold::Repairs),
        translate::hold(plan::Hold::Rebases),
        translate::hold(plan::Hold::PullClosed),
        translate::hold(plan::Hold::Escalated),
        translate::hold(plan::Hold::Stalled),
    ];
    assert_eq!(codes, [1, 2, 3, 4, 5, 6], "a stored code means the same after a restart");
    assert!(!codes.contains(&translate::NO_STEP), "no plan reason is the top level's own");
}

#[test]
fn a_cold_start_lists_live_work() {
    let mut model = model();
    let requests = idle(&mut model, &env(1));
    let mut listed = false;
    for request in &requests {
        if let Request::Forge { op: api::Op::Items { .. }, payload: None, .. } = request {
            listed = true;
        }
    }
    assert!(listed, "the forge sub-model lists live work: {requests:?}");
}

#[test]
fn a_person_asking_of_an_item_not_held_is_refused() {
    let mut model = model();
    let ask = Ask::Stop { item: ITEM };
    let requests =
        stepped(&mut model, &env(1), Event::Ask { reply_to: ReplyTo::new(Token::new(5)), person: ALICE, ask });
    let [Request::Reply { to, reply }] = requests.as_slice() else { panic!("one reply: {requests:?}") };
    assert_eq!(*reply, Reply::Refused(Refusal::Unknown));
    assert_eq!(to, &ReplyTo::new(Token::new(5)), "the reply answers the call");
}

#[test]
fn a_workers_hello_reaches_the_fleet() {
    let mut model = model();
    let hello = Hello { slots: 2, workstreams: Box::new([]), hosting: Box::new([]) };
    let requests = stepped(&mut model, &env(1), Event::Hello { channel: Token::new(1), hello });
    assert!(requests.is_empty(), "a hello is not answered: {requests:?}");
    assert_eq!(model.fleet().workers(), 1, "the worker is in contact");
}
