//! Engine host-tool declarations and whole-object decoding. A malformed call
//! is feedback to Smith's LLM, never a root decision (domain/agent.md, 4.3).

use alloc::boxed::Box;
use skein_lib::{Duration, List, Wall, bytes};
use smith_domain_run as run;
use temper_engine_domain::engine;
use jig_core_tasks as tasks;

use crate::{Problem, forge, json::Value, nested};

struct Spec {
    name: &'static [u8],
    description: &'static [u8],
    schema: &'static [u8],
    effect: run::HostEffect,
}

const READ_SCHEMA: &[u8] = br#"{"type":"object","properties":{"repository":{"type":"object"},"read":{"type":"object"}},"required":["repository","read"]}"#;

const SPECS: [Spec; 31] = [
    Spec { name: b"read_items", description: b"Read one page of forge items.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_item", description: b"Read an issue or pull and its comments.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_pull", description: b"Read a pull request.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_pull_for", description: b"Find a pull by head and base.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_reviews", description: b"Read reviews on a pull request.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_statuses", description: b"Read statuses on a commit.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_remarks", description: b"Read review remarks.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_branch", description: b"Read a branch head.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_branches", description: b"List branches.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_pull_files", description: b"Read changed files on a pull.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_compare", description: b"Compare two commits.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_checks", description: b"Read a commit's combined checks.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_job", description: b"Read one bounded CI job log.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_protection", description: b"Read branch protection.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_settings", description: b"Read repository settings.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_collaborators", description: b"Read repository collaborators.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"read_permission", description: b"Read a user's repository permission.", schema: READ_SCHEMA, effect: run::HostEffect::Read },
    Spec { name: b"effect_forge", description: b"Ask the connector for a keyed forge write.", schema: br#"{"type":"object","properties":{"repository":{"type":"object"},"resource":{"type":"object"},"write":{"type":"object"}},"required":["repository","resource","write"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"subscribe_forge", description: b"Hear matching forge news.", schema: br#"{"type":"object","properties":{"topic":{"type":"object"},"own_change":{"type":"integer","minimum":1},"paths":{"type":"array","items":{"type":"string"}}},"required":["topic"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"delegate", description: b"Create an authorized batch of direct child tasks.", schema: br#"{"type":"object","properties":{"batch":{"type":"array","items":{"type":"object"}}},"required":["batch"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"amend", description: b"Amend a live delegate within authority.", schema: br#"{"type":"object","properties":{"target":{"type":"integer","minimum":1},"amendment":{"type":"object"}},"required":["target","amendment"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"message", description: b"Send words, a question or an answer to a referenced task.", schema: br#"{"type":"object","properties":{"target":{"type":"integer","minimum":1},"form":{"type":"string","enum":["words","question","answer"]},"words":{"type":"string"},"question":{"type":"integer","minimum":1}},"required":["target","form","words"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"cancel", description: b"Cancel a live delegate and its subtree.", schema: br#"{"type":"object","properties":{"target":{"type":"integer","minimum":1},"reason":{"type":"string"}},"required":["target","reason"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"release", description: b"Release a held delegate.", schema: br#"{"type":"object","properties":{"target":{"type":"integer","minimum":1}},"required":["target"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"introduce", description: b"Give two referenced tasks reciprocal references.", schema: br#"{"type":"object","properties":{"left":{"type":"integer","minimum":1},"right":{"type":"integer","minimum":1}},"required":["left","right"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"unsubscribe", description: b"Remove a standing subscription.", schema: br#"{"type":"object","properties":{"subscription":{"type":"integer","minimum":1}},"required":["subscription"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"withdraw", description: b"Withdraw one of your pending proposals.", schema: br#"{"type":"object","properties":{"proposal":{"type":"integer","minimum":1}},"required":["proposal"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"decide_escalation", description: b"Release, reject or pass a held descendant.", schema: br#"{"type":"object","properties":{"task":{"type":"integer","minimum":1},"revision":{"type":"integer","minimum":1},"decision":{"type":"string","enum":["release","reject","pass"]},"reason":{"type":"string"}},"required":["task","revision","decision"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"decide", description: b"Accept, reject or pass a pending proposal.", schema: br#"{"type":"object","properties":{"proposer":{"type":"integer","minimum":1},"proposal":{"type":"integer","minimum":1},"decision":{"type":"string","enum":["accept","reject","pass"]},"reason":{"type":"string"}},"required":["proposer","proposal","decision"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"subscribe", description: b"Watch a referenced task or a timer.", schema: br#"{"type":"object","properties":{"kind":{"type":"string","enum":["task","timer"]},"target":{"type":"integer","minimum":1},"held":{"type":"boolean"},"result":{"type":"boolean"},"at":{"type":"integer","minimum":0},"period":{"type":"integer","minimum":1}},"required":["kind"]}"#, effect: run::HostEffect::Write },
    Spec { name: b"propose", description: b"Ask the covering holder to act beyond your authority.", schema: br#"{"type":"object","properties":{"action":{"type":"string","enum":["batch","amend","widen","release"]},"task":{"type":"integer","minimum":1},"batch":{"type":"array","items":{"type":"object"}},"amendment":{"type":"object"},"authority":{"type":"object"},"reason":{"type":"string"},"as_holder":{"type":"boolean"}},"required":["action","reason"]}"#, effect: run::HostEffect::Write },
];

/// Host declarations offered to Smith with exact schemas and write effects.
/// Root authority still decides each accepted call by its durable name.
#[must_use]
pub fn tools(timeout: Duration) -> Box<[run::HostTool]> {
    let mut tools = List::with_capacity(u32::try_from(SPECS.len()).expect("fixed tool list"));
    for spec in SPECS {
        tools
            .push(run::HostTool {
                name: bytes::copy_of(spec.name),
                description: bytes::copy_of(spec.description),
                schema: bytes::copy_of(spec.schema),
                effect: spec.effect,
                timeout,
            })
            .expect("fixed tool list");
    }
    tools.into_boxed()
}

/// Decode an attested Smith host call under one activation-qualified name.
/// The caller supplies its task/attempt envelope separately to the root.
#[expect(clippy::too_many_lines, reason = "one exhaustive finite host-tool registry")]
pub fn call(name: run::CallName, tool: &[u8], input: &run::HostInput) -> Result<engine::Call, Problem> {
    let value = crate::json::parse(input.bytes())?;
    let tool = match tool {
        b"read_items"
        | b"read_item"
        | b"read_pull"
        | b"read_pull_for"
        | b"read_reviews"
        | b"read_statuses"
        | b"read_remarks"
        | b"read_branch"
        | b"read_branches"
        | b"read_pull_files"
        | b"read_compare"
        | b"read_checks"
        | b"read_job"
        | b"read_protection"
        | b"read_settings"
        | b"read_collaborators"
        | b"read_permission" => engine::Tool::ReadForge {
            repository: forge::repository(value.required(b"repository")?)?,
            read: forge::read_named(tool, value.required(b"read")?)?,
        },
        b"effect_forge" => engine::Tool::EffectForge {
            repository: forge::repository(value.required(b"repository")?)?,
            resource: forge::resource(value.required(b"resource")?)?,
            write: Box::new(forge::write(value.required(b"write")?)?),
        },
        b"subscribe_forge" => engine::Tool::SubscribeForge {
            topic: forge::topic(value.required(b"topic")?)?,
            own_change: match value.field(b"own_change")? {
                Some(change) => Some(change.number()?),
                None => None,
            },
            paths: match value.field(b"paths")? {
                Some(paths) => forge::texts(paths)?,
                None => Box::new([]),
            },
        },
        b"delegate" => engine::Tool::Delegate { batch: nested::delegates(value.required(b"batch")?)? },
        b"amend" => engine::Tool::Amend {
            target: positive(&value, b"target")?,
            amendment: nested::amendment(value.required(b"amendment")?)?,
        },
        b"message" => {
            let target = positive(&value, b"target")?;
            let words = value.required(b"words")?.text()?;
            let form = value.required(b"form")?.text()?;
            let form = match form.as_ref() {
                b"words" => engine::MessageForm::Words,
                b"question" => engine::MessageForm::Question,
                b"answer" => engine::MessageForm::Answer { question: positive(&value, b"question")? },
                _ => return Err(Problem::Range),
            };
            engine::Tool::Message { target, form, words }
        }
        b"cancel" => {
            engine::Tool::Cancel { target: positive(&value, b"target")?, reason: value.required(b"reason")?.text()? }
        }
        b"release" => engine::Tool::Release { target: positive(&value, b"target")? },
        b"introduce" => {
            engine::Tool::Introduce { left: positive(&value, b"left")?, right: positive(&value, b"right")? }
        }
        b"unsubscribe" => engine::Tool::Unsubscribe { subscription: positive(&value, b"subscription")? },
        b"withdraw" => engine::Tool::Withdraw { proposal: positive(&value, b"proposal")? },
        b"decide_escalation" => {
            let decision = value.required(b"decision")?.text()?;
            let decision = match decision.as_ref() {
                b"release" => engine::EscalationChoice::Release,
                b"reject" => engine::EscalationChoice::Reject { reason: value.required(b"reason")?.text()? },
                b"pass" => engine::EscalationChoice::Pass,
                _ => return Err(Problem::Range),
            };
            engine::Tool::DecideEscalation {
                task: positive(&value, b"task")?,
                revision: positive(&value, b"revision")?,
                decision,
            }
        }
        b"decide" => {
            let decision = value.required(b"decision")?.text()?;
            let decision = match decision.as_ref() {
                b"accept" => engine::ProposalChoice::Accept,
                b"reject" => engine::ProposalChoice::Reject { reason: value.required(b"reason")?.text()? },
                b"pass" => engine::ProposalChoice::Pass,
                _ => return Err(Problem::Range),
            };
            engine::Tool::Decide {
                proposer: positive(&value, b"proposer")?,
                proposal: positive(&value, b"proposal")?,
                decision,
            }
        }
        b"subscribe" => {
            let kind = value.required(b"kind")?.text()?;
            let kind = match kind.as_ref() {
                b"task" => tasks::SubscriptionKind::Task {
                    target: positive(&value, b"target")?,
                    held: optional_bool(&value, b"held", false)?,
                    result: optional_bool(&value, b"result", true)?,
                },
                b"timer" => tasks::SubscriptionKind::Timer {
                    at: Wall::from_nanos(number(&value, b"at")?),
                    period: match value.field(b"period")? {
                        Some(period) => Some(Duration::from_nanos(period.number()?)),
                        None => None,
                    },
                },
                _ => return Err(Problem::Range),
            };
            engine::Tool::Subscribe { kind }
        }
        b"propose" => {
            let action = value.required(b"action")?.text()?;
            let action = match action.as_ref() {
                b"release" => engine::ProposedAction::Release { task: positive(&value, b"task")? },
                b"batch" => engine::ProposedAction::Batch(nested::delegates(value.required(b"batch")?)?),
                b"amend" => engine::ProposedAction::Amend {
                    task: positive(&value, b"task")?,
                    amendment: nested::amendment(value.required(b"amendment")?)?,
                },
                b"widen" => engine::ProposedAction::Widen {
                    task: positive(&value, b"task")?,
                    authority: nested::authority(value.required(b"authority")?)?,
                },
                _ => return Err(Problem::Range),
            };
            engine::Tool::Propose {
                action,
                reason: value.required(b"reason")?.text()?,
                as_holder: optional_bool(&value, b"as_holder", false)?,
            }
        }
        _ => return Err(Problem::UnknownTool),
    };
    Ok(engine::Call { completion: name.completion, position: name.position, tool })
}

fn number(value: &Value, field: &[u8]) -> Result<u64, Problem> {
    value.required(field)?.number()
}

fn positive(value: &Value, field: &[u8]) -> Result<u64, Problem> {
    let number = number(value, field)?;
    if number == 0 { Err(Problem::Range) } else { Ok(number) }
}

fn optional_bool(value: &Value, field: &[u8], default: bool) -> Result<bool, Problem> {
    match value.field(field)? {
        Some(value) => value.boolean(),
        None => Ok(default),
    }
}
