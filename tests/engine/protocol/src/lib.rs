//! Focused production engine channel translations and conversion memory.
use skein_lib::Duration;
use temper_channel::Sizes;
use temper_engine_domain::{self as engine, brief, plan, views};

pub const SIZES: Sizes = Sizes {
    charter: 4096,
    outcome: 8192,
    call: 4096,
    answer: 8192,
    inbound: 128,
    fact: 128,
    snapshot: 128,
    name_bytes: 16,
    token_bytes: 32,
    repositories: 2,
    slots: 2,
    workstreams: 2,
    grants: 2,
    accounts: 2,
    endpoints: 2,
    entries: 4,
    ..Sizes::STARTING
};

#[must_use]
pub fn charter() -> engine::Charter {
    engine::Charter {
        why: plan::Why::Repair(plan::Repair::Conflicts),
        brief: (0..SIZES.entries)
            .map(|_| brief::Section { kind: brief::Kind::Ci, body: brief::Body::Text(b"failed".as_slice().into()) })
            .collect(),
        instructions: Box::new([]),
        grants: permissions(),
        finish: plan::Finish::Change { checks: true },
        budget: budget(),
        models: (0..SIZES.endpoints)
            .map(|endpoint| engine::Model {
                endpoint,
                model: vec![b'm'; SIZES.name_bytes as usize].into(),
                max_tokens: 1024,
            })
            .collect(),
        policy: views::Policy {
            text: views::Capture::Content,
            progress: views::Capture::Shape,
            calls: views::Capture::Nothing,
            tools: views::Capture::Content,
            usage: views::Capture::Shape,
        },
    }
}
fn permissions() -> plan::Grants {
    plan::Grants { modify: true, shell: true, forge: true, subagents: false, note: true }
}
fn budget() -> plan::Budget {
    plan::Budget { tokens: 1000, turns: 10, time: Duration::from_secs(60) }
}
fn plan_charter() -> plan::Charter {
    plan::Charter {
        instructions: b"do it".as_slice().into(),
        template: Some(b"coding".as_slice().into()),
        grants: permissions(),
        budget: budget(),
    }
}

pub fn outcome() -> engine::Outcome {
    let steps = (0..SIZES.entries)
        .map(|number| plan::Step {
            name: format!("step{number}").into_bytes().into(),
            repository: plan::Repository(1),
            work: plan::Work::Change(plan::ChangeSpec {
                base: b"main".as_slice().into(),
                produce: plan_charter(),
                checks: true,
                review: plan::Review::Agent(plan_charter()),
            }),
            after: (0..SIZES.entries).map(|_| Box::from(&b"after"[..])).collect(),
            gates: (0..SIZES.entries).map(|_| plan::Gate::Approvals(2)).collect(),
        })
        .collect();
    let envelope = plan::Envelope {
        agents: 1,
        changes: 4,
        waits: 1,
        sessions: 1,
        repositories: (0..SIZES.repositories).map(plan::Repository).collect(),
        into: (0..SIZES.entries)
            .map(|_| plan::Target { repository: plan::Repository(1), base: b"main".as_slice().into() })
            .collect(),
    };
    engine::Outcome::Plan { plan: plan::Plan { steps, envelope, budget: 1000 }, text: Box::new([]) }
}
