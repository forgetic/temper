#![expect(clippy::disallowed_methods, reason = "ordinary Rust tests mutate golden frames and term lists")]
//! Independent golden bodies and round trips for every schema branch.
use crate::{Sizes, codec, payload, primitives::Encoder, sizes, wire};
use alloc::boxed::Box;
use skein_lib::Reader;
#[test]
fn term_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Term { kind: 7, largest: 7 };
    let mut measure = Encoder::measure();
    wire::put_term(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_term(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 7, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_term(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn open_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Open {
        channel: wire::Channel::Link,
        lowest: 7,
        highest: 7,
        name: Box::from(*b"abc"),
        secret: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    wire::put_open(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_open(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 7, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_open(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn refuse_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Refuse { reason: 7, text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_refuse(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_refuse(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 7, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_refuse(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn grant_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Grant {
        account: 7,
        generation: 7,
        valid: skein_lib::Duration::from_nanos(7),
        token: Box::from(*b"abc"),
        account_id: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    wire::put_grant(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_grant(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_grant(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn workspace_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Workspace {
        key: Box::from(*b"abc"),
        repositories: Box::from([wire::Repository {
            tag: 7,
            name: Box::from(*b"abc"),
            remote: Box::from(*b"abc"),
            start: wire::Start::Base { branch: Box::from(*b"abc") },
            access: wire::Access::ReadOnly,
            identity: 7,
        }]),
    };
    let mut measure = Encoder::measure();
    wire::put_workspace(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_workspace(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0,
            3, 97, 98, 99, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_workspace(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn repository_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Repository {
        tag: 7,
        name: Box::from(*b"abc"),
        remote: Box::from(*b"abc"),
        start: wire::Start::Base { branch: Box::from(*b"abc") },
        access: wire::Access::ReadOnly,
        identity: 7,
    };
    let mut measure = Encoder::measure();
    wire::put_repository(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_repository(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_repository(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn hosting_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Hosting { run: 7, attempt: 7, phase: wire::HostingPhase::Preparing };
    let mut measure = Encoder::measure();
    wire::put_hosting(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_hosting(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_hosting(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn landed_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Landed { tag: 7, commit: [7; 32] };
    let mut measure = Encoder::measure();
    wire::put_landed(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_landed(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_landed(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn work_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Work {
        landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]),
        saved: Some(Box::from([wire::Landing::Explained {
            failure: wire::PushFailure {
                repository: Some(7),
                reason: wire::PushReason::MissingRepository,
                output: Box::from(*b"abc"),
                cut: 7,
            },
        }])),
    };
    let mut measure = Encoder::measure();
    wire::put_work(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_work(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 1, 0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_work(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_failure_0() {
    let sizes = Sizes::STARTING;
    let value = wire::PushFailure {
        repository: Some(7),
        reason: wire::PushReason::MissingRepository,
        output: Box::from(*b"abc"),
        cut: 7,
    };
    let mut measure = Encoder::measure();
    wire::put_push_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn agent_repository_0() {
    let sizes = Sizes::STARTING;
    let value = wire::AgentRepository { name: Box::from(*b"abc"), writable: true };
    let mut measure = Encoder::measure();
    wire::put_agent_repository(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_agent_repository(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 3, 97, 98, 99, 1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_agent_repository(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn endpoint_descriptor_0() {
    let sizes = Sizes::STARTING;
    let value = wire::EndpointDescriptor {
        endpoint: 7,
        provider: wire::Provider::Anthropic,
        host: Box::from(*b"abc"),
        address: wire::Address::V4 { bytes: [7; 4] },
        port: 7,
        path: Box::from(*b"abc"),
        account: 7,
        effort: Box::from(*b"abc"),
        thinking: Some(7),
    };
    let mut measure = Encoder::measure();
    wire::put_endpoint_descriptor(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_endpoint_descriptor(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 7, 7, 7, 7, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0, 0, 3,
            97, 98, 99, 1, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_endpoint_descriptor(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn item_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Item { repository: 7, number: 7 };
    let mut measure = Encoder::measure();
    payload::put_item(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_item(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_item(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Section {
        kind: payload::SectionKind::Item,
        body: payload::SectionBody::Text { text: Box::from(*b"abc") },
    };
    let mut measure = Encoder::measure();
    payload::put_section(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn permissions_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Permissions { modify: true, shell: true, forge: true, subagents: true, note: true };
    let mut measure = Encoder::measure();
    payload::put_permissions(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_permissions(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 1, 1, 1, 1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_permissions(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn budget_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) };
    let mut measure = Encoder::measure();
    payload::put_budget(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_budget(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_budget(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn model_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Model { endpoint: 7, model: Box::from(*b"abc"), max_tokens: 7 };
    let mut measure = Encoder::measure();
    payload::put_model(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_model(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_model(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn policy_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Policy {
        text: payload::Capture::Nothing,
        progress: payload::Capture::Nothing,
        calls: payload::Capture::Nothing,
        tools: payload::Capture::Nothing,
        usage: payload::Capture::Nothing,
    };
    let mut measure = Encoder::measure();
    payload::put_policy(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_policy(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_policy(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn charter_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Charter {
        why: payload::Why::Work,
        brief: Box::from([payload::Section {
            kind: payload::SectionKind::Item,
            body: payload::SectionBody::Text { text: Box::from(*b"abc") },
        }]),
        instructions: Box::from(*b"abc"),
        grants: payload::Permissions { modify: true, shell: true, forge: true, subagents: true, note: true },
        finish: payload::FinishSpec::Report { grows: true },
        budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
        models: Box::from([payload::Model { endpoint: 7, model: Box::from(*b"abc"), max_tokens: 7 }]),
        policy: payload::Policy {
            text: payload::Capture::Nothing,
            progress: payload::Capture::Nothing,
            calls: payload::Capture::Nothing,
            tools: payload::Capture::Nothing,
            usage: payload::Capture::Nothing,
        },
    };
    let mut measure = Encoder::measure();
    payload::put_charter(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_charter(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 1, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0,
            0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0,
            0, 0, 0
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_charter(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn plan_charter_0() {
    let sizes = Sizes::STARTING;
    let value = payload::PlanCharter {
        instructions: Box::from(*b"abc"),
        template: Some(Box::from(*b"abc")),
        grants: payload::Permissions { modify: true, shell: true, forge: true, subagents: true, note: true },
        budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
    };
    let mut measure = Encoder::measure();
    payload::put_plan_charter(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_plan_charter(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0,
            0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_plan_charter(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn target_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Target { repository: 7, base: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_target(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_target(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_target(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn envelope_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Envelope {
        agents: 7,
        changes: 7,
        waits: 7,
        sessions: 7,
        repositories: Box::from([7]),
        into: Box::from([payload::Target { repository: 7, base: Box::from(*b"abc") }]),
    };
    let mut measure = Encoder::measure();
    payload::put_envelope(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_envelope(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 3,
            97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_envelope(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn sources_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Sources { own: true, related: true, subscribed: true, messages: true };
    let mut measure = Encoder::measure();
    payload::put_sources(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_sources(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 1, 1, 1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_sources(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn batch_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Batch { count: 7, age: Some(skein_lib::Duration::from_nanos(7)) };
    let mut measure = Encoder::measure();
    payload::put_batch(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_batch(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 7, 1, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_batch(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn wake_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Wake {
        on: payload::Sources { own: true, related: true, subscribed: true, messages: true },
        every: Some(skein_lib::Duration::from_nanos(7)),
        batch: payload::Batch { count: 7, age: Some(skein_lib::Duration::from_nanos(7)) },
    };
    let mut measure = Encoder::measure();
    payload::put_wake(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_wake(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 1, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_wake(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn step_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Step {
        name: Box::from(*b"abc"),
        repository: 7,
        work: payload::StepWork::Agent {
            charter: payload::PlanCharter {
                instructions: Box::from(*b"abc"),
                template: Some(Box::from(*b"abc")),
                grants: payload::Permissions { modify: true, shell: true, forge: true, subagents: true, note: true },
                budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
            },
            grows: true,
        },
        after: Box::from([Box::from(*b"abc")]),
        gates: Box::from([payload::Gate::Approvals { count: 7 }]),
    };
    let mut measure = Encoder::measure();
    payload::put_step(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_step(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1, 1, 1, 1, 1, 0,
            0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1,
            0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_step(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn plan_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Plan {
        steps: Box::from([payload::Step {
            name: Box::from(*b"abc"),
            repository: 7,
            work: payload::StepWork::Agent {
                charter: payload::PlanCharter {
                    instructions: Box::from(*b"abc"),
                    template: Some(Box::from(*b"abc")),
                    grants: payload::Permissions {
                        modify: true,
                        shell: true,
                        forge: true,
                        subagents: true,
                        note: true,
                    },
                    budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
                },
                grows: true,
            },
            after: Box::from([Box::from(*b"abc")]),
            gates: Box::from([payload::Gate::Approvals { count: 7 }]),
        }]),
        envelope: payload::Envelope {
            agents: 7,
            changes: 7,
            waits: 7,
            sessions: 7,
            repositories: Box::from([7]),
            into: Box::from([payload::Target { repository: 7, base: Box::from(*b"abc") }]),
        },
        budget: 7,
    };
    let mut measure = Encoder::measure();
    payload::put_plan(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_plan(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1, 1,
            1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99,
            0, 0, 0, 1, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0,
            1, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_plan(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn fact_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Fact { kind: payload::FactKind::Text, content: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_fact(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_fact(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_fact(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn snapshot_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Snapshot { version: 7, body: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_snapshot(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_snapshot(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 7, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_snapshot(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn scopes_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Scopes { repository: 7, goal: Some(payload::Item { repository: 7, number: 7 }) };
    let mut measure = Encoder::measure();
    payload::put_scopes(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_scopes(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 7, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_scopes(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn page_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Page {
        description: Box::from(*b"abc"),
        author: payload::Author::Person { person: 7 },
        references: Box::from([payload::Item { repository: 7, number: 7 }]),
        body: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    payload::put_page(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_page(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0,
            3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_page(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn entry_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Entry {
        scope: payload::Scope::Deployment,
        name: Box::from(*b"abc"),
        revision: 7,
        page: payload::Page {
            description: Box::from(*b"abc"),
            author: payload::Author::Person { person: 7 },
            references: Box::from([payload::Item { repository: 7, number: 7 }]),
            body: Box::from(*b"abc"),
        },
    };
    let mut measure = Encoder::measure();
    payload::put_entry(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_entry(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0,
            0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_entry(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn position_0() {
    let sizes = Sizes::STARTING;
    let value =
        payload::Position { comment: 7, pull_comment: 7, reviews: 7, head: Some([7; 32]), ci: payload::Ci::None_ };
    let mut measure = Encoder::measure();
    payload::put_position(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_position(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 0
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_position(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn summary_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Summary {
        number: 7,
        kind: payload::ForgeKind::Issue,
        state: payload::State::Open,
        author: 7,
        key: Some(Box::from(*b"abc")),
        labels: Box::from([Box::from(*b"abc")]),
        title: Box::from(*b"abc"),
        body: Box::from(*b"abc"),
        updated: 7,
    };
    let mut measure = Encoder::measure();
    payload::put_summary(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_summary(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 3,
            97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_summary(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn comment_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Comment {
        id: 7,
        author: 7,
        created: 7,
        revision: 7,
        mark: payload::Mark::None_,
        body: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    payload::put_comment(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_comment(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0,
            3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_comment(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn pull_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Pull {
        number: 7,
        state: payload::State::Open,
        head: Box::from(*b"abc"),
        base: Box::from(*b"abc"),
        commit: [7; 32],
        base_commit: Some([7; 32]),
        merged: Some([7; 32]),
        mergeable: true,
        ci: payload::Ci::None_,
    };
    let mut measure = Encoder::measure();
    payload::put_pull(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_pull(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 0
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_pull(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_review_0() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeReview {
        id: 7,
        author: 7,
        verdict: payload::ForgeVerdict::Approve,
        commit: [7; 32],
        key: Some(Box::from(*b"abc")),
        body: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    payload::put_forge_review(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_review(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_review(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn remark_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Remark { id: 7, author: 7, path: Box::from(*b"abc"), line: 7, body: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_remark(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_remark(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_remark(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn status_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Status {
        context: Box::from(*b"abc"),
        check: payload::Check::Pending,
        description: Box::from(*b"abc"),
        url: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    payload::put_status(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_status(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_status(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_page_0() {
    let sizes = Sizes::STARTING;
    let value =
        payload::ForgePage { name: Box::from(*b"abc"), content: Box::from(*b"abc"), revision: 7, nonce: Some(7) };
    let mut measure = Encoder::measure();
    payload::put_forge_page(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_page(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 0, 0, 0, 0, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_page(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn page_name_0() {
    let sizes = Sizes::STARTING;
    let value = payload::PageName { name: Box::from(*b"abc"), revision: 7 };
    let mut measure = Encoder::measure();
    payload::put_page_name(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_page_name(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_page_name(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn channel_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Channel::Link;
    let mut measure = Encoder::measure();
    wire::put_channel(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_channel(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_channel(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn channel_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Channel::Agent;
    let mut measure = Encoder::measure();
    wire::put_channel(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_channel(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_channel(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn start_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Start::Base { branch: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_start(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_start(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_start(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn start_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Start::Branch { branch: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_start(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_start(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_start(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn start_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Start::Commit { commit: [7; 32] };
    let mut measure = Encoder::measure();
    wire::put_start(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_start(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[2, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_start(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn start_3() {
    let sizes = Sizes::STARTING;
    let value = wire::Start::Saved { branch: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_start(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_start(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_start(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn access_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Access::ReadOnly;
    let mut measure = Encoder::measure();
    wire::put_access(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_access(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_access(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn access_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Access::Writable { push: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_access(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_access(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_access(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn hosting_phase_0() {
    let sizes = Sizes::STARTING;
    let value = wire::HostingPhase::Preparing;
    let mut measure = Encoder::measure();
    wire::put_hosting_phase(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_hosting_phase(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_hosting_phase(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn hosting_phase_1() {
    let sizes = Sizes::STARTING;
    let value = wire::HostingPhase::Starting;
    let mut measure = Encoder::measure();
    wire::put_hosting_phase(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_hosting_phase(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_hosting_phase(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn hosting_phase_2() {
    let sizes = Sizes::STARTING;
    let value = wire::HostingPhase::Active;
    let mut measure = Encoder::measure();
    wire::put_hosting_phase(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_hosting_phase(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_hosting_phase(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn hosting_phase_3() {
    let sizes = Sizes::STARTING;
    let value = wire::HostingPhase::Waiting;
    let mut measure = Encoder::measure();
    wire::put_hosting_phase(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_hosting_phase(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_hosting_phase(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn hosting_phase_4() {
    let sizes = Sizes::STARTING;
    let value = wire::HostingPhase::Ending;
    let mut measure = Encoder::measure();
    wire::put_hosting_phase(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_hosting_phase(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_hosting_phase(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn landing_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Landing::Explained {
        failure: wire::PushFailure {
            repository: Some(7),
            reason: wire::PushReason::MissingRepository,
            output: Box::from(*b"abc"),
            cut: 7,
        },
    };
    let mut measure = Encoder::measure();
    wire::put_landing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_landing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_landing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn landing_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Landing::Landed { commit: [7; 32] };
    let mut measure = Encoder::measure();
    wire::put_landing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_landing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_landing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn landing_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Landing::Moved;
    let mut measure = Encoder::measure();
    wire::put_landing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_landing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_landing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn landing_3() {
    let sizes = Sizes::STARTING;
    let value = wire::Landing::Failed;
    let mut measure = Encoder::measure();
    wire::put_landing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_landing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_landing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn landing_4() {
    let sizes = Sizes::STARTING;
    let value = wire::Landing::Refused;
    let mut measure = Encoder::measure();
    wire::put_landing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_landing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_landing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn landing_5() {
    let sizes = Sizes::STARTING;
    let value = wire::Landing::Unchanged;
    let mut measure = Encoder::measure();
    wire::put_landing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_landing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[5]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_landing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_0() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::MissingRepository;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_1() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::MissingBranch;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_2() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::MissingCommit;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_3() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::Refused;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_4() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::Unreachable;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_5() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::Broken;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[5]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_6() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::TimedOut;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[6]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_7() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::Cancelled;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[7]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_8() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::Unavailable;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[8]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_9() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::Busy;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[9]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_10() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::TooLarge;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[10]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_11() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::Nothing;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[11]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_reason_12() {
    let sizes = Sizes::STARTING;
    let value = wire::PushReason::Unknown;
    let mut measure = Encoder::measure();
    wire::put_push_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[12]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Push::Done;
    let mut measure = Encoder::measure();
    wire::put_push(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Push::Moved;
    let mut measure = Encoder::measure();
    wire::put_push(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Push::Failed {
        failure: wire::PushFailure {
            repository: Some(7),
            reason: wire::PushReason::MissingRepository,
            output: Box::from(*b"abc"),
            cut: 7,
        },
    };
    let mut measure = Encoder::measure();
    wire::put_push(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn push_3() {
    let sizes = Sizes::STARTING;
    let value = wire::Push::Nothing;
    let mut measure = Encoder::measure();
    wire::put_push(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_push(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_push(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn assignment_refusal_0() {
    let sizes = Sizes::STARTING;
    let value = wire::AssignmentRefusal::Busy;
    let mut measure = Encoder::measure();
    wire::put_assignment_refusal(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_assignment_refusal(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_assignment_refusal(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn assignment_refusal_1() {
    let sizes = Sizes::STARTING;
    let value = wire::AssignmentRefusal::Invalid { invalid: wire::Invalid::Repositories };
    let mut measure = Encoder::measure();
    wire::put_assignment_refusal(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_assignment_refusal(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_assignment_refusal(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn invalid_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Invalid::Repositories;
    let mut measure = Encoder::measure();
    wire::put_invalid(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_invalid(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_invalid(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn invalid_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Invalid::Duplicate;
    let mut measure = Encoder::measure();
    wire::put_invalid(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_invalid(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_invalid(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn invalid_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Invalid::Name;
    let mut measure = Encoder::measure();
    wire::put_invalid(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_invalid(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_invalid(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn invalid_3() {
    let sizes = Sizes::STARTING;
    let value = wire::Invalid::Charter;
    let mut measure = Encoder::measure();
    wire::put_invalid(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_invalid(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_invalid(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn invalid_4() {
    let sizes = Sizes::STARTING;
    let value = wire::Invalid::Snapshot;
    let mut measure = Encoder::measure();
    wire::put_invalid(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_invalid(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_invalid(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn failure_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Failure::Unprepared { preparation: wire::Preparation::Transient };
    let mut measure = Encoder::measure();
    wire::put_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn failure_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Failure::Run { failure: wire::RunFailure::Model };
    let mut measure = Encoder::measure();
    wire::put_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn failure_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Failure::Agent { failure: wire::AgentFailure::Unstarted };
    let mut measure = Encoder::measure();
    wire::put_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn failure_3() {
    let sizes = Sizes::STARTING;
    let value = wire::Failure::Cancelled { reason: wire::CancelReason::Engine };
    let mut measure = Encoder::measure();
    wire::put_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn preparation_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Preparation::Transient;
    let mut measure = Encoder::measure();
    wire::put_preparation(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_preparation(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_preparation(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn preparation_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Preparation::Missing { repository: 7, missing: wire::Missing::Repository };
    let mut measure = Encoder::measure();
    wire::put_preparation(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_preparation(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 7, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_preparation(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn preparation_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Preparation::Refused { repository: 7 };
    let mut measure = Encoder::measure();
    wire::put_preparation(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_preparation(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_preparation(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn missing_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Missing::Repository;
    let mut measure = Encoder::measure();
    wire::put_missing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_missing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_missing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn missing_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Missing::Branch;
    let mut measure = Encoder::measure();
    wire::put_missing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_missing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_missing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn missing_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Missing::Commit;
    let mut measure = Encoder::measure();
    wire::put_missing(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_missing(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_missing(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn run_failure_0() {
    let sizes = Sizes::STARTING;
    let value = wire::RunFailure::Model;
    let mut measure = Encoder::measure();
    wire::put_run_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_run_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_run_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn run_failure_1() {
    let sizes = Sizes::STARTING;
    let value = wire::RunFailure::Budget;
    let mut measure = Encoder::measure();
    wire::put_run_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_run_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_run_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn run_failure_2() {
    let sizes = Sizes::STARTING;
    let value = wire::RunFailure::Policy;
    let mut measure = Encoder::measure();
    wire::put_run_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_run_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_run_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn run_failure_3() {
    let sizes = Sizes::STARTING;
    let value = wire::RunFailure::Cancelled;
    let mut measure = Encoder::measure();
    wire::put_run_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_run_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_run_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn run_failure_4() {
    let sizes = Sizes::STARTING;
    let value = wire::RunFailure::Stale;
    let mut measure = Encoder::measure();
    wire::put_run_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_run_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_run_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn agent_failure_0() {
    let sizes = Sizes::STARTING;
    let value = wire::AgentFailure::Unstarted;
    let mut measure = Encoder::measure();
    wire::put_agent_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_agent_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_agent_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn agent_failure_1() {
    let sizes = Sizes::STARTING;
    let value = wire::AgentFailure::Exited;
    let mut measure = Encoder::measure();
    wire::put_agent_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_agent_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_agent_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn agent_failure_2() {
    let sizes = Sizes::STARTING;
    let value = wire::AgentFailure::Rules;
    let mut measure = Encoder::measure();
    wire::put_agent_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_agent_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_agent_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn agent_failure_3() {
    let sizes = Sizes::STARTING;
    let value = wire::AgentFailure::NoProgress;
    let mut measure = Encoder::measure();
    wire::put_agent_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_agent_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_agent_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn agent_failure_4() {
    let sizes = Sizes::STARTING;
    let value = wire::AgentFailure::WallTime;
    let mut measure = Encoder::measure();
    wire::put_agent_failure(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_agent_failure(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_agent_failure(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn cancel_reason_0() {
    let sizes = Sizes::STARTING;
    let value = wire::CancelReason::Engine;
    let mut measure = Encoder::measure();
    wire::put_cancel_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_cancel_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_cancel_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn cancel_reason_1() {
    let sizes = Sizes::STARTING;
    let value = wire::CancelReason::Contact;
    let mut measure = Encoder::measure();
    wire::put_cancel_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_cancel_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_cancel_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn cancel_reason_2() {
    let sizes = Sizes::STARTING;
    let value = wire::CancelReason::Shutdown;
    let mut measure = Encoder::measure();
    wire::put_cancel_reason(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_cancel_reason(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_cancel_reason(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn link_answer_0() {
    let sizes = Sizes::STARTING;
    let value = wire::LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy };
    let mut measure = Encoder::measure();
    wire::put_link_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_link_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_link_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn link_answer_1() {
    let sizes = Sizes::STARTING;
    let value = wire::LinkAnswer::Ended {
        outcome: Box::from(*b"abc"),
        work: wire::Work {
            landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]),
            saved: Some(Box::from([wire::Landing::Explained {
                failure: wire::PushFailure {
                    repository: Some(7),
                    reason: wire::PushReason::MissingRepository,
                    output: Box::from(*b"abc"),
                    cut: 7,
                },
            }])),
        },
    };
    let mut measure = Encoder::measure();
    wire::put_link_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_link_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0,
            0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_link_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn link_answer_2() {
    let sizes = Sizes::STARTING;
    let value = wire::LinkAnswer::Parked {
        snapshot: Some(Box::from(*b"abc")),
        work: wire::Work {
            landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]),
            saved: Some(Box::from([wire::Landing::Explained {
                failure: wire::PushFailure {
                    repository: Some(7),
                    reason: wire::PushReason::MissingRepository,
                    output: Box::from(*b"abc"),
                    cut: 7,
                },
            }])),
        },
    };
    let mut measure = Encoder::measure();
    wire::put_link_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_link_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            2, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0,
            0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_link_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn link_answer_3() {
    let sizes = Sizes::STARTING;
    let value = wire::LinkAnswer::Failed {
        failure: wire::Failure::Unprepared { preparation: wire::Preparation::Transient },
        detail: Box::from(*b"abc"),
        work: wire::Work {
            landed: Box::from([wire::Landed { tag: 7, commit: [7; 32] }]),
            saved: Some(Box::from([wire::Landing::Explained {
                failure: wire::PushFailure {
                    repository: Some(7),
                    reason: wire::PushReason::MissingRepository,
                    output: Box::from(*b"abc"),
                    cut: 7,
                },
            }])),
        },
    };
    let mut measure = Encoder::measure();
    wire::put_link_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_link_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            3, 0, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0,
            0, 0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_link_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn ask_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Ask::Push { message: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_ask(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_ask(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_ask(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn ask_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Ask::Relay { body: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_ask(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_ask(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_ask(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn reply_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Reply::Relayed { answer: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_reply(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_reply(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_reply(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn reply_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Reply::Pushed { push: wire::Push::Done };
    let mut measure = Encoder::measure();
    wire::put_reply(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_reply(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_reply(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn reply_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Reply::Unavailable;
    let mut measure = Encoder::measure();
    wire::put_reply(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_reply(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_reply(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn reply_3() {
    let sizes = Sizes::STARTING;
    let value = wire::Reply::Busy;
    let mut measure = Encoder::measure();
    wire::put_reply(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_reply(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_reply(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn reply_4() {
    let sizes = Sizes::STARTING;
    let value = wire::Reply::Withdrawn;
    let mut measure = Encoder::measure();
    wire::put_reply(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_reply(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_reply(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn reply_5() {
    let sizes = Sizes::STARTING;
    let value = wire::Reply::TooLarge;
    let mut measure = Encoder::measure();
    wire::put_reply(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_reply(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[5]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_reply(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn finish_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Finish::Ended { outcome: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    wire::put_finish(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_finish(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_finish(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn finish_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Finish::Parked { snapshot: Some(Box::from(*b"abc")) };
    let mut measure = Encoder::measure();
    wire::put_finish(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_finish(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 1, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_finish(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn finish_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Finish::Failed { failure: wire::RunFailure::Model };
    let mut measure = Encoder::measure();
    wire::put_finish(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_finish(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_finish(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn bounce_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Bounce::TooLarge;
    let mut measure = Encoder::measure();
    wire::put_bounce(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_bounce(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_bounce(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn bounce_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Bounce::Full;
    let mut measure = Encoder::measure();
    wire::put_bounce(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_bounce(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_bounce(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn bounce_2() {
    let sizes = Sizes::STARTING;
    let value = wire::Bounce::Ending;
    let mut measure = Encoder::measure();
    wire::put_bounce(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_bounce(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_bounce(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn provider_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Provider::Anthropic;
    let mut measure = Encoder::measure();
    wire::put_provider(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_provider(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_provider(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn provider_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Provider::OpenAi;
    let mut measure = Encoder::measure();
    wire::put_provider(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_provider(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_provider(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn address_0() {
    let sizes = Sizes::STARTING;
    let value = wire::Address::V4 { bytes: [7; 4] };
    let mut measure = Encoder::measure();
    wire::put_address(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_address(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 7, 7, 7, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_address(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn address_1() {
    let sizes = Sizes::STARTING;
    let value = wire::Address::V6 { bytes: [7; 16] };
    let mut measure = Encoder::measure();
    wire::put_address(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    wire::put_address(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = wire::get_address(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn why_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Why::Work;
    let mut measure = Encoder::measure();
    payload::put_why(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_why(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_why(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn why_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Why::Produce;
    let mut measure = Encoder::measure();
    payload::put_why(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_why(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_why(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn why_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Why::Repair { repair: payload::Repair::CiFailed };
    let mut measure = Encoder::measure();
    payload::put_why(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_why(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_why(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn why_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Why::Review { head: [7; 32] };
    let mut measure = Encoder::measure();
    payload::put_why(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_why(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[3, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_why(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn why_4() {
    let sizes = Sizes::STARTING;
    let value = payload::Why::Turn;
    let mut measure = Encoder::measure();
    payload::put_why(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_why(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_why(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn repair_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Repair::CiFailed;
    let mut measure = Encoder::measure();
    payload::put_repair(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_repair(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_repair(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn repair_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Repair::ChangesRequested;
    let mut measure = Encoder::measure();
    payload::put_repair(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_repair(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_repair(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn repair_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Repair::BaseMoved;
    let mut measure = Encoder::measure();
    payload::put_repair(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_repair(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_repair(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn repair_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Repair::Conflicts;
    let mut measure = Encoder::measure();
    payload::put_repair(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_repair(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_repair(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_0() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Item;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_1() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Comments;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_2() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Dependencies;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_3() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Ci;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_4() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Reviews;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_5() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Pull;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[5]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_6() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Attempts;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[6]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_7() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Plan;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_8() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Notes;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[8]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_kind_9() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionKind::Template;
    let mut measure = Encoder::measure();
    payload::put_section_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[9]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn unread_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Unread::Failed;
    let mut measure = Encoder::measure();
    payload::put_unread(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_unread(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_unread(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn unread_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Unread::Late;
    let mut measure = Encoder::measure();
    payload::put_unread(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_unread(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_unread(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn unread_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Unread::Oversized;
    let mut measure = Encoder::measure();
    payload::put_unread(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_unread(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_unread(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_body_0() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionBody::Text { text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_section_body(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_body(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_body(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn section_body_1() {
    let sizes = Sizes::STARTING;
    let value = payload::SectionBody::Missing { unread: payload::Unread::Failed };
    let mut measure = Encoder::measure();
    payload::put_section_body(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_section_body(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_section_body(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn finish_spec_0() {
    let sizes = Sizes::STARTING;
    let value = payload::FinishSpec::Report { grows: true };
    let mut measure = Encoder::measure();
    payload::put_finish_spec(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_finish_spec(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_finish_spec(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn finish_spec_1() {
    let sizes = Sizes::STARTING;
    let value = payload::FinishSpec::Change { checks: true };
    let mut measure = Encoder::measure();
    payload::put_finish_spec(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_finish_spec(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_finish_spec(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn finish_spec_2() {
    let sizes = Sizes::STARTING;
    let value = payload::FinishSpec::Verdict;
    let mut measure = Encoder::measure();
    payload::put_finish_spec(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_finish_spec(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_finish_spec(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn finish_spec_3() {
    let sizes = Sizes::STARTING;
    let value = payload::FinishSpec::Turn { supervising: true };
    let mut measure = Encoder::measure();
    payload::put_finish_spec(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_finish_spec(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3, 1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_finish_spec(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn capture_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Capture::Nothing;
    let mut measure = Encoder::measure();
    payload::put_capture(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_capture(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_capture(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn capture_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Capture::Shape;
    let mut measure = Encoder::measure();
    payload::put_capture(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_capture(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_capture(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn capture_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Capture::Content;
    let mut measure = Encoder::measure();
    payload::put_capture(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_capture(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_capture(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn verdict_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Verdict::Approve;
    let mut measure = Encoder::measure();
    payload::put_verdict(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_verdict(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_verdict(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn verdict_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Verdict::Changes;
    let mut measure = Encoder::measure();
    payload::put_verdict(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_verdict(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_verdict(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn gate_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Gate::Approvals { count: 7 };
    let mut measure = Encoder::measure();
    payload::put_gate(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_gate(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_gate(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn gate_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Gate::Accepted;
    let mut measure = Encoder::measure();
    payload::put_gate(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_gate(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_gate(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn resume_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Resume::Default;
    let mut measure = Encoder::measure();
    payload::put_resume(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_resume(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_resume(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn resume_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Resume::Always;
    let mut measure = Encoder::measure();
    payload::put_resume(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_resume(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_resume(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn resume_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Resume::Never;
    let mut measure = Encoder::measure();
    payload::put_resume(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_resume(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_resume(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn review_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Review::Person;
    let mut measure = Encoder::measure();
    payload::put_review(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_review(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_review(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn review_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Review::Agent {
        charter: payload::PlanCharter {
            instructions: Box::from(*b"abc"),
            template: Some(Box::from(*b"abc")),
            grants: payload::Permissions { modify: true, shell: true, forge: true, subagents: true, note: true },
            budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
        },
    };
    let mut measure = Encoder::measure();
    payload::put_review(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_review(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0,
            0, 0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_review(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn wait_spec_0() {
    let sizes = Sizes::STARTING;
    let value = payload::WaitSpec::Steps;
    let mut measure = Encoder::measure();
    payload::put_wait_spec(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_wait_spec(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_wait_spec(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn wait_spec_1() {
    let sizes = Sizes::STARTING;
    let value = payload::WaitSpec::Decision;
    let mut measure = Encoder::measure();
    payload::put_wait_spec(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_wait_spec(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_wait_spec(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn wait_spec_2() {
    let sizes = Sizes::STARTING;
    let value = payload::WaitSpec::Time { duration: skein_lib::Duration::from_nanos(7) };
    let mut measure = Encoder::measure();
    payload::put_wait_spec(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_wait_spec(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_wait_spec(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn step_work_0() {
    let sizes = Sizes::STARTING;
    let value = payload::StepWork::Agent {
        charter: payload::PlanCharter {
            instructions: Box::from(*b"abc"),
            template: Some(Box::from(*b"abc")),
            grants: payload::Permissions { modify: true, shell: true, forge: true, subagents: true, note: true },
            budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
        },
        grows: true,
    };
    let mut measure = Encoder::measure();
    payload::put_step_work(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_step_work(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0,
            0, 0, 0, 0, 0, 0, 7, 1
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_step_work(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn step_work_1() {
    let sizes = Sizes::STARTING;
    let value = payload::StepWork::Change {
        base: Box::from(*b"abc"),
        produce: payload::PlanCharter {
            instructions: Box::from(*b"abc"),
            template: Some(Box::from(*b"abc")),
            grants: payload::Permissions { modify: true, shell: true, forge: true, subagents: true, note: true },
            budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
        },
        checks: true,
        review: payload::Review::Person,
    };
    let mut measure = Encoder::measure();
    payload::put_step_work(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_step_work(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0,
            0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_step_work(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn step_work_2() {
    let sizes = Sizes::STARTING;
    let value = payload::StepWork::Wait { wait: payload::WaitSpec::Steps };
    let mut measure = Encoder::measure();
    payload::put_step_work(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_step_work(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_step_work(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn step_work_3() {
    let sizes = Sizes::STARTING;
    let value = payload::StepWork::Session {
        charter: payload::PlanCharter {
            instructions: Box::from(*b"abc"),
            template: Some(Box::from(*b"abc")),
            grants: payload::Permissions { modify: true, shell: true, forge: true, subagents: true, note: true },
            budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
        },
        resume: payload::Resume::Default,
        wake: payload::Wake {
            on: payload::Sources { own: true, related: true, subscribed: true, messages: true },
            every: Some(skein_lib::Duration::from_nanos(7)),
            batch: payload::Batch { count: 7, age: Some(skein_lib::Duration::from_nanos(7)) },
        },
    };
    let mut measure = Encoder::measure();
    payload::put_step_work(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_step_work(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            3, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0,
            0, 0, 0, 0, 0, 0, 7, 0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 1, 0, 0, 0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_step_work(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Change { message: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Verdict { verdict: payload::Verdict::Approve, text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Report { text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Plan {
        plan: payload::Plan {
            steps: Box::from([payload::Step {
                name: Box::from(*b"abc"),
                repository: 7,
                work: payload::StepWork::Agent {
                    charter: payload::PlanCharter {
                        instructions: Box::from(*b"abc"),
                        template: Some(Box::from(*b"abc")),
                        grants: payload::Permissions {
                            modify: true,
                            shell: true,
                            forge: true,
                            subagents: true,
                            note: true,
                        },
                        budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
                    },
                    grows: true,
                },
                after: Box::from([Box::from(*b"abc")]),
                gates: Box::from([payload::Gate::Approvals { count: 7 }]),
            }]),
            envelope: payload::Envelope {
                agents: 7,
                changes: 7,
                waits: 7,
                sessions: 7,
                repositories: Box::from([7]),
                into: Box::from([payload::Target { repository: 7, base: Box::from(*b"abc") }]),
            },
            budget: 7,
        },
        text: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            3, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1,
            1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98,
            99, 0, 0, 0, 1, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0,
            0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_4() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Steps {
        steps: Box::from([payload::Step {
            name: Box::from(*b"abc"),
            repository: 7,
            work: payload::StepWork::Agent {
                charter: payload::PlanCharter {
                    instructions: Box::from(*b"abc"),
                    template: Some(Box::from(*b"abc")),
                    grants: payload::Permissions {
                        modify: true,
                        shell: true,
                        forge: true,
                        subagents: true,
                        note: true,
                    },
                    budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
                },
                grows: true,
            },
            after: Box::from([Box::from(*b"abc")]),
            gates: Box::from([payload::Gate::Approvals { count: 7 }]),
        }]),
        text: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            4, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1,
            1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98,
            99, 0, 0, 0, 1, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_5() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Tasks {
        tasks: Box::from([payload::Step {
            name: Box::from(*b"abc"),
            repository: 7,
            work: payload::StepWork::Agent {
                charter: payload::PlanCharter {
                    instructions: Box::from(*b"abc"),
                    template: Some(Box::from(*b"abc")),
                    grants: payload::Permissions {
                        modify: true,
                        shell: true,
                        forge: true,
                        subagents: true,
                        note: true,
                    },
                    budget: payload::Budget { tokens: 7, turns: 7, time: skein_lib::Duration::from_nanos(7) },
                },
                grows: true,
            },
            after: Box::from([Box::from(*b"abc")]),
            gates: Box::from([payload::Gate::Approvals { count: 7 }]),
        }]),
        text: Box::from(*b"abc"),
    };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            5, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 1,
            1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98,
            99, 0, 0, 0, 1, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_6() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Reply { text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[6, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_7() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Finished { text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[7, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_8() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Release { step: Box::from(*b"abc"), text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[8, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn outcome_9() {
    let sizes = Sizes::STARTING;
    let value = payload::Outcome::Escalation { text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_outcome(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_outcome(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[9, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_outcome(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn ci_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Ci::None_;
    let mut measure = Encoder::measure();
    payload::put_ci(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_ci(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_ci(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn ci_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Ci::Pending;
    let mut measure = Encoder::measure();
    payload::put_ci(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_ci(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_ci(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn ci_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Ci::Passed;
    let mut measure = Encoder::measure();
    payload::put_ci(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_ci(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_ci(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn ci_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Ci::Failed;
    let mut measure = Encoder::measure();
    payload::put_ci(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_ci(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_ci(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn news_0() {
    let sizes = Sizes::STARTING;
    let value = payload::News::Comment { on: 7, id: 7, author: 7 };
    let mut measure = Encoder::measure();
    payload::put_news(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_news(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_news(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn news_1() {
    let sizes = Sizes::STARTING;
    let value = payload::News::Reviews { commit: [7; 32] };
    let mut measure = Encoder::measure();
    payload::put_news(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_news(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_news(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn news_2() {
    let sizes = Sizes::STARTING;
    let value = payload::News::Pull {
        commit: [7; 32],
        ci: payload::Ci::None_,
        open: true,
        merged: Some([7; 32]),
        mergeable: true,
    };
    let mut measure = Encoder::measure();
    payload::put_news(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_news(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            2, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 0, 1, 1,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_news(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn inbound_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Inbound::News { news: payload::News::Comment { on: 7, id: 7, author: 7 } };
    let mut measure = Encoder::measure();
    payload::put_inbound(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_inbound(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_inbound(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn inbound_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Inbound::Finished { item: payload::Item { repository: 7, number: 7 } };
    let mut measure = Encoder::measure();
    payload::put_inbound(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_inbound(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_inbound(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn inbound_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Inbound::Held { item: payload::Item { repository: 7, number: 7 } };
    let mut measure = Encoder::measure();
    payload::put_inbound(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_inbound(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_inbound(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn inbound_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Inbound::Decided { accepted: true };
    let mut measure = Encoder::measure();
    payload::put_inbound(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_inbound(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3, 1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_inbound(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn fact_kind_0() {
    let sizes = Sizes::STARTING;
    let value = payload::FactKind::Text;
    let mut measure = Encoder::measure();
    payload::put_fact_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_fact_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_fact_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn fact_kind_1() {
    let sizes = Sizes::STARTING;
    let value = payload::FactKind::Progress;
    let mut measure = Encoder::measure();
    payload::put_fact_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_fact_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_fact_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn fact_kind_2() {
    let sizes = Sizes::STARTING;
    let value = payload::FactKind::Call;
    let mut measure = Encoder::measure();
    payload::put_fact_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_fact_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_fact_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn fact_kind_3() {
    let sizes = Sizes::STARTING;
    let value = payload::FactKind::Tool;
    let mut measure = Encoder::measure();
    payload::put_fact_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_fact_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_fact_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn fact_kind_4() {
    let sizes = Sizes::STARTING;
    let value = payload::FactKind::Usage;
    let mut measure = Encoder::measure();
    payload::put_fact_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_fact_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_fact_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Item { item: payload::Item { repository: 7, number: 7 }, after: 7 };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Pull { item: payload::Item { repository: 7, number: 7 } };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Reviews { item: payload::Item { repository: 7, number: 7 }, page: 7 };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Remarks { item: payload::Item { repository: 7, number: 7 }, review: 7, page: 7 };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_4() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::PullFor { repository: 7, head: Box::from(*b"abc"), base: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_5() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Statuses { repository: 7, commit: [7; 32], page: 7 };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            5, 0, 0, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_6() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Permission { repository: 7, user: 7 };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[6, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_7() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Branch { repository: 7, branch: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[7, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_8() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Pages { repository: 7, after: Some(Box::from(*b"abc")) };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[8, 0, 0, 0, 7, 1, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn read_9() {
    let sizes = Sizes::STARTING;
    let value = payload::Read::Page { repository: 7, name: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_read(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_read(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[9, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_read(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn scope_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Scope::Deployment;
    let mut measure = Encoder::measure();
    payload::put_scope(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_scope(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_scope(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn scope_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Scope::Repository { repository: 7 };
    let mut measure = Encoder::measure();
    payload::put_scope(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_scope(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_scope(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn scope_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Scope::Goal { repository: 7, number: 7 };
    let mut measure = Encoder::measure();
    payload::put_scope(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_scope(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_scope(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn author_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Author::Person { person: 7 };
    let mut measure = Encoder::measure();
    payload::put_author(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_author(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_author(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn author_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Author::Run { repository: 7, number: 7 };
    let mut measure = Encoder::measure();
    payload::put_author(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_author(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_author(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn recall_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Recall::Name { scope: payload::Scope::Deployment, name: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_recall(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_recall(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_recall(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn recall_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Recall::Search {
        scopes: payload::Scopes { repository: 7, goal: Some(payload::Item { repository: 7, number: 7 }) },
        query: Box::from(*b"abc"),
        most: 7,
    };
    let mut measure = Encoder::measure();
    payload::put_recall(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_recall(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[1, 0, 0, 0, 7, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_recall(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn change_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Change::New {
        page: payload::Page {
            description: Box::from(*b"abc"),
            author: payload::Author::Person { person: 7 },
            references: Box::from([payload::Item { repository: 7, number: 7 }]),
            body: Box::from(*b"abc"),
        },
    };
    let mut measure = Encoder::measure();
    payload::put_change(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_change(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0,
            0, 3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_change(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn change_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Change::Revise {
        page: payload::Page {
            description: Box::from(*b"abc"),
            author: payload::Author::Person { person: 7 },
            references: Box::from([payload::Item { repository: 7, number: 7 }]),
            body: Box::from(*b"abc"),
        },
        revision: 7,
    };
    let mut measure = Encoder::measure();
    payload::put_change(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_change(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0,
            0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_change(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn change_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Change::Remove;
    let mut measure = Encoder::measure();
    payload::put_change(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_change(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_change(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn call_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Call::Read {
        read: payload::Read::Item { item: payload::Item { repository: 7, number: 7 }, after: 7 },
    };
    let mut measure = Encoder::measure();
    payload::put_call(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_call(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_call(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn call_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Call::Recall {
        recall: payload::Recall::Name { scope: payload::Scope::Deployment, name: Box::from(*b"abc") },
    };
    let mut measure = Encoder::measure();
    payload::put_call(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_call(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_call(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn call_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Call::Note {
        scope: payload::Scope::Deployment,
        name: Box::from(*b"abc"),
        change: payload::Change::New {
            page: payload::Page {
                description: Box::from(*b"abc"),
                author: payload::Author::Person { person: 7 },
                references: Box::from([payload::Item { repository: 7, number: 7 }]),
                body: Box::from(*b"abc"),
            },
        },
    };
    let mut measure = Encoder::measure();
    payload::put_call(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_call(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            2, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7,
            0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_call(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn call_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Call::Comment { text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_call(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_call(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_call(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn call_4() {
    let sizes = Sizes::STARTING;
    let value = payload::Call::Escalate { text: Box::from(*b"abc") };
    let mut measure = Encoder::measure();
    payload::put_call(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_call(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4, 0, 0, 0, 3, 97, 98, 99]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_call(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn noted_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Noted::Done;
    let mut measure = Encoder::measure();
    payload::put_noted(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_noted(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_noted(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn noted_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Noted::Missing;
    let mut measure = Encoder::measure();
    payload::put_noted(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_noted(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_noted(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn noted_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Noted::Exists;
    let mut measure = Encoder::measure();
    payload::put_noted(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_noted(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_noted(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn noted_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Noted::Moved;
    let mut measure = Encoder::measure();
    payload::put_noted(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_noted(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_noted(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn noted_4() {
    let sizes = Sizes::STARTING;
    let value = payload::Noted::Unavailable;
    let mut measure = Encoder::measure();
    payload::put_noted(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_noted(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_noted(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn unserved_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Unserved::Ungranted;
    let mut measure = Encoder::measure();
    payload::put_unserved(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_unserved(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_unserved(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn unserved_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Unserved::Busy;
    let mut measure = Encoder::measure();
    payload::put_unserved(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_unserved(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_unserved(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn unserved_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Unserved::Invalid;
    let mut measure = Encoder::measure();
    payload::put_unserved(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_unserved(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_unserved(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn unserved_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Unserved::Refused;
    let mut measure = Encoder::measure();
    payload::put_unserved(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_unserved(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_unserved(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn unserved_4() {
    let sizes = Sizes::STARTING;
    let value = payload::Unserved::Failed;
    let mut measure = Encoder::measure();
    payload::put_unserved(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_unserved(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_unserved(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_kind_0() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeKind::Issue;
    let mut measure = Encoder::measure();
    payload::put_forge_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_kind_1() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeKind::Pull;
    let mut measure = Encoder::measure();
    payload::put_forge_kind(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_kind(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_kind(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn state_0() {
    let sizes = Sizes::STARTING;
    let value = payload::State::Open;
    let mut measure = Encoder::measure();
    payload::put_state(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_state(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_state(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn state_1() {
    let sizes = Sizes::STARTING;
    let value = payload::State::Closed;
    let mut measure = Encoder::measure();
    payload::put_state(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_state(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_state(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn permission_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Permission::None_;
    let mut measure = Encoder::measure();
    payload::put_permission(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_permission(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_permission(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn permission_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Permission::Read;
    let mut measure = Encoder::measure();
    payload::put_permission(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_permission(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_permission(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn permission_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Permission::Write;
    let mut measure = Encoder::measure();
    payload::put_permission(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_permission(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_permission(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn permission_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Permission::Admin;
    let mut measure = Encoder::measure();
    payload::put_permission(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_permission(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_permission(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn mark_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Mark::None_;
    let mut measure = Encoder::measure();
    payload::put_mark(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_mark(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_mark(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn mark_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Mark::Key { key: Box::from(*b"abc"), person: Some(7) };
    let mut measure = Encoder::measure();
    payload::put_mark(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_mark(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_mark(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn mark_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Mark::Record {
        position: payload::Position {
            comment: 7,
            pull_comment: 7,
            reviews: 7,
            head: Some([7; 32]),
            ci: payload::Ci::None_,
        },
        nonce: 7,
    };
    let mut measure = Encoder::measure();
    payload::put_mark(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_mark(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            2, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 0, 0, 0, 0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_mark(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn mark_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Mark::Mangled;
    let mut measure = Encoder::measure();
    payload::put_mark(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_mark(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_mark(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_verdict_0() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeVerdict::Approve;
    let mut measure = Encoder::measure();
    payload::put_forge_verdict(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_verdict(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_verdict(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_verdict_1() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeVerdict::RequestChanges;
    let mut measure = Encoder::measure();
    payload::put_forge_verdict(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_verdict(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_verdict(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_verdict_2() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeVerdict::Comment;
    let mut measure = Encoder::measure();
    payload::put_forge_verdict(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_verdict(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_verdict(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn check_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Check::Pending;
    let mut measure = Encoder::measure();
    payload::put_check(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_check(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_check(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn check_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Check::Passed;
    let mut measure = Encoder::measure();
    payload::put_check(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_check(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[1]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_check(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn check_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Check::Failed;
    let mut measure = Encoder::measure();
    payload::put_check(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_check(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_check(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_0() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Items {
        items: Box::from([payload::Summary {
            number: 7,
            kind: payload::ForgeKind::Issue,
            state: payload::State::Open,
            author: 7,
            key: Some(Box::from(*b"abc")),
            labels: Box::from([Box::from(*b"abc")]),
            title: Box::from(*b"abc"),
            body: Box::from(*b"abc"),
            updated: 7,
        }]),
        more: true,
        now: 7,
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1,
            0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0,
            0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_1() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Item {
        item: payload::Summary {
            number: 7,
            kind: payload::ForgeKind::Issue,
            state: payload::State::Open,
            author: 7,
            key: Some(Box::from(*b"abc")),
            labels: Box::from([Box::from(*b"abc")]),
            title: Box::from(*b"abc"),
            body: Box::from(*b"abc"),
            updated: 7,
        },
        comments: Box::from([payload::Comment {
            id: 7,
            author: 7,
            created: 7,
            revision: 7,
            mark: payload::Mark::None_,
            body: Box::from(*b"abc"),
        }]),
        more: true,
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 3,
            97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 0,
            0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98,
            99, 1
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_2() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Comment {
        comment: payload::Comment {
            id: 7,
            author: 7,
            created: 7,
            revision: 7,
            mark: payload::Mark::None_,
            body: Box::from(*b"abc"),
        },
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            2, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0,
            0, 3, 97, 98, 99
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_3() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Pull {
        pull: payload::Pull {
            number: 7,
            state: payload::State::Open,
            head: Box::from(*b"abc"),
            base: Box::from(*b"abc"),
            commit: [7; 32],
            base_commit: Some([7; 32]),
            merged: Some([7; 32]),
            mergeable: true,
            ci: payload::Ci::None_,
        },
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            3, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 0
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_4() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Reviews {
        reviews: Box::from([payload::ForgeReview {
            id: 7,
            author: 7,
            verdict: payload::ForgeVerdict::Approve,
            commit: [7; 32],
            key: Some(Box::from(*b"abc")),
            body: Box::from(*b"abc"),
        }]),
        more: true,
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 1
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_5() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Statuses {
        ci: payload::Ci::None_,
        statuses: Box::from([payload::Status {
            context: Box::from(*b"abc"),
            check: payload::Check::Pending,
            description: Box::from(*b"abc"),
            url: Box::from(*b"abc"),
        }]),
        more: true,
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[5, 0, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 1]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_6() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Remarks {
        remarks: Box::from([payload::Remark {
            id: 7,
            author: 7,
            path: Box::from(*b"abc"),
            line: 7,
            body: Box::from(*b"abc"),
        }]),
        more: true,
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            6, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7, 0, 0, 0,
            3, 97, 98, 99, 1
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_7() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Permission { permission: payload::Permission::None_ };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[7, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_8() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Commit { commit: [7; 32] };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[8, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_9() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Pages {
        pages: Box::from([payload::PageName { name: Box::from(*b"abc"), revision: 7 }]),
        next: Some(Box::from(*b"abc")),
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[9, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 3, 97, 98, 99]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_10() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Page {
        page: payload::ForgePage { name: Box::from(*b"abc"), content: Box::from(*b"abc"), revision: 7, nonce: Some(7) },
    };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[10, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 0, 0, 0, 0, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_11() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Created { number: 7 };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[11, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_12() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Commented { id: 7, revision: 7 };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[12, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_13() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Edited { revision: 7 };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[13, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_14() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Reviewed { review: 7 };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[14, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_15() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Merged { commit: [7; 32] };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[15, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_16() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Revision { revision: 7 };
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[16, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn forge_answer_17() {
    let sizes = Sizes::STARTING;
    let value = payload::ForgeAnswer::Done;
    let mut measure = Encoder::measure();
    payload::put_forge_answer(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_forge_answer(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[17]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_forge_answer(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn served_0() {
    let sizes = Sizes::STARTING;
    let value = payload::Served::Read {
        answer: payload::ForgeAnswer::Items {
            items: Box::from([payload::Summary {
                number: 7,
                kind: payload::ForgeKind::Issue,
                state: payload::State::Open,
                author: 7,
                key: Some(Box::from(*b"abc")),
                labels: Box::from([Box::from(*b"abc")]),
                title: Box::from(*b"abc"),
                body: Box::from(*b"abc"),
                updated: 7,
            }]),
            more: true,
            now: 7,
        },
    };
    let mut measure = Encoder::measure();
    payload::put_served(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_served(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0,
            1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0,
            0, 0, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_served(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn served_1() {
    let sizes = Sizes::STARTING;
    let value = payload::Served::Recalled {
        entries: Box::from([payload::Entry {
            scope: payload::Scope::Deployment,
            name: Box::from(*b"abc"),
            revision: 7,
            page: payload::Page {
                description: Box::from(*b"abc"),
                author: payload::Author::Person { person: 7 },
                references: Box::from([payload::Item { repository: 7, number: 7 }]),
                body: Box::from(*b"abc"),
            },
        }]),
        failed: 7,
    };
    let mut measure = Encoder::measure();
    payload::put_served(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_served(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 0, 0, 0, 1, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 0, 0,
            0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 7
        ]
    );
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_served(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn served_2() {
    let sizes = Sizes::STARTING;
    let value = payload::Served::Noted { noted: payload::Noted::Done };
    let mut measure = Encoder::measure();
    payload::put_served(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_served(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[2, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_served(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn served_3() {
    let sizes = Sizes::STARTING;
    let value = payload::Served::Posted { comment: 7 };
    let mut measure = Encoder::measure();
    payload::put_served(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_served(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[3, 0, 0, 0, 0, 0, 0, 0, 7]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_served(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn served_4() {
    let sizes = Sizes::STARTING;
    let value = payload::Served::Unserved { reason: payload::Unserved::Ungranted };
    let mut measure = Encoder::measure();
    payload::put_served(&mut measure, &value, &sizes).unwrap();
    let mut out = Encoder::writing(measure.length());
    payload::put_served(&mut out, &value, &sizes).unwrap();
    let bytes = out.finish();
    assert_eq!(bytes.as_ref(), &[4, 0]);
    let mut input = Reader::new(&bytes);
    let decoded = payload::get_served(&mut input, &sizes).unwrap();
    assert_eq!(decoded, value);
    assert!(input.is_empty());
}
#[test]
fn message_open() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Open {
        open: wire::Open {
            channel: wire::Channel::Link,
            lowest: 7,
            highest: 7,
            name: Box::from(*b"abc"),
            secret: Box::from(*b"abc"),
        },
    };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[0, 1, 0, 0, 0, 0, 0, 23, 116, 109, 112, 114, 1, 0, 7, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(1, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_accept() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Accept { version: 7 };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[0, 2, 0, 0, 0, 0, 0, 2, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(2, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_refuse() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Refuse { refuse: wire::Refuse { reason: 7, text: Box::from(*b"abc") } };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[0, 3, 0, 0, 0, 0, 0, 9, 0, 7, 0, 0, 0, 3, 97, 98, 99]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(3, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_ping() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Ping;
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[0, 4, 0, 0, 0, 0, 0, 0]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(4, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_terms() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Terms { terms: Box::from([wire::Term { kind: 7, largest: 7 }]) };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[0, 16, 0, 0, 0, 0, 0, 10, 0, 0, 0, 1, 0, 7, 0, 0, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(16, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_hello() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Hello {
        slots: 7,
        workstreams: Box::from([Box::from(*b"abc")]),
        hosting: Box::from([wire::Hosting { run: 7, attempt: 7, phase: wire::HostingPhase::Preparing }]),
    };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 1, 0, 0, 0, 0, 0, 36, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
            7, 0, 0, 0, 0, 0, 0, 0, 7, 0
        ]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(257, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_answer() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Answer {
        run: 7,
        attempt: 7,
        answer: wire::LinkAnswer::Refused { refusal: wire::AssignmentRefusal::Busy },
    };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[1, 2, 0, 0, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(258, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_relay() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Relay { run: 7, attempt: 7, call: 7, body: Box::from(*b"abc") };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 3, 0, 0, 0, 0, 0, 31, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0,
            3, 97, 98, 99
        ]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(259, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_bounced() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Bounced { run: 7, attempt: 7, event: 7, bounce: wire::Bounce::TooLarge };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[1, 4, 0, 0, 0, 0, 0, 25, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(260, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_told() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Told { run: 7, attempt: 7, fact: Box::from(*b"abc") };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[1, 5, 0, 0, 0, 0, 0, 23, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(261, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_rejected() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Rejected { run: 7, attempt: 7, account: 7, generation: 7 };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[1, 6, 0, 0, 0, 0, 0, 28, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(262, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_exhausted() {
    let sizes = Sizes::STARTING;
    let value =
        wire::Message::Exhausted { run: 7, attempt: 7, account: 7, retry_after: skein_lib::Duration::from_nanos(7) };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[1, 7, 0, 0, 0, 0, 0, 28, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(263, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_assign() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Assign {
        run: 7,
        attempt: 7,
        workspace: wire::Workspace {
            key: Box::from(*b"abc"),
            repositories: Box::from([wire::Repository {
                tag: 7,
                name: Box::from(*b"abc"),
                remote: Box::from(*b"abc"),
                start: wire::Start::Base { branch: Box::from(*b"abc") },
                access: wire::Access::ReadOnly,
                identity: 7,
            }]),
        },
        save: Some(Box::from(*b"abc")),
        charter: Box::from(*b"abc"),
        snapshot: Some(Box::from(*b"abc")),
        grants: Box::from([wire::Grant {
            account: 7,
            generation: 7,
            valid: skein_lib::Duration::from_nanos(7),
            token: Box::from(*b"abc"),
            account_id: Box::from(*b"abc"),
        }]),
    };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 129, 0, 0, 0, 0, 0, 119, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0,
            0, 1, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 0, 7,
            1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0,
            0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(385, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_inbound() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Inbound { run: 7, attempt: 7, event: 7, body: Box::from(*b"abc") };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 130, 0, 0, 0, 0, 0, 31, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0,
            3, 97, 98, 99
        ]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(386, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_cancel() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Cancel { run: 7, attempt: 7 };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[1, 131, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(387, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_relayed() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Relayed { run: 7, attempt: 7, call: 7, answer: Box::from(*b"abc") };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 132, 0, 0, 0, 0, 0, 31, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0,
            3, 97, 98, 99
        ]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(388, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_acknowledge() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Acknowledge { run: 7, attempt: 7 };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[1, 133, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(389, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_grant() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Grant {
        run: 7,
        attempt: 7,
        grant: wire::Grant {
            account: 7,
            generation: 7,
            valid: skein_lib::Duration::from_nanos(7),
            token: Box::from(*b"abc"),
            account_id: Box::from(*b"abc"),
        },
    };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[
            1, 134, 0, 0, 0, 0, 0, 50, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0,
            7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(390, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_agent_call() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentCall { call: 7, ask: wire::Ask::Push { message: Box::from(*b"abc") } };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 1, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(513, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_withdraw() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Withdraw { call: 7 };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 2, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(514, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_fact() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Fact { fact: Box::from(*b"abc") };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 3, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(515, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_long() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Long { span: skein_lib::Duration::from_nanos(7) };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 4, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(516, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_long_done() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::LongDone;
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 5, 0, 0, 0, 0, 0, 0]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(517, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_waiting() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Waiting { heard: 7 };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 6, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(518, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_finish() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::Finish { finish: wire::Finish::Ended { outcome: Box::from(*b"abc") } };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 7, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 3, 97, 98, 99]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(519, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_agent_rejected() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentRejected { account: 7, generation: 7 };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 8, 0, 0, 0, 0, 0, 12, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(520, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_agent_exhausted() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentExhausted { account: 7, retry_after: skein_lib::Duration::from_nanos(7) };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 9, 0, 0, 0, 0, 0, 12, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(521, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_agent_start() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentStart {
        charter: Box::from(*b"abc"),
        snapshot: Some(Box::from(*b"abc")),
        repositories: Box::from([wire::AgentRepository { name: Box::from(*b"abc"), writable: true }]),
        endpoints: Box::from([wire::EndpointDescriptor {
            endpoint: 7,
            provider: wire::Provider::Anthropic,
            host: Box::from(*b"abc"),
            address: wire::Address::V4 { bytes: [7; 4] },
            port: 7,
            path: Box::from(*b"abc"),
            account: 7,
            effort: Box::from(*b"abc"),
            thinking: Some(7),
        }]),
        grants: Box::from([wire::Grant {
            account: 7,
            generation: 7,
            valid: skein_lib::Duration::from_nanos(7),
            token: Box::from(*b"abc"),
            account_id: Box::from(*b"abc"),
        }]),
    };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[
            2, 129, 0, 0, 0, 0, 0, 111, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 1, 0, 0, 0, 3, 97,
            98, 99, 1, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99, 0, 7, 7, 7, 7, 0, 7, 0, 0, 0, 3, 97, 98, 99,
            0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 1, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0,
            0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(641, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_agent_event() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentEvent { event: 7, body: Box::from(*b"abc") };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 130, 0, 0, 0, 0, 0, 15, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98, 99]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(642, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_agent_answer() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentAnswer { call: 7, reply: wire::Reply::Relayed { answer: Box::from(*b"abc") } };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 131, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 3, 97, 98, 99]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(643, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_agent_cancel() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentCancel;
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(bytes.as_ref(), &[2, 132, 0, 0, 0, 0, 0, 0]);
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(644, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}
#[test]
fn message_agent_grant() {
    let sizes = Sizes::STARTING;
    let value = wire::Message::AgentGrant {
        grant: wire::Grant {
            account: 7,
            generation: 7,
            valid: skein_lib::Duration::from_nanos(7),
            token: Box::from(*b"abc"),
            account_id: Box::from(*b"abc"),
        },
    };
    let bytes = codec::encode(&value, &sizes).unwrap();
    assert_eq!(
        bytes.as_ref(),
        &[
            2, 133, 0, 0, 0, 0, 0, 34, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 3, 97, 98,
            99, 0, 0, 0, 3, 97, 98, 99
        ]
    );
    assert_eq!(codec::decode(&bytes, &sizes), Some(value));
    assert!(u32::try_from(bytes.len()).unwrap() <= sizes::frame(645, &sizes).unwrap());
    for cut in 0..bytes.len() {
        assert!(codec::decode(&bytes[..cut], &sizes).is_none());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(codec::decode(&trailing, &sizes).is_none());
}

#[test]
fn malformed_headers_and_lengths() {
    let sizes = Sizes::STARTING;
    for bytes in [[0, 4, 0, 1, 0, 0, 0, 0], [0xff, 0xff, 0, 0, 0, 0, 0, 0], [0, 2, 0, 0, 0, 0, 0, 3]] {
        assert!(codec::header(&bytes, &sizes).is_none());
    }
    // Terms count announces more entries than remain; no list is allocated.
    assert!(codec::body(16, &[0, 0, 0, 32], &sizes).is_none());
    // The byte length overflows what remains, and unknown option/enum/bool tags fail.
    assert!(payload::decode_fact(&[0, 255, 255, 255, 255], &sizes).is_none());
    assert!(payload::decode_inbound(&[3, 2], &sizes).is_none());
    assert!(codec::body(0x207, &[1, 2], &sizes).is_none());
    assert!(payload::decode_outcome(&[255], &sizes).is_none());
}
#[test]
fn terms_must_be_complete_unique_and_large_enough() {
    for endpoint in [
        crate::machine::Endpoint::Engine,
        crate::machine::Endpoint::WorkerLink,
        crate::machine::Endpoint::WorkerAgent,
        crate::machine::Endpoint::Agent,
    ] {
        let sizes = Sizes::STARTING;
        let peer = match endpoint {
            crate::machine::Endpoint::Engine => crate::machine::Endpoint::WorkerLink,
            crate::machine::Endpoint::WorkerLink => crate::machine::Endpoint::Engine,
            crate::machine::Endpoint::WorkerAgent => crate::machine::Endpoint::Agent,
            crate::machine::Endpoint::Agent => crate::machine::Endpoint::WorkerAgent,
        };
        let terms = sizes::terms(peer, &sizes).unwrap();
        assert!(sizes::check_terms(endpoint, &terms, &sizes).is_some());
        assert!(sizes::check_terms(endpoint, &terms[1..], &sizes).is_none());
        let mut wrong = terms.to_vec();
        wrong[0].largest = 0;
        assert!(sizes::check_terms(endpoint, &wrong, &sizes).is_none());
        wrong[0] = terms[0].clone();
        wrong.push(terms[0].clone());
        assert!(sizes::check_terms(endpoint, &wrong, &sizes).is_none());
    }
}
#[test]
fn bounds_are_checked_and_hand_computed() {
    let sizes = Sizes::STARTING;
    assert_eq!(sizes::largest(1, &sizes), Some(256));
    assert_eq!(sizes::largest(2, &sizes), Some(2));
    assert_eq!(sizes::largest(3, &sizes), Some(512));
    assert_eq!(sizes::largest(4, &sizes), Some(0));
    assert_eq!(sizes::largest(0x104, &sizes), Some(25));
    assert_eq!(sizes::largest(0x186, &sizes), Some(16 + 4 + 8 + 8 + 4 + sizes.token_bytes + 4 + sizes.token_bytes));
    let huge = Sizes { repositories: u32::MAX, ..sizes };
    assert!(sizes::largest(0x181, &huge).is_none());
    assert!(sizes::output_cap(crate::machine::Endpoint::Engine, &huge).is_none());
    assert_eq!(wire::RefusalReason::from_code(99), wire::RefusalReason::Other);
}

#[test]
fn protocol_deadlines_are_nonzero() {
    use crate::Limits;
    let zero = skein_lib::Duration::ZERO;
    for limits in [
        Limits { handshake: zero, ..Limits::STARTING },
        Limits { hello: zero, ..Limits::STARTING },
        Limits { ping: zero, ..Limits::STARTING },
        Limits { silence: zero, ..Limits::STARTING },
        Limits { stall: zero, ..Limits::STARTING },
    ] {
        assert!(!limits.valid());
    }
}

#[test]
fn exhausted_finish_keeps_the_appended_wire_tag() {
    let sizes = Sizes::STARTING;
    let message = wire::Message::Finish { finish: wire::Finish::Failed { failure: wire::RunFailure::Exhausted } };
    let frame = codec::encode(&message, &sizes).unwrap();
    assert_eq!(&*frame, &[0x02, 0x07, 0, 0, 0, 0, 0, 2, 2, 5]);
    assert_eq!(codec::decode(&frame, &sizes), Some(message));
}
