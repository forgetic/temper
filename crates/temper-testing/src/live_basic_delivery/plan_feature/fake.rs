use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::json;
use temper_workflow::{ArtifactKindId, WorkflowMetadata, render_metadata_block};

use super::{
    FEATURE_TITLE, FIRST_CODE_TITLE, FOLLOWUP_CODE_TITLE, FOLLOWUP_VALIDATION_SUMMARY,
    LANDING_TITLE, PLAN_TITLE, RolePromptEvidence, SECOND_CODE_TITLE, VALIDATION_SUMMARY,
};

pub(super) struct PlanFeatureFake {
    fake: FakeLlm,
    architect_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    engineer_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    tester_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

struct GuidanceContract {
    role: &'static str,
    role_guidance: &'static str,
    prompt_guidance: &'static str,
    tool_guidance: &'static str,
    constraints: &'static [&'static str],
}

const GUIDANCE_CONTRACTS: &[GuidanceContract] = &[
    GuidanceContract {
        role: "architect",
        role_guidance: "Own product feature shaping.",
        prompt_guidance: "should omit target_branch so Temper can stamp the deterministic",
        tool_guidance: "Use this from feature_planning and decompose_plan.",
        constraints: &[
            "Only read the work-item context and repository",
            "Return one declared verdict; Temper applies labels",
        ],
    },
    GuidanceContract {
        role: "engineer",
        role_guidance: "Claim ready code issues",
        prompt_guidance: "implementation PRs must target that feature branch rather than main",
        tool_guidance: "Use this for open_pr on ready code issues",
        constraints: &[
            "Only touch the checked-out repository workspace",
            "Do not create bookkeeping-only diffs",
        ],
    },
    GuidanceContract {
        role: "tester",
        role_guidance: "Validate completed feature plans",
        prompt_guidance: "Use validate_plan only after reading",
        tool_guidance: "Use this from validate_plan.",
        constraints: &[
            "Do not mutate Forge state directly",
            "Tie validation evidence to the current feature branch head",
        ],
    },
];

impl PlanFeatureFake {
    pub(super) fn start() -> Self {
        let architect_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let engineer_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tester_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let architect_seen = std::sync::Arc::clone(&architect_requests);
        let engineer_seen = std::sync::Arc::clone(&engineer_requests);
        let tester_seen = std::sync::Arc::clone(&tester_requests);
        let fake = FakeLlm::start(Script::rule(move |view| {
            if request_role_is(view, "tester") {
                tester_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tester_reply(view)
            } else if request_role_is(view, "engineer") {
                engineer_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                engineer_reply(view)
            } else if request_role_is(view, "architect") {
                architect_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                architect_reply(view)
            } else {
                Reply::text("unexpected plan-centric fake-LLM request")
            }
        }))
        .expect("start plan-centric fake LLM");
        Self {
            fake,
            architect_requests,
            engineer_requests,
            tester_requests,
        }
    }

    pub(super) fn base_url(&self) -> String {
        self.fake.base_url()
    }

