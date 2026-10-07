//! A root assignment as one Smith activation. The caller supplies Smith's
//! already decoded history and mounts; no encoding or file access occurs here.

use alloc::boxed::Box;
use jig_charter as shared;
use jig_core_brief as brief;
use jig_core_tasks as tasks;
use skein_lib::{Decimal, List, ReplyTo, Token, bytes};
use smith_domain as smith;
use smith_domain_run as run;
use temper_engine_domain::engine;

/// Turn one root-owned model policy into Smith's model data.
#[must_use]
pub fn model(model: &engine::Model) -> run::charter::Llm {
    run::charter::Llm {
        prices: run::Prices {
            input: model.input_price,
            cached: model.cached_price,
            output: model.output_price,
            unit: model.price_unit,
        },
        dialect: model.dialect,
        account: model.account,
        endpoint: run::charter::Endpoint(model.endpoint),
        model: model.name.clone(),
        max_tokens: model.max_tokens,
    }
}

fn shared_model(model: &engine::Model) -> shared::Model {
    shared::Model {
        prices: shared::Prices {
            input: model.input_price,
            cached: model.cached_price,
            output: model.output_price,
            unit: model.price_unit,
        },
        dialect: model.dialect,
        account: model.account,
        endpoint: model.endpoint,
        name: model.name.clone(),
        max_tokens: model.max_tokens,
    }
}

/// Render one already bounded root brief section with its kind as a title.
#[must_use]
pub fn section(section: &engine::BriefSection) -> run::Section {
    let title: &[u8] = match section.kind {
        engine::BriefKind::Core(brief::Core::Task) => b"Task",
        engine::BriefKind::Core(brief::Core::Lineage) => b"Lineage",
        engine::BriefKind::Core(brief::Core::Inbox) => b"Inbox",
        engine::BriefKind::Core(brief::Core::Results) => b"Dependencies",
        engine::BriefKind::Core(brief::Core::Plan) => b"Plan",
        engine::BriefKind::Core(brief::Core::Attempts) => b"Earlier attempts",
        engine::BriefKind::Core(brief::Core::Calls) => b"Calls",
        engine::BriefKind::Core(brief::Core::Waiting) => b"Waiting",
        engine::BriefKind::Core(brief::Core::NotesIndex) => b"Notes",
        engine::BriefKind::Core(brief::Core::TranscriptTail) => b"Transcript tail",
        engine::BriefKind::Forge(engine::ForgeBriefKind::Ci) => b"CI",
        engine::BriefKind::Forge(engine::ForgeBriefKind::Reviews) => b"Reviews",
        engine::BriefKind::Forge(engine::ForgeBriefKind::Pull) => b"Pull request",
    };
    let text = match &section.body {
        engine::BriefBody::Text(text) => text.clone(),
        engine::BriefBody::Missing(reason) => match reason {
            brief::GatherMissing::Failed => bytes::copy_of(b"[read failed]"),
            brief::GatherMissing::Late => bytes::copy_of(b"[read timed out]"),
            brief::GatherMissing::Budget => bytes::copy_of(b"[read exceeded limit]"),
        },
    };
    run::Section { title: bytes::copy_of(title), text }
}

/// Produce Smith's complete typed start after the root's claim is durable.
/// The caller owns the live reply right, prepared workspace and concrete
/// history; the engine deliberately retains its stored transcript as bytes.
#[must_use]
pub fn start(
    assignment: &engine::Assignment,
    workspace: Option<run::Workspace>,
    transcript: Option<smith::Transcript>,
    reply_to: ReplyTo,
    host_run: Token,
    window: smith::Window,
) -> smith::Event {
    let mut sections = List::with_capacity(assignment.sections.len().try_into().expect("bounded brief sections"));
    for item in &assignment.sections {
        let item = section(item);
        sections.push(shared::Section { title: item.title, text: item.text }).expect("bounded brief sections");
    }
    let policy = &assignment.run.policy;
    let host_tools = crate::tools(policy.call_timeout);
    let mut tools = List::with_capacity(host_tools.len().try_into().expect("bounded host tools"));
    for tool in host_tools {
        tools
            .push(shared::Tool {
                name: tool.name,
                description: tool.description,
                schema: tool.schema,
                effect: match tool.effect {
                    run::HostEffect::Read => shared::ToolEffect::Read,
                    run::HostEffect::Write => shared::ToolEffect::Write,
                },
                timeout: tool.timeout,
            })
            .expect("bounded host tools");
    }
    let mut models = List::with_capacity(policy.alternatives.len().try_into().expect("bounded model list"));
    for candidate in &policy.alternatives {
        models.push(shared_model(candidate)).expect("bounded model list");
    }
    let charter = shared::charter(
        shared::Charter {
            instructions: policy.instructions.clone(),
            tools: tools.into_boxed(),
            wait: true,
            agents: policy.agents,
            workspace: shared::WorkspaceTools { inspect: policy.inspect, modify: policy.modify, shell: policy.shell },
            conventions: Some(shared::Conventions {
                guide: bytes::copy_of(b"AGENTS.md"),
                checks: bytes::copy_of(b".temper/pre-pr"),
            }),
            contract: contract(&assignment.run.contract),
            budget: shared::Budget { turns: policy.turns, spend: assignment.run.budget, time: policy.time },
            model: shared_model(&policy.model),
            models: models.into_boxed(),
            waiting: policy.waiting,
            resumes: policy.resume,
        },
        sections.into_boxed(),
        transcript.is_some(),
    )
    .expect("bounded engine charter translates");
    smith::Event::Start {
        reply_to,
        host_run,
        activation: assignment.attempt,
        window,
        charter,
        workspace,
        transcript,
        // The host will supply recovered answers when it keeps their durable
        // records; this typed system route has none to pass yet.
        answered: Box::new([]),
        grants: Box::new([smith::Grant {
            name: smith::GrantName { account: assignment.grant.account, generation: assignment.grant.generation },
            valid: assignment.grant.valid,
        }]),
    }
}

fn contract(contract: &tasks::Contract) -> shared::Contract {
    match contract {
        tasks::Contract::Report { words } => shared::Contract {
            change: None,
            verdicts: Box::new([]),
            report: Some(shared::TextRule { max: *words, fields: Box::new([]) }),
            failure: Some(shared::TextRule { max: *words, fields: Box::new([]) }),
        },
        tasks::Contract::Verdict { choices } => {
            let mut verdicts = List::with_capacity(choices.len().try_into().expect("bounded verdict list"));
            for choice in choices {
                verdicts
                    .push(shared::VerdictRule {
                        name: bytes::copy_of(Decimal::of(u64::from(choice.code)).as_bytes()),
                        text_max: choice.words,
                        fields: Box::new([]),
                        items: shared::Items { min: 0, max: 0, kinds: Box::new([]) },
                    })
                    .expect("bounded verdict list");
            }
            shared::Contract {
                change: None,
                verdicts: verdicts.into_boxed(),
                report: None,
                failure: Some(shared::TextRule { max: 1024, fields: Box::new([]) }),
            }
        }
        tasks::Contract::Change { words, .. } => {
            let change = shared::ChangeRule {
                checks_must_pass: true,
                fields: Box::new([
                    shared::FieldRule { name: bytes::copy_of(b"title"), max: *words },
                    shared::FieldRule { name: bytes::copy_of(b"body"), max: *words },
                ]),
            };
            shared::Contract {
                change: Some(change),
                verdicts: Box::new([]),
                report: None,
                failure: Some(shared::TextRule { max: *words, fields: Box::new([]) }),
            }
        }
    }
}
