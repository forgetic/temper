// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    AgentAssignmentIdentityV1, CaptureModeV1, FailureInfoV1, StopReasonV1, ToolFailureCategoryV1,
    ToolFailureReasonV1,
};
use temper_protocol_agent::WorkspaceResult;

/// Current JSON contract version for one benchmark run summary.
pub const RUN_SUMMARY_VERSION: u32 = 1;

/// A versioned benchmark summary for one agent run.
///
/// Optional metric groups mean "not observable", never zero. Metrics that are
/// only partially observable carry an explicit coverage value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RunSummaryWireV1")]
pub struct RunSummaryV1 {
    pub version: u32,
    pub identity: RunIdentityV1,
    pub source: TraceInputKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<BenchmarkRunV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureModeV1>,
    pub trace: TraceCoverageV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<RunTerminalV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_ms: Option<u64>,
    pub metrics: RunMetricsV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<ValidationEvidenceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffStatisticsV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostMetadataV1>,
    /// Terminal product emitted by a direct agent-session run. Historical
    /// trace analysis leaves this absent because traces do not carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_result: Option<WorkspaceResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<TraceDiagnosticV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunSummaryWireV1 {
    version: u32,
    identity: RunIdentityV1,
    source: TraceInputKindV1,
    #[serde(default)]
    benchmark: Option<BenchmarkRunV1>,
    #[serde(default)]
    capture: Option<CaptureModeV1>,
    trace: TraceCoverageV1,
    #[serde(default)]
    terminal: Option<RunTerminalV1>,
    #[serde(default)]
    wall_time_ms: Option<u64>,
    metrics: RunMetricsV1,
    #[serde(default)]
    validation: Option<ValidationEvidenceV1>,
    #[serde(default)]
    diff: Option<DiffStatisticsV1>,
    #[serde(default)]
    host: Option<HostMetadataV1>,
    #[serde(default)]
    workspace_result: Option<WorkspaceResult>,
    #[serde(default)]
    diagnostics: Vec<TraceDiagnosticV1>,
}

impl TryFrom<RunSummaryWireV1> for RunSummaryV1 {
    type Error = String;

