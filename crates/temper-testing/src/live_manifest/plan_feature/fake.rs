use std::path::Path;

use jig_core::{Reply, RequestView, Script, ScriptFile};
use jig_server::FakeLlm;

use super::RolePromptEvidence;

pub(crate) struct PlanFeatureFake {
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
    pub(crate) fn start(script_path: &Path) -> Result<Self, String> {
        let script = ScriptFile::load(script_path)
            .map_err(|error| {
                format!(
                    "load scenario Jig script {}: {error}",
                    script_path.display()
                )
            })?
            .into_script();
        let architect_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let engineer_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tester_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let architect_seen = std::sync::Arc::clone(&architect_requests);
        let engineer_seen = std::sync::Arc::clone(&engineer_requests);
        let tester_seen = std::sync::Arc::clone(&tester_requests);
        let fake = FakeLlm::start(Script::rule(move |view| {
            if request_role_is(view, "tester") {
                tester_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            } else if request_role_is(view, "engineer") {
                engineer_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            } else if request_role_is(view, "architect") {
                architect_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            } else {
                return Reply::text("unexpected role for scenario-owned Jig script");
            }
            script.next_reply(view)
        }))
        .map_err(|error| format!("start scenario Jig fake LLM: {error}"))?;
        Ok(Self {
            fake,
            architect_requests,
            engineer_requests,
            tester_requests,
        })
    }

    pub(crate) fn base_url(&self) -> String {
        self.fake.base_url()
    }

    pub(crate) fn architect_requests(&self) -> usize {
        self.architect_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn engineer_requests(&self) -> usize {
        self.engineer_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn tester_requests(&self) -> usize {
        self.tester_requests
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn prompt_guidance_evidence(&self) -> Result<Vec<RolePromptEvidence>, String> {
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

    pub(crate) fn log_tail(&self) -> String {
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

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
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