    pub(super) fn architect_requests(&self) -> usize {
        self.architect_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn engineer_requests(&self) -> usize {
        self.engineer_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn tester_requests(&self) -> usize {
        self.tester_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) fn prompt_guidance_evidence(&self) -> Result<Vec<RolePromptEvidence>, String> {
        let requests = self.fake.requests();
        GUIDANCE_CONTRACTS
            .iter()
            .map(|contract| {
                let views = requests
                    .iter()
                    .filter_map(|request| request.view.as_ref())
                    .filter(|view| request_role_is(view, contract.role))
                    .collect::<Vec<_>>();
                if views.is_empty() {
                    return Err(format!(
                        "captured fake LLM requests contained no {} prompt",
                        contract.role
                    ));
                }
                for (index, view) in views.iter().enumerate() {
                    let missing = required_guidance(contract)
                        .filter(|required| !messages_contain(view, required))
                        .collect::<Vec<_>>();
                    if !missing.is_empty() {
                        return Err(format!(
                            "captured {} request {} omitted configured guidance: {missing:?}",
                            contract.role,
                            index + 1
                        ));
                    }
                }
                Ok(RolePromptEvidence {
                    role: contract.role.to_string(),
                    request_count: views.len(),
                    role_guidance_excerpt: contract.role_guidance.to_string(),
                    prompt_guidance_excerpt: contract.prompt_guidance.to_string(),
                    tool_guidance_excerpt: contract.tool_guidance.to_string(),
                    constraint_excerpts: contract
                        .constraints
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                })
            })
            .collect()
    }

    pub(super) fn log_tail(&self) -> String {
        let requests = self.fake.requests();
        if requests.is_empty() {
            return "<fake LLM received no requests>".to_string();
        }
        let start = requests.len().saturating_sub(32);
        requests[start..]
            .iter()
            .enumerate()
            .map(|(offset, request)| {
                let index = start + offset + 1;
                let view = request.view.as_ref();
                let role = view.map(role_hint).unwrap_or("unknown");
                let prior = view.map(|v| v.prior_tool_results).unwrap_or_default();
                let guidance = view
                    .and_then(|view| contract_for_role(role).map(|contract| (view, contract)))
                    .map(|(view, contract)| {
                        if required_guidance(contract).all(|value| messages_contain(view, value)) {
                            "complete"
                        } else {
                            "missing"
                        }
                    })
                    .unwrap_or("unknown");
                let last = view
                    .and_then(RequestView::last_message)
                    .map(|m| format!("{}: {}", m.role, snippet(&m.content, 160)))
                    .unwrap_or_else(|| "<no projected message>".to_string());
                format!(
                    "#{index} {} {} role={role} prior_tool_results={prior} configured_guidance={guidance} last={last}",
                    request.method, request.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn architect_reply(view: &RequestView) -> Reply {
    let decomposing = primary_artifact_contains(view, PLAN_TITLE);
    assert_lineage_context(
        view,
        if decomposing {
            PLAN_TITLE
        } else {
            FEATURE_TITLE
        },
    );
    if view.prior_tool_results == 0 {
        return bash_reply("printf plan-centric-architect\\n");
    }
    if decomposing {
        Reply::text(
            json!({
                "verdict": "children_ready",
                "children": [
                    {
                        "slug": "foundation",
                        "kind": "code",
                        "title": FIRST_CODE_TITLE,
                        "body": "Implement the first feature-branch slice by adding the foundation fixture file. Keep the change small and self-contained."
                    },
                    {
                        "slug": "validation-landing",
                        "kind": "code",
                        "title": SECOND_CODE_TITLE,
                        "body": "Implement the validation and landing slice after the foundation slice has landed.",
                        "labels": ["blocked"],
                        "depends_on": ["foundation"]
                    }
                ]
            })
            .to_string(),
        )
    } else {
        let metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("plan")),
            ..WorkflowMetadata::default()
        };
        let body = format!(
            "Plan for {FEATURE_TITLE}.\n\nTemper derives the non-default feature branch from the source feature number; this plan intentionally does not select a branch.\n\nImplementation DAG:\n1. `{FIRST_CODE_TITLE}`.\n2. `{SECOND_CODE_TITLE}` after the first child lands.\n\nValidation: tester confirms both implementation PRs landed, exercises a follow-up, then opens the aggregate landing PR.\n\n{}",
            render_metadata_block(&metadata)
        );
        Reply::text(
            json!({
                "verdict": "needs_plan",
                "children": [
                    {
                        "slug": "plan",
                        "kind": "plan",
                        "title": PLAN_TITLE,
                        "body": body
                    }
                ]
            })
            .to_string(),
        )
    }
}

fn engineer_reply(view: &RequestView) -> Reply {
    let slice = if primary_artifact_contains(view, FOLLOWUP_CODE_TITLE) {
        CodeSlice::Followup
    } else if primary_artifact_contains(view, SECOND_CODE_TITLE) {
        CodeSlice::Second
    } else {
        CodeSlice::First
    };
    assert_lineage_context(view, slice.title());
    match view.prior_tool_results {
        0 => Reply {
            turns: vec![Turn::ToolCall {
                id: format!("call_write_{}", slice.slug()),
                name: "write".to_string(),
                args: json!({
                    "path": slice.path(),
                    "content": slice.content(),
                }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: format!("call_submit_{}", slice.slug()),
                name: "submit_for_pr".to_string(),
                args: json!({ "summary": slice.summary() }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => Reply::text(
            json!({
                "title": slice.title(),
                "body": format!("{} implemented.", slice.summary()),
                "summary": slice.summary()
            })
            .to_string(),
        ),
    }
}

#[derive(Clone, Copy)]
enum CodeSlice {
    First,
    Second,
    Followup,
}

impl CodeSlice {
    fn slug(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Second => "second",
            Self::Followup => "followup",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::First => FIRST_CODE_TITLE,
            Self::Second => SECOND_CODE_TITLE,
            Self::Followup => FOLLOWUP_CODE_TITLE,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::First => "service/FOUNDATION_SLICE.md",
            Self::Second => "service/VALIDATION_LANDING_SLICE.md",
            Self::Followup => "service/VALIDATION_FOLLOWUP.md",
        }
    }

    fn content(self) -> &'static str {
        match self {
            Self::First => "foundation slice\n",
            Self::Second => "validation and landing slice\n",
            Self::Followup => "validation follow-up regression\n",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::First => "Implemented foundation slice.",
            Self::Second => "Implemented validation and landing slice.",
            Self::Followup => "Implemented validation follow-up regression.",
        }
    }
}

fn tester_reply(view: &RequestView) -> Reply {
    assert_lineage_context(view, PLAN_TITLE);
    assert!(
        messages_contain(view, "Validation summaries:"),
        "plan validation turn must receive implementation summary context"
    );
    if view.prior_tool_results == 0 {
        return bash_reply("printf plan-centric-validation\\n");
    }
    if messages_contain(view, FOLLOWUP_CODE_TITLE) {
        Reply::text(
            json!({
                "verdict": "validated",
                "title": LANDING_TITLE,
                "body": "Validation passed for the current derived feature branch head. Both initial implementation PRs and the validation follow-up landed into the feature branch, and the aggregate branch is ready for main.",
                "summary": VALIDATION_SUMMARY
            })
            .to_string(),
        )
    } else {
        Reply::text(
            json!({
                "verdict": "needs_followup",
                "summary": FOLLOWUP_VALIDATION_SUMMARY,
                "children": [{
                    "slug": "validation-followup-regression",
                    "kind": "code",
                    "title": FOLLOWUP_CODE_TITLE,
                    "body": "Add the validation follow-up fixture without selecting a target branch; Temper must inherit the plan feature branch."
                }]
            })
            .to_string(),
        )
    }
}

fn required_guidance(contract: &GuidanceContract) -> impl Iterator<Item = &str> {
    [
        contract.role_guidance,
        contract.prompt_guidance,
        contract.tool_guidance,
    ]
    .into_iter()
    .chain(contract.constraints.iter().copied())
}

fn contract_for_role(role: &str) -> Option<&'static GuidanceContract> {
    GUIDANCE_CONTRACTS
        .iter()
        .find(|contract| contract.role == role)
}

fn bash_reply(command: &str) -> Reply {
    Reply {
        turns: vec![Turn::ToolCall {
            id: "call_probe".to_string(),
            name: "bash".to_string(),
            args: json!({ "command": command }),
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn assert_lineage_context(view: &RequestView, legacy_title: &str) {
    assert!(
        messages_contain(view, "Artifact context bundle (version 1):"),
        "{legacy_title} turn did not receive the artifact context bundle"
    );
    assert!(
        messages_contain(view, "Primary artifact:"),
        "{legacy_title} turn did not receive the primary artifact"
    );
    assert!(
        messages_contain(view, legacy_title),
        "{legacy_title} from the singular work item was not preserved at the agent boundary"
    );
}

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn primary_artifact_contains(view: &RequestView, needle: &str) -> bool {
    view.messages.iter().any(|message| {
        let Some((_, primary_and_rest)) = message.content.split_once("Primary artifact:") else {
            return false;
        };
        let primary = primary_and_rest
            .split_once("\n\nMandatory lineage:")
            .map_or(primary_and_rest, |(primary, _)| primary);
        primary.contains(needle)
    })
}

fn request_role_is(view: &RequestView, role: &str) -> bool {
    let title_case = format!("Role: {role}");
    let upper_case = format!("ROLE: {role}");
    view.messages.iter().any(|message| {
        message.content.contains(&title_case) || message.content.contains(&upper_case)
    })
}

fn role_hint(view: &RequestView) -> &'static str {
    if request_role_is(view, "tester") {
        "tester"
    } else if request_role_is(view, "engineer") {
        "engineer"
    } else if request_role_is(view, "architect") {
        "architect"
    } else {
        "unknown"
    }
}

fn snippet(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max {
            out.push('…');
            break;
        }
        out.push(if ch == '\n' { ' ' } else { ch });
    }
    out
}