    fn try_from(value: RunSummaryWireV1) -> Result<Self, Self::Error> {
        if value.version != RUN_SUMMARY_VERSION {
            return Err(format!(
                "unsupported run summary version {}; expected {RUN_SUMMARY_VERSION}",
                value.version
            ));
        }
        if let Some(benchmark) = &value.benchmark {
            if benchmark.name.trim().is_empty() || benchmark.repetition == 0 {
                return Err(
                    "benchmark name and repetition must be non-empty and non-zero".to_string(),
                );
            }
        }
        Ok(Self {
            version: value.version,
            identity: value.identity,
            source: value.source,
            benchmark: value.benchmark,
            capture: value.capture,
            trace: value.trace,
            terminal: value.terminal,
            wall_time_ms: value.wall_time_ms,
            metrics: value.metrics,
            validation: value.validation,
            diff: value.diff,
            host: value.host,
            workspace_result: value.workspace_result,
            diagnostics: value.diagnostics,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunIdentityV1 {
    pub run_id: String,
    pub assignment: AgentAssignmentIdentityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceInputKindV1 {
    JournalDirectory,
    RawEventsJsonl,
    ExportJsonl,
}

/// Identity supplied by the direct benchmark runner. Historical trace analysis
/// leaves this absent rather than guessing a benchmark or execution mode.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkRunV1 {
    pub name: String,
    pub mode: BenchmarkModeV1,
    pub repetition: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<BenchmarkConditionV1>,
}

/// Availability condition selected for a controlled benchmark profile.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkConditionV1 {
    CodebaseMemoryEnabled,
    CodebaseMemoryDisabled,
    CodebaseMemoryUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkModeV1 {
    Harness,
    Live,
}

/// Counts how many values were observed and, when knowable, how many should
/// have been present. `expected: None` means the denominator is unavailable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricCoverageV1 {
    pub observed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceCoverageV1 {
    pub events: MetricCoverageV1,
    pub attachments: MetricCoverageV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seq: Option<u64>,
    pub terminal_event_observed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTerminalStatusV1 {
    Succeeded,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTerminalV1 {
    pub status: RunTerminalStatusV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReasonV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureInfoV1>,
}

/// Metric groups are optional independently so historical metadata-only traces
/// do not acquire misleading zeroes.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunMetricsV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelMetricsV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenMetricsV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolMetricsV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphMetricsV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<StructureMetricsV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMetricsV1 {
    pub calls: u64,
    pub attempts: u64,
    pub succeeded_attempts: u64,
    pub failed_attempts: u64,
    pub cancelled_attempts: u64,
    pub retries: u64,
    pub provider_failures: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_duration_ms: Option<u64>,
    pub duration_coverage: MetricCoverageV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_time_to_first_token_ms: Option<u64>,
    pub time_to_first_token_coverage: MetricCoverageV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenMetricsV1 {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub coverage: MetricCoverageV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolMetricsV1 {
    pub calls: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_duration_ms: Option<u64>,
    pub duration_coverage: MetricCoverageV1,
    pub by_name: BTreeMap<String, ToolNameMetricsV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slowest: Vec<SlowToolCallV1>,
    /// Graph wrappers retain their separate metric group and are excluded from
    /// these ordinary-tool totals. Historical summaries leave this absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinary: Option<OrdinaryToolMetricsV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrdinaryToolMetricsV1 {
    pub calls: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub status_coverage: MetricCoverageV1,
    pub failures_by_category: BTreeMap<ToolFailureCategoryV1, u64>,
    pub failure_category_coverage: MetricCoverageV1,
    pub failures_by_reason: BTreeMap<ToolFailureReasonV1, u64>,
    pub failure_reason_coverage: MetricCoverageV1,
    /// Available only when every ordinary call status and every non-success
    /// diagnostic are present. This is derived solely from the closed
    /// circuit-redirect category/reasons, never from arguments or fingerprints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeated_failure_redirects: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolNameMetricsV1 {
    pub calls: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_duration_ms: Option<u64>,
    pub duration_coverage: MetricCoverageV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlowToolCallV1 {
    pub call_id: String,
    pub name: String,
    pub duration_ms: u64,
}

/// Decision target classes are fixture-declared; the analyzer never guesses a
/// caller or focused test from an RPC success or a filename alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphDecisionKindV1 {
    Implementation,
    Caller,
    FocusedTest,
}

/// Tool names which may appear in privacy-safe graph decision evidence. The
/// analyzer maps trace-local names to this closed vocabulary rather than
/// copying arbitrary tool names into a run summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphEvidenceToolV1 {
    Read,
    Edit,
    Write,
    ApplyPatch,
    SearchGraph,
    SearchCode,
    TracePath,
    GetCodeSnippet,
}

impl GraphEvidenceToolV1 {
    pub(crate) fn from_tool_name(name: &str) -> Option<Self> {
        match name {
            "read" => Some(Self::Read),
            "edit" => Some(Self::Edit),
            "write" => Some(Self::Write),
            "apply_patch" => Some(Self::ApplyPatch),
            "codebase_memory_search_graph" => Some(Self::SearchGraph),
            "codebase_memory_search_code" => Some(Self::SearchCode),
            "codebase_memory_trace_path" => Some(Self::TracePath),
            "codebase_memory_get_code_snippet" => Some(Self::GetCodeSnippet),
            _ => None,
        }
    }

    pub(crate) fn is_targeted_graph(self) -> bool {
        matches!(
            self,
            Self::SearchGraph | Self::SearchCode | Self::TracePath | Self::GetCodeSnippet
        )
    }
}

/// The bounded, ordered action by which a graph result was consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphConsumptionModeV1 {
    Selection,
    Graph,
    Source,
    Mutation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphDecisionEvidenceV1 {
    pub graph_call_id: String,
    /// The producer must finish before this sequence, in the same scope, for
    /// the evidence to be considered relevant.
    pub graph_finish_seq: u64,
    pub graph_tool: GraphEvidenceToolV1,
    pub consumer_call_id: String,
    /// The consumer must start at this sequence after `graph_finish_seq`.
    pub consumer_start_seq: u64,
    pub consumer_tool: GraphEvidenceToolV1,
    pub consumption_mode: GraphConsumptionModeV1,
    pub target: String,
    pub kind: GraphDecisionKindV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConventionalDiscoveryMetricsV1 {
    /// Direct conventional-tool calls are counted independently of shell
    /// classification so partial shell evidence does not erase them.
    pub grep_calls: u64,
    pub find_calls: u64,
    pub read_calls: u64,
    /// Discovery command-list segments from completely parsed shell calls.
    #[serde(alias = "classified_shell_calls")]
    pub classified_shell_segments: u64,
    /// The all-component conventional-discovery total is available only when
    /// every preceding shell call was captured and classified. It is the value
    /// suitable for a cross-condition improvement gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_calls: Option<u64>,
    /// Shell calls whose complete command was parsed under the constrained
    /// command-list grammar, over all preceding shell calls.
    #[serde(alias = "shell_classification_coverage")]
    pub shell_command_classification_coverage: MetricCoverageV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphMetricsV1 {
    pub calls: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub failures_by_category: BTreeMap<ToolFailureCategoryV1, u64>,
    pub status_coverage: MetricCoverageV1,
    pub failure_category_coverage: MetricCoverageV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_readiness_wait_ms: Option<u64>,
    pub readiness_wait_coverage: MetricCoverageV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_discovery_duration_ms: Option<u64>,
    pub discovery_duration_coverage: MetricCoverageV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immediate_repeated_attempts_after_systemic_failure: Option<u64>,
    pub immediate_repeat_coverage: MetricCoverageV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevant_results: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irrelevant_successes: Option<u64>,
    pub relevance_coverage: MetricCoverageV1,
    /// Successful graph calls carrying a complete wrapper-issued typed target
    /// correlation, over all successful graph calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_correlation_coverage: Option<MetricCoverageV1>,
    /// Successful graph calls whose decision-anchor lineage is valid for that
    /// typed correlation. Acceptance requires complete current-chain lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_lineage_coverage: Option<MetricCoverageV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_evidence: Vec<GraphDecisionEvidenceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conventional_discovery_before_selection: Option<ConventionalDiscoveryMetricsV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructureMetricsV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_edit_attempts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_turns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub single_mutation_turns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mutations_per_turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_boundaries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_validation_mutations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_invalidations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revalidations: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationEvidenceV1 {
    pub command_count: u64,
    pub succeeded: u64,
    pub failed: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffStatisticsV1 {
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
    pub tracked_files: u64,
    pub untracked_files: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostMetadataV1 {
    pub temper: TemperBuildMetadataV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_models: Vec<ObservedModelIdentityV1>,
    pub os: String,
    pub architecture: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_cpu_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_average: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_warmth: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemperBuildMetadataV1 {
    pub package_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedModelIdentityV1 {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceDiagnosticCodeV1 {
    SequenceGap,
    TraceGap,
    TruncatedRecord,
    TruncatedContent,
    IncompleteModelCall,
    IncompleteToolCall,
    MissingRunStart,
    MissingTerminalEvent,
    HostEvidenceUnavailable,
    DiffEvidenceUnavailable,
    ValidationEvidenceUnavailable,
    StructureEvidenceUnavailable,
    GraphEvidenceUnavailable,
    OrdinaryToolEvidenceUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverityV1 {
    Info,
    Warning,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceDiagnosticV1 {
    pub code: TraceDiagnosticCodeV1,
    pub severity: DiagnosticSeverityV1,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}
