//! A root assignment as one Smith activation. The caller supplies Smith's
//! already decoded history and mounts; no encoding or file access occurs here.

use alloc::boxed::Box;
use skein_lib::{Decimal, List, ReplyTo, Token, bytes};
use smith_domain as smith;
use smith_domain_run as run;
use temper_engine_domain::engine;
use temper_engine_domain_brief as brief;
use temper_engine_domain_tasks as tasks;

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

/// Render one already bounded root brief section with its kind as a title.
#[must_use]
pub fn section(section: &brief::Section) -> run::Section {
    let title: &[u8] = match section.kind {
        brief::Kind::Task => b"Task",
        brief::Kind::Transcript => b"Transcript tail",
        brief::Kind::Item => b"Item",
        brief::Kind::Comments => b"Comments",
        brief::Kind::Dependencies => b"Dependencies",
        brief::Kind::Ci => b"CI",
        brief::Kind::Reviews => b"Reviews",
        brief::Kind::Pull => b"Pull request",
        brief::Kind::Attempts => b"Earlier attempts",
        brief::Kind::Plan => b"Plan",
        brief::Kind::Notes => b"Notes",
        brief::Kind::Template => b"Template",
    };
    let text = match &section.body {
        brief::Body::Text(text) => text.clone(),
        brief::Body::Missing(reason) => match reason {
            brief::Unread::Failed => bytes::copy_of(b"[read failed]"),
            brief::Unread::Late => bytes::copy_of(b"[read timed out]"),
            brief::Unread::Oversized => bytes::copy_of(b"[read exceeded limit]"),
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
) -> smith::Event {
    let mut sections = List::with_capacity(assignment.sections.len().try_into().expect("bounded brief sections"));
    for item in &assignment.sections {
        sections.push(section(item)).expect("bounded brief sections");
    }
    let outcome = outcome(&assignment.run.contract);
    let policy = &assignment.run.policy;
    let grants = run::charter::Grants {
        wait: true,
        deliver: outcome.change.clone(),
        tools: run::charter::Tools { inspect: policy.inspect, modify: policy.modify, shell: policy.shell },
        agents: policy.agents,
        host_tools: crate::tools(policy.call_timeout),
    };
    let mut models = List::with_capacity(policy.alternatives.len().try_into().expect("bounded model list"));
    for candidate in &policy.alternatives {
        models.push(model(candidate)).expect("bounded model list");
    }
    smith::Event::Start {
        reply_to,
        host_run,
        activation: assignment.attempt,
        charter: run::Charter {
            resume: policy.resume && transcript.is_some(),
            waiting: policy.waiting,
            instructions: policy.instructions.clone(),
            brief: run::Brief { sections: sections.into_boxed() },
            conventions: Some(run::Conventions {
                guide: bytes::copy_of(b"AGENTS.md"),
                checks: bytes::copy_of(b".temper/pre-pr"),
            }),
            grants,
            outcome,
            budget: run::Budget { turns: policy.turns, spend: assignment.run.budget, time: policy.time },
            llm: model(&policy.model),
            models: models.into_boxed(),
        },
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

fn outcome(contract: &tasks::Contract) -> run::outcome::OutcomeSpec {
    match contract {
        tasks::Contract::Report { words } => run::outcome::OutcomeSpec {
            change: None,
            verdicts: Box::new([]),
            report: Some(run::outcome::TextSpec { max: *words, fields: Box::new([]) }),
            failure: Some(run::outcome::TextSpec { max: *words, fields: Box::new([]) }),
        },
        tasks::Contract::Verdict { choices } => {
            let mut verdicts = List::with_capacity(choices.len().try_into().expect("bounded verdict list"));
            for choice in choices {
                verdicts
                    .push(run::outcome::VerdictRule {
                        name: bytes::copy_of(Decimal::of(u64::from(choice.code)).as_bytes()),
                        text_max: choice.words,
                        fields: Box::new([]),
                        items: run::outcome::ItemSpec { min: 0, max: 0, kinds: Box::new([]) },
                    })
                    .expect("bounded verdict list");
            }
            run::outcome::OutcomeSpec {
                change: None,
                verdicts: verdicts.into_boxed(),
                report: None,
                failure: Some(run::outcome::TextSpec { max: 1024, fields: Box::new([]) }),
            }
        }
        tasks::Contract::Change { words, .. } => {
            let change = run::outcome::ChangeSpec {
                checks_must_pass: true,
                fields: Box::new([
                    run::outcome::FieldRule { name: bytes::copy_of(b"title"), max: *words },
                    run::outcome::FieldRule { name: bytes::copy_of(b"body"), max: *words },
                ]),
            };
            run::outcome::OutcomeSpec {
                change: Some(change),
                verdicts: Box::new([]),
                report: None,
                failure: Some(run::outcome::TextSpec { max: *words, fields: Box::new([]) }),
            }
        }
    }
}
