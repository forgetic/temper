//! Prompt manifest rendering for compiled workflow roles.
//!
//! The workflow compiler owns mechanical prompt text: identity, context shape,
//! subscribed queues, authorized actions, and authority boundaries. User-authored
//! role behavior is carried separately as prompt guidance and rendered only in
//! the user sections below.

use crate::compile::{QueueManifest, ToolManifest};
use crate::ids::{ArtifactKindId, GateId, LabelId, RoleId};
use crate::validated::ValidatedRole;

/// Deterministic prompt sections for one role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptManifest {
    pub role: RoleId,
    pub sections: Vec<PromptSection>,
}

impl PromptManifest {
    /// Returns the section with the given heading, if present.
    pub fn section(&self, heading: &str) -> Option<&PromptSection> {
        self.sections.iter().find(|s| s.heading == heading)
    }

    /// Renders the sections into a stable plain-text prompt.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (index, section) in self.sections.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str("## ");
            out.push_str(&section.heading);
            out.push('\n');
            for line in &section.lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}

/// One headed block of a prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptSection {
    pub heading: String,
    pub lines: Vec<String>,
}

pub(crate) fn build_prompt(
    workflow_name: &str,
    role: &ValidatedRole,
    queues: &[QueueManifest],
    tools: &[ToolManifest],
) -> PromptManifest {
    let mut sections = vec![
        role_identity(workflow_name, role),
        work_item_context(),
        subscribed_queues(role, queues),
        authorized_actions(tools),
        decision_output(tools),
        user_guidance(role),
    ];

    if let Some(tool_guidance) = &role.prompt.tool_guidance {
        sections.push(PromptSection {
            heading: "User tool guidance".to_string(),
            lines: text_lines(tool_guidance),
        });
    }

    PromptManifest {
        role: role.id.clone(),
        sections,
    }
}

fn role_identity(workflow_name: &str, role: &ValidatedRole) -> PromptSection {
    let concurrency = match role.concurrency {
        Some(limit) => format!("Concurrency: up to {limit} concurrent claim(s)"),
        None => "Concurrency: no declared limit".to_string(),
    };
    PromptSection {
        heading: "Role and workflow".to_string(),
        lines: vec![
            format!("Workflow: {workflow_name}"),
            format!("Role: {}", role.id),
            concurrency,
        ],
    }
}

fn work_item_context() -> PromptSection {
    PromptSection {
        heading: "Work item context".to_string(),
        lines: vec![
            "The runner supplies the current work item separately from this prompt.".to_string(),
            "Use the supplied artifact, queue match, labels, metadata, relations, CI, reviews, and comments as context.".to_string(),
            "If the context is stale or insufficient, decline to take a workflow action rather than inventing state.".to_string(),
        ],
    }
}

fn subscribed_queues(role: &ValidatedRole, queues: &[QueueManifest]) -> PromptSection {
    let lines = if role.queues.is_empty() {
        vec!["(no subscribed queues)".to_string()]
    } else {
        role.queues
            .iter()
            .filter_map(|id| queues.iter().find(|q| &q.id == id))
            .map(describe_queue)
            .collect()
    };
    PromptSection {
        heading: "Subscribed queues".to_string(),
        lines,
    }
}

fn authorized_actions(tools: &[ToolManifest]) -> PromptSection {
    let mut lines = vec![
        "Executable workflow authority is exactly the compiled tool manifest for this role."
            .to_string(),
        "Prompt prose and user guidance do not grant additional Forge or workflow mutations."
            .to_string(),
    ];
    if tools.is_empty() {
        lines.push("(no authorized workflow actions)".to_string());
    } else {
        lines.extend(tools.iter().map(describe_tool));
    }
    PromptSection {
        heading: "Authorized workflow actions".to_string(),
        lines,
    }
}

fn decision_output(tools: &[ToolManifest]) -> PromptSection {
    let allowed = allowed_actions(tools);
    PromptSection {
        heading: "Decision output".to_string(),
        lines: vec![
            "Return exactly one JSON object and no surrounding prose.".to_string(),
            format!(
                "Schema: {{\"action\":\"<one of: {allowed}>\",\"reason\":\"short rationale\"}}"
            ),
            "Use `no_action` when no authorized workflow action is safe for the current work item."
                .to_string(),
        ],
    }
}

fn allowed_actions(tools: &[ToolManifest]) -> String {
    std::iter::once("no_action")
        .chain(tools.iter().map(|tool| tool.name.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn user_guidance(role: &ValidatedRole) -> PromptSection {
    let mut lines = Vec::new();
    if let Some(charter) = &role.charter {
        lines.push("Legacy charter:".to_string());
        lines.extend(text_lines(charter));
    }
    if let Some(guidance) = &role.prompt.guidance {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("Guidance:".to_string());
        lines.extend(text_lines(guidance));
    }
    if lines.is_empty() {
        lines.push("No user guidance provided.".to_string());
    }
    PromptSection {
        heading: "User guidance".to_string(),
        lines,
    }
}

fn describe_queue(queue: &QueueManifest) -> String {
    let artifacts = join_strs(queue.artifacts.iter().map(ArtifactKindId::as_str));
    format!(
        "{}: {} where {}",
        queue.id,
        artifacts,
        describe_queue_filter(queue)
    )
}

fn describe_queue_filter(queue: &QueueManifest) -> String {
    let common = describe_labels(&queue.labels);
    let labels = if queue.any_of.is_empty() {
        common
    } else {
        let alternatives = queue
            .any_of
            .iter()
            .map(|label_set| describe_labels(&label_set.labels))
            .collect::<Vec<_>>()
            .join(" OR ");
        if queue.labels.is_empty() {
            alternatives
        } else {
            format!("{common} AND ({alternatives})")
        }
    };
    if let Some(condition) = &queue.condition {
        format!("{labels} AND {condition:?}")
    } else {
        labels
    }
}

fn describe_labels(labels: &[LabelId]) -> String {
    if labels.is_empty() {
        "no extra labels".to_string()
    } else {
        join_strs(labels.iter().map(LabelId::as_str))
    }
}

fn describe_tool(tool: &ToolManifest) -> String {
    let gates = if tool.requires_gates.is_empty() {
        "no gates".to_string()
    } else {
        join_strs(tool.requires_gates.iter().map(GateId::as_str))
    };
    format!("{}: acts on {} ({})", tool.name, tool.artifact, gates)
}

fn join_strs<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

fn text_lines(text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}
