use crate::codebase_memory::{
    CodebaseMemoryToolMetadata, CodebaseMemoryToolset,
    build_codebase_memory_toolset_with_timeout_and_containment,
};
use std::path::Path;
use std::time::Duration;

use temper_agent_core::{AgentContainmentContext, LineageAdmissionHandle};
use temper_protocol_agent::{AgentToolConfig, WorkspaceContext};
use tongs::tools::ToolRegistry;

use super::CodingAgentError;

pub(super) struct PreparedCodebaseMemoryTools {
    prompt_section: Option<String>,
    pub(super) toolset: CodebaseMemoryToolset,
}

pub(super) struct PreparedCodebaseMemoryGuidance {
    prompt_section: Option<String>,
    registered_safe_names: Vec<String>,
    lineage_admission: Option<LineageAdmissionHandle>,
    pub(super) coverage: Option<std::sync::Arc<crate::codebase_memory::CoverageService>>,
}

impl PreparedCodebaseMemoryTools {
    /// Appends the prepared safe tools and retains only the metadata needed to
    /// check the finalized registry before rendering guidance.
    pub(super) fn append_to_registry(
        self,
        registry: &mut ToolRegistry,
    ) -> PreparedCodebaseMemoryGuidance {
        let registered_safe_names = self.toolset.registered_tool_names().to_vec();
        let lineage_admission = self.toolset.lineage_admission();
        let coverage = self.toolset.coverage();
        self.toolset.append_to_registry(registry);
        PreparedCodebaseMemoryGuidance {
            prompt_section: self.prompt_section,
            registered_safe_names,
            lineage_admission,
            coverage,
        }
    }
}

impl PreparedCodebaseMemoryGuidance {
    /// Returns guidance only when at least one safe tool from this prepared
    /// toolset survived into the finalized provider registry.
    pub(super) fn prompt_section_for_registry(&self, registry: &ToolRegistry) -> Option<&str> {
        if self
            .registered_safe_names
            .iter()
            .any(|name| registry.get(name).is_some())
        {
            self.prompt_section.as_deref()
        } else {
            None
        }
    }

    pub(super) fn lineage_admission(&self) -> Option<LineageAdmissionHandle> {
        self.lineage_admission.clone()
    }
}

#[cfg(test)]
pub(super) async fn prepare_codebase_memory_tools(
    tool_config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
) -> Result<PreparedCodebaseMemoryTools, CodingAgentError> {
    prepare_codebase_memory_tools_with_timeout(
        tool_config,
        role,
        context,
        cwd,
        Duration::MAX,
        &crate::containment_tests::containment_context(),
        Some(&|| {}),
    )
    .await
}

pub(super) async fn prepare_codebase_memory_tools_with_timeout(
    tool_config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
    generic_tool_timeout: Duration,
    containment: &AgentContainmentContext,
    serving_admitted: Option<&(dyn Fn() + Send + Sync)>,
) -> Result<PreparedCodebaseMemoryTools, CodingAgentError> {
    let toolset = build_codebase_memory_toolset_with_timeout_and_containment(
        tool_config,
        role,
        context,
        cwd,
        generic_tool_timeout,
        containment,
        serving_admitted,
    )
    .await
    .map_err(|error| CodingAgentError::CodebaseMemory(error.to_string()))?;
    let prompt_section = codebase_memory_prompt_section_with_status(
        toolset.registered_tool_metadata(),
        toolset.prompt_status(),
    );
    Ok(PreparedCodebaseMemoryTools {
        prompt_section,
        toolset,
    })
}

#[cfg(test)]
pub(crate) fn codebase_memory_prompt_section(
    tools: &[CodebaseMemoryToolMetadata],
) -> Option<String> {
    codebase_memory_prompt_section_with_status(tools, None)
}

