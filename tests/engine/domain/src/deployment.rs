//! The deployment the world runs: its repositories on the fake forge, its
//! users, its labels, and the engine's configuration and limits.

use skein_lib::{Duration, List};
use temper_engine_domain::brief::{self, Budgets};
use temper_engine_domain::plan::{self, Budget};
use temper_engine_domain::rules::{self, Acts, Permission, Rules};
use temper_engine_domain::views::{self, Capture, Policy};
use temper_engine_domain::work::{self, Retries, Retry};
use temper_engine_domain::{Config, Limits, fleet, forge, notes};

/// The engine's forge user, CI's, and the workers' (the identity a worker
/// pushes with).
pub const ENGINE: u64 = 1;
pub const CI: u64 = 2;
pub const WORKER: u64 = 3;

/// People: forge users, and clients of the engine's web. The first is an
/// admin of every repository, the others may write; `STRANGER` may only
/// read.
pub const PEOPLE: [u64; 4] = [10, 11, 12, 13];
pub const REVIEWER: u64 = 14;
pub const STRANGER: u64 = 19;

/// The deployment's repositories, by their forge names, in its order; and a
/// repository of the forge's that is not the deployment's, where nothing of
/// the engine's may land.
pub const REPOSITORIES: [&[u8]; 2] = [b"acme/one", b"acme/two"];
pub const ELSEWHERE: &[u8] = b"acme/elsewhere";
/// The deployment's repository whose CI never reports.
pub const STALLED: &[u8] = b"acme/two";
pub const MAIN: &[u8] = b"main";

/// The tracking label and the hand-in label; and every label the
/// repositories define.
pub const TRACKING: &[u8] = b"temper";
pub const HAND_IN: &[u8] = b"temper:hand-in";
pub const LABELS: [&[u8]; 4] = [TRACKING, HAND_IN, b"bug", b"feature"];

/// The file CI reads, and what it holds on a commit CI passes.
pub const CUE: &[u8] = b"ci";
pub const GREEN: &[u8] = b"green";

/// The branch an item's change is pushed to is this, then its number.
pub const BRANCHES: &[u8] = b"temper/";

/// The budget of every run, and the most a plan's step may ask.
pub const BUDGET: Budget = Budget { tokens: 1_000, turns: 20, time: Duration::from_secs(600) };

const RETRY: Retry = Retry { retries: 2, base: Duration::from_secs(1), max: Duration::from_secs(8) };

/// The engine's limits in a calm world: room for every story at once.
pub const LIMITS: Limits = Limits {
    work: work::Limits {
        items: 24,
        retries: Retries { transient: RETRY, permanent: RETRY, run: RETRY, agent: RETRY, lost: RETRY, invalid: RETRY },
        undelivered: 2,
        facts: 64,
    },
    plan: plan::Limits {
        templates: 4,
        bases: 4,
        steps: 8,
        name_bytes: 16,
        dependencies: 4,
        gates: 2,
        targets: 2,
        instruction_bytes: 64,
        tasks: 4,
        events: 8,
        repairs: 2,
        rebases: 8,
        rejections: 2,
        stall: Duration::from_secs(1_200),
        budget: BUDGET,
    },
    rules: rules::Limits { repositories: 2, protected: 2, branch_bytes: 16, grants: 4, reviews: 4, gates: 4, lands: 4 },
    forge: forge::Limits {
        repositories: 2,
        items: 24,
        labels: 3,
        members: 4,
        inbox: 8,
        reviewers: 4,
        reads: 8,
        writes: 16,
        calls: 8,
        page: 8,
        name_bytes: 48,
        title_bytes: 64,
        body_bytes: 256,
        rate: 600,
        window: Duration::from_secs(60),
        reserve: 60,
        poll: Duration::from_secs(30),
        hinted: Duration::from_secs(2),
        resolution: Duration::from_secs(1),
        slow: Duration::from_secs(600),
        probes: 4,
        backoff: Duration::from_millis(500),
        backoff_max: Duration::from_secs(10),
        attempts: 5,
        // The protocol layer's deadline, the fake's latest answer and its
        // latest landing after it, and room to spare.
        lifetime: Duration::from_secs(30),
        facts: 128,
    },
    fleet: fleet::Limits {
        workers: 4,
        slots: 4,
        workstreams: 8,
        workstream_bytes: 16,
        attempts: 32,
        calls: 8,
        grace: Duration::from_secs(30),
        facts: 64,
    },
    brief: brief::Limits {
        briefs: 8,
        sections: 10,
        items: 4,
        parts: 4,
        read_bytes: 512,
        budgets: Budgets {
            item: 128,
            comments: 128,
            dependencies: 128,
            ci: 64,
            reviews: 64,
            pull: 64,
            attempts: 64,
            plan: 128,
            notes: 128,
            template: 64,
        },
        brief_bytes: 1_024,
        gather: Duration::from_secs(30),
        facts: 64,
    },
    notes: notes::Limits {
        scopes: 4,
        entries: 8,
        name_bytes: 16,
        description_bytes: 32,
        body_bytes: 64,
        references: 2,
        calls: 4,
        lines: 8,
        recalled: 2,
        facts: 64,
    },
    views: views::Limits {
        runs: 8,
        watchers: 4,
        backlog: 8,
        report_bytes: 64,
        snapshot_bytes: 512,
        records: 8,
        batch_bytes: 512,
        appends: 2,
        flush: Duration::from_secs(1),
        retention: Duration::from_secs(3_600),
        sweep: Duration::from_secs(60),
        facts: 64,
    },
    asks: 8,
    text_bytes: 256,
    models_bytes: 64,
    steps: 32,
    facts: 256,
};

