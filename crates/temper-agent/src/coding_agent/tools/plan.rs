//! Model-facing plan publication tool and its host callback.

use temper_protocol_agent::{PlanPublication, WorkspaceContext};

/// Guidance appended to the role prompt when the `publish_plan` tool is wired.
pub(crate) const PUBLISH_PLAN_GUIDANCE: &str = "\nPLAN PUBLICATION:\n\
    - You have a `publish_plan` tool. Call it only when you expect two or more \
    separate, non-empty checkpoint commits. The phases are planned checkpoint \
    commit boundaries, not a generic todo list.\n\
    - Pass a concise `summary` (or `title`) and only diff-bearing phase labels. \
    Do not include validation-only steps such as formatting, running tests, or \
    manual verification; report validation in the final summary instead.\n\
    - Use those same phase labels in the final `plan.phases` and later \
    checkpoint labels so host progress can match checklist items. If you expect \
    one checkpoint commit, do not call this tool.\n\
    - The host fills target repository, base branch, and work branch data from \
    the workspace context. Do NOT run git, create branches, open PRs, or call a \
    forge to publish the plan yourself.\n";

/// Orchestration callback the `publish_plan` tool invokes.
///
/// The model supplies only a concise summary/title and ordered phase labels.
/// The tool combines those with target repository/base/work-branch data from the
/// trusted [`WorkspaceContext`] before calling this hook. Implementations decide
/// how to relay or persist the publication; the tool itself performs no git or
/// forge action.
#[async_trait::async_trait]
pub trait PublishPlanHook: Send + Sync {
    async fn publish_plan(&self, publication: PlanPublication) -> Result<(), String>;
}

/// The model-facing `publish_plan` tool.
pub(crate) struct PublishPlanTool {
    pub(crate) context: WorkspaceContext,
    pub(crate) hook: std::sync::Arc<dyn PublishPlanHook>,
}

#[async_trait::async_trait]
impl tongs::tools::Tool for PublishPlanTool {
    fn name(&self) -> &str {
        "publish_plan"
    }

    fn description(&self) -> &str {
        "Publish your checkpoint/commit plan to the host before substantial \
         work, only when you expect two or more separate non-empty checkpoint \
         commits. Provide a short summary/title and ordered diff-bearing phase \
         labels. The host fills target repo, base branch, and work branch data \
         from the workspace context; do not run git, create branches, open PRs, \
         or call a forge to publish the plan yourself."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Short human summary/title for the planned change."
                },
                "title": {
                    "type": "string",
                    "description": "Optional alias for summary/title. Use summary when possible."
                },
                "phases": {
                    "type": "array",
                    "description": "Ordered diff-bearing checkpoint/commit phase labels, concise and reusable as PR checklist lines.",
                    "items": { "type": "string" },
                    "minItems": 1
                }
            },
            "required": ["phases"],
            "anyOf": [
                { "required": ["summary"] },
                { "required": ["title"] }
            ]
        })
    }

    fn effects(&self) -> tongs::tools::ToolEffects {
        // The hook may relay the publication to a host/orchestrator channel.
        // It does not edit the working tree or run a process.
        tongs::tools::ToolEffects {
            reads: false,
            writes: false,
            network: true,
            process: false,
        }
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(tongs::tools::ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<tongs::tools::ToolOutput> {
        let summary = summary_from_input(&input)?;
        let phases = phases_from_input(&input)?;
        let phase_count = phases.len();
        let publication = PlanPublication::from_context(summary, phases, &self.context);
        self.hook
            .publish_plan(publication)
            .await
            .map_err(|error| tongs::Error::tool("publish_plan", error))?;
        Ok(tongs::tools::ToolOutput::text(format!(
            "Published plan with {phase_count} phase(s)."
        )))
    }
}

fn summary_from_input(input: &serde_json::Value) -> tongs::Result<String> {
    let summary = input
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .or_else(|| input.get("title").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            tongs::Error::tool(
                "publish_plan",
                "expected non-empty `summary` (or `title`) string",
            )
        })?;
    Ok(summary.to_string())
}

fn phases_from_input(input: &serde_json::Value) -> tongs::Result<Vec<String>> {
    let phases = input
        .get("phases")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| tongs::Error::tool("publish_plan", "expected `phases` array"))?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|phase| !phase.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if phases.is_empty() {
        return Err(tongs::Error::tool(
            "publish_plan",
            "expected at least one non-empty phase label",
        ));
    }
    Ok(phases)
}