pub(crate) fn codebase_memory_prompt_section_with_status(
    tools: &[CodebaseMemoryToolMetadata],
    status: Option<&str>,
) -> Option<String> {
    // The provider request already contains complete tool names, descriptions,
    // and schemas. Registration metadata controls only whether this guidance is
    // relevant; copying any of it into the prompt would duplicate the tool API.
    if tools.is_empty() {
        return None;
    }

    let status = status
        .map(|status| format!("\nWorkspace/index status:\n{status}\n"))
        .unwrap_or_default();

    let reporting = "Verify is the default task-directed reporting contract: check coverage for every cited source path and read exact source. Scout reports only narrow provisional positive findings. Auditor allows broader conclusions only within explicit checked scopes, a current generation, complete relevant graph and coverage pagination, and stated limitations. No tier or clean coverage result proves completeness.
Use scoped coverage diagnostics at verification time, including after exploration closes. Coverage never reopens discovery, raises retry budgets, grants lineage/source authority, or replaces the ordinary read before mutation. Before absence, exhaustive inventory, or dead-code claims define the scope and finish relevant pagination; otherwise explicitly limit the claim. Read flagged, excluded, skipped, stale or unavailable source directly. Missing optional coverage (also in required mode) and timeouts require bounded direct-read/limited-claim fallback, not repeated provider retries.
When delegating, prefer structured graph_context with the session coverage evidence_id, task scope, qualified symbols, repository-relative paths, source origins, relationships, query bounds, pagination, and limitations. Children keep their existing filesystem permissions and model. Their findings are attributed evidence only. Revalidate project, checkout/source identity and generation before relying on returned findings and after compaction or a fresh session; preserve unresolved flags and limitations.
";
    Some(format!(
        "\nCODEBASE MEMORY:\n{reporting}\n\
         You have repository-index tools for architecture, symbol search, code search,\n\
         and call/impact tracing. For work that needs code discovery, use a targeted repository-index\n\
         query before any shell inventory. Do not precede graph-based source selection with a compound\n\
         shell inventory; keep repository status, validation, and other operational checks as separate\n\
         calls after selection.\n\n\
         When work requires implementation selection, caller/data-flow understanding, or\n\
         behavioral preservation, use every successful targeted graph result as a decision\n\
         checkpoint: consume it with the work-item requirements before selecting a dependent\n\
         refinement, trace, or source read. Select and invoke that dependent operation only in\n\
         a later model turn. A `Decision anchor` explicitly marks a bounded successful targeted\n\
         current-root result; select from that provider result, not unrelated discovery. It is\n\
         absent for failures, unavailable tools, and truncated or ambiguous output. A generic decision-anchor\n\
         recovery message means a successful result was unconsumable: make a bounded later targeted\n\
         correction or stop without a product. Before selecting the initial graph route, derive query\n\
         terms from the requested behavior and intended repair. If the work item also names an incidental\n\
         field, accessor, or symbol, begin with a task-semantic graph query that describes the behavior;\n\
         do not lead with a name pattern or identifier token. Narrow with identifiers returned by that\n\
         semantic result only afterward. An implementation-purpose result may over-return caller- or\n\
         test-shaped candidates. Reuse its exact implementation candidate, but never credit those later\n\
         evidence kinds through direct source reads. When the active-root handoff presents multiple\n\
         implementation candidates, follow its bounded preview/commit protocol: inspect only presented\n\
         candidates without an evidence purpose, compare their source, then explicitly commit exactly one\n\
         candidate with the implementation purpose in a later turn. A preview never earns evidence or\n\
         ordinary read/mutation authority. An implementation source committed before caller and focused-test\n\
         convergence remains provisional. If its typed traversal later produces the bounded pre-mutation\n\
         implementation-correction handoff, inspect every bounded presented alternative exactly once before\n\
         implementation authority becomes final. After those previews complete in either order, explicitly\n\
         correct at most once when one inspected candidate better explains the retained caller/focused-test\n\
         evidence, or retain the provisional target with its exact ordinary read. Correction, exact read, and\n\
         mutation remain blocked during the inspection checkpoint. A correction atomically replaces rather than\n\
         accumulates implementation authority; after it succeeds, the old target and every unchosen candidate\n\
         remain non-actionable. Choose the behaviorally relevant implementation candidate and\n\
         consume its exact source first; only then traverse inbound calls from that exact implementation in a later turn.\n\
         Admit caller source only from an exact identity returned by that traversal. Among\n\
         returned implementation candidates, favor the one whose result context matches the requested\n\
         behavior, then use the shortest provider-derived refinement needed for the decision. Consume the\n\
         relevant caller source; do not choose an outer wrapper or incidental caller merely because it can\n\
         carry a caller evidence label.\n\
         Only the selected-implementation traversal's provider-returned caller identities are eligible\n\
         later-turn caller/model selectors. A\n\
         complete inbound trace with an explicit zero total reports no production callers in the graph; do not\n\
         manufacture caller evidence by rereading the traced symbol as its own caller. This typed outcome waives only\n\
         caller source, never focused-test evidence or the exact ordinary source read. An empty list without a\n\
         total does not establish this outcome. Initial discovery may\n\
         establish independent implementation and focused-test roots in parallel, but every dependent selector\n\
         must come from its own provider result. Focused-test evidence follows a separately admitted root:\n\
         consume only an exact test identity returned by that root in a later turn, and keep its exact source\n\
         lineage on that root. Never move focused-test evidence onto the implementation root, derive a recovery\n\
         selector from task text, source, diagnostics, or another root, or issue an unlisted semantic search or\n\
         caller-to-test traversal. If no provider-derived focused-test action remains, follow the closed\n\
         stop-without-product guidance without retrying or inventing a selector.\n\
         After a local\n\
         decision-evidence denial, follow only the compatible recovery menu and never repeat the denied\n\
         tool/selector/evidence-kind tuple. Do not batch speculative snippets with a producer, and do not\n\
         issue a source consumer until a later turn has received its producer's typed result. When\n\
         multiple typed routes could fill a decision gap, favor the route whose producer query, returned\n\
         implementation, and consumer chain are semantically connected to the requested behavior; an\n\
         evidence-kind declaration alone does not make an incidental route preferable. Keep every source\n\
         selector provider-derived and on the root that produced it. Successful enabled activity never\n\
         releases conventional mutation authority: follow bounded compatible recovery or stop without a\n\
         product. Trusted systemic unavailability remains a distinct one-failure, non-retrying conventional\n\
         fallback. Keep genuinely independent discovery parallel. A call that\n\
         consumes the current result must be in a later model turn; later evidence calls whose\n\
         selectors were established by earlier turns may remain parallel. Do not mutate until consumed\n\
         source evidence covers the selected implementation and its inbound caller on one root (or its typed zero-caller report) plus the\n\
         focused behavioral test on its own retained root, sufficient to justify the smallest semantic diff.\n\
         Bound later independent roots, pivots, readiness rechecks, and rejected selector tuples; repeated,\n\
         broad, malformed, irrelevant, or cross-root attempts add no evidence and cannot reopen exploration.\n\
         Once the retained forest contains the complete implementation/caller and focused-test lineages, stop\n\
         codebase-memory exploration, obey convergence or exploration-closed messages, use conventional reads\n\
         for any remaining verification, and produce the smallest role-appropriate product.\n\n\
         Use them early for non-trivial tasks, but choose the narrowest useful query:\n\
         - concrete defects: begin with a targeted symbol or code search tied to the reported\n\
           symptom, file, or area; then use call/path tracing and read exact source snippets as\n\
           needed. Avoid empty or broad graph searches and broad architecture calls for\n\
           already-localized work.\n\
         - architect: map affected areas before triage/breakdown only when a genuine topology\n\
           question warrants an architecture view;\n\
         - engineer: start with targeted symbols/code, then trace affected callers before editing;\n\
         - scenario-author: inspect the runtime seam and observable behavior before authoring coverage;\n\
         - tester: verify impacted behavior and focused tests before a validation verdict;\n\
         - reviewer: inspect impacted code paths and callers before verdicts;\n\
         - mechanical roles: use only the graph evidence needed for their narrow deterministic product.\n\n\
         Treat the graph as an index, not truth. Verify exact code with read/grep/git diff\n\
         before editing or making final claims.\n\
{status}"
    ))
}

#[cfg(test)]
#[path = "codebase_memory_tests.rs"]
mod tests;