/// What a session carries, opened from the web or handed in.
#[must_use]
pub fn session() -> plan::SessionSpec {
    let charter = plan::Charter {
        instructions: b"converse".as_slice().into(),
        template: None,
        grants: plan::Grants { modify: true, shell: false, forge: true, subagents: false, note: true },
        budget: BUDGET,
    };
    let wake = plan::Wake {
        on: plan::Sources { own: true, related: true, subscribed: false, messages: true },
        every: None,
        batch: plan::Batch { count: 1, age: None },
    };
    plan::SessionSpec { charter, resume: plan::Resume::Default, wake }
}

/// The deployment's configuration: two repositories, each protecting its
/// default branch; the engine's user and labels; the rules every run and
/// write must satisfy.
#[must_use]
pub fn config() -> Config {
    let mut protected = List::with_capacity(LIMITS.rules.protected);
    for repository in 0..2 {
        protected.push(rules::Branch { repository, name: MAIN.into() }).expect("room for each protected branch");
    }
    let repo = plan::Repo { bases: Box::new([MAIN.into()]) };
    Config {
        plan: plan::Config { repositories: Box::new([repo.clone(), repo]), templates: Box::new([]) },
        home: 0,
        forge: forge::Config {
            engine: ENGINE,
            tracking: TRACKING.into(),
            hand_in: HAND_IN.into(),
            projected: Box::new([]),
        },
        rules: Rules {
            repositories: 2,
            protected,
            engine: ENGINE,
            reviewer: Permission::Write,
            plan_steps: 8,
            plan_spend: 100_000,
            plan_acceptance: Permission::Write,
            repository_notes: None,
            deployment_notes: Some(Permission::Admin),
            run_spend: 1_000,
            goal_spend: 100_000,
            deployment_spend: 10_000_000,
            acts: Acts {
                open: Permission::Write,
                steer: Permission::Write,
                accept: Permission::Write,
                cancel: Permission::Write,
                release: Permission::Write,
                watch: Permission::Read,
            },
        },
        session: session(),
        models: b"model".as_slice().into(),
        policy: Policy {
            text: Capture::Content,
            progress: Capture::Shape,
            calls: Capture::Shape,
            tools: Capture::Shape,
            usage: Capture::Shape,
        },
        branches: BRANCHES.into(),
        saved: b"temper-saved/".as_slice().into(),
    }
}

/// The deployment's index for a repository's forge name, if it is one of
/// its own.
#[must_use]
pub fn index(name: &[u8]) -> Option<u32> {
    let at = REPOSITORIES.iter().position(|repository| **repository == *name)?;
    u32::try_from(at).ok()
}

/// A repository's forge name, by the deployment's index for it.
#[must_use]
pub fn name(repository: u32) -> &'static [u8] {
    REPOSITORIES[usize::try_from(repository).expect("few repositories")]
}
