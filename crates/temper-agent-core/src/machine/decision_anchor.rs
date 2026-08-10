//! Privacy-safe, per-run enforcement for codebase-memory decision anchors.
//!
//! The policy never retains provider text, model arguments, paths, or source.
//! It compares only bounded SHA-256 fingerprints supplied by the trusted
//! wrapper and drops them with the agent run.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{GraphCorrelationToolV1, GraphCorrelationV1};
use tongs::model::ToolCall;
use tongs::tools::{ToolEffects, ToolOutput};

use super::protocol::{CODEBASE_MEMORY_TOOL_PREFIX, SAFE_GRAPH_CORRELATION_DETAIL_KEY};

/// Reserved wrapper detail carrying bounded fingerprints of provider-returned
/// selection candidates. It is never projected into durable activity metadata.
pub const SAFE_DECISION_ANCHOR_DETAIL_KEY: &str = "temper_decision_anchor_evidence_v1";
const DECISION_ANCHOR_EVIDENCE_VERSION: u32 = 1;
const MAX_RESULT_TARGET_DIGESTS: usize = 256;

/// Fixed, model-visible explanation for a locally denied mutation.
pub const DECISION_ANCHOR_MUTATION_BLOCKED_MESSAGE: &str = "workspace mutation blocked until the successful decision anchor is consumed through later result-derived codebase-memory evidence for the implementation, caller/model, and focused behavioral tests";

/// Closed, privacy-safe evidence extracted transiently from a successful
/// provider result. The entries are only normalized target fingerprints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionAnchorEvidenceV1 {
    pub version: u32,
    pub result_target_digests: Vec<String>,
}

impl DecisionAnchorEvidenceV1 {
    /// Builds a bounded canonical record from already-fingerprinted targets.
    pub fn new(digests: impl IntoIterator<Item = String>) -> Self {
        let mut result_target_digests = digests
            .into_iter()
            .filter(|digest| is_digest(digest))
            .collect::<Vec<_>>();
        result_target_digests.sort();
        result_target_digests.dedup();
        result_target_digests.truncate(MAX_RESULT_TARGET_DIGESTS);
        Self {
            version: DECISION_ANCHOR_EVIDENCE_VERSION,
            result_target_digests,
        }
    }

    /// Rejects unknown versions, duplicate/unbounded fingerprints, and values
    /// that are not canonical SHA-256 hex before the core policy trusts them.
    pub fn is_valid(&self) -> bool {
        self.version == DECISION_ANCHOR_EVIDENCE_VERSION
            && self.result_target_digests.len() <= MAX_RESULT_TARGET_DIGESTS
            && self
                .result_target_digests
                .iter()
                .all(|digest| is_digest(digest))
            && self
                .result_target_digests
                .windows(2)
                .all(|pair| pair[0] < pair[1])
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub(super) struct DecisionAnchorState {
    mutation_tools: BTreeSet<String>,
    phase: Option<AnchorPhase>,
    calls: BTreeMap<String, PendingCodebaseCall>,
}

enum AnchorPhase {
    Root(Anchor),
    Trail(Trail),
}

struct Anchor {
    produced_turn: usize,
    result_target_digests: BTreeSet<String>,
}

struct Trail {
    anchor: Anchor,
    evidence: SourceEvidence,
}

#[derive(Default)]
struct SourceEvidence {
    trace_turn: Option<usize>,
    source_reads_after_trace: u8,
}

struct PendingCodebaseCall {
    turn: usize,
    result_derived: bool,
}

struct AnchorOutput {
    evidence: DecisionAnchorEvidenceV1,
    tool: GraphCorrelationToolV1,
}

impl DecisionAnchorState {
    pub(super) fn from_effects(effects: &BTreeMap<String, ToolEffects>) -> Option<Self> {
        let mutation_tools = effects
            .iter()
            .filter(|(_, effect)| effect.writes)
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        let has_codebase_memory = effects
            .keys()
            .any(|name| name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX));
        (has_codebase_memory && !mutation_tools.is_empty()).then_some(Self {
            mutation_tools,
            phase: None,
            calls: BTreeMap::new(),
        })
    }

    pub(super) fn on_tool_dispatched(&mut self, call: &ToolCall, turn: usize) {
        if !call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
            return;
        }
        let Some(call_key) = GraphCorrelationV1::target_digest(&call.id) else {
            return;
        };
        let arguments = argument_digests(&call.arguments);
        let result_derived = self.active_anchor().is_some_and(|anchor| {
            turn > anchor.produced_turn && !arguments.is_disjoint(&anchor.result_target_digests)
        });
        self.calls.insert(
            call_key,
            PendingCodebaseCall {
                turn,
                result_derived,
            },
        );
    }

    pub(super) fn on_tool_finished(&mut self, id: &str, name: &str, output: &ToolOutput) {
        let Some(call_key) = GraphCorrelationV1::target_digest(id) else {
            return;
        };
        let Some(call) = self.calls.remove(&call_key) else {
            return;
        };
        let Some(output) = anchor_output(name, output) else {
            return;
        };
        let anchor = Anchor {
            produced_turn: call.turn,
            result_target_digests: output.evidence.result_target_digests.into_iter().collect(),
        };

        match self.phase.take() {
            None => self.phase = Some(AnchorPhase::Root(anchor)),
            Some(AnchorPhase::Root(mut root))
                if call.turn == root.produced_turn && !call.result_derived =>
            {
                root.result_target_digests
                    .extend(anchor.result_target_digests);
                self.phase = Some(AnchorPhase::Root(root));
            }
            Some(AnchorPhase::Root(root))
                if call.result_derived && call.turn > root.produced_turn =>
            {
                let mut evidence = SourceEvidence::default();
                evidence.record(output.tool, call.turn);
                self.phase = Some(AnchorPhase::Trail(Trail { anchor, evidence }));
            }
            Some(AnchorPhase::Trail(mut trail))
                if call.result_derived && call.turn > trail.anchor.produced_turn =>
            {
                trail.evidence.record(output.tool, call.turn);
                trail.anchor = anchor;
                self.phase = Some(AnchorPhase::Trail(trail));
            }
            Some(phase) => self.phase = Some(phase),
        }
    }

    pub(super) fn blocks_mutation(&self, name: &str) -> bool {
        self.mutation_tools.contains(name)
            && self.phase.as_ref().is_some_and(|phase| match phase {
                AnchorPhase::Root(_) => true,
                AnchorPhase::Trail(trail) => !trail.evidence.is_complete(),
            })
    }

    fn active_anchor(&self) -> Option<&Anchor> {
        match self.phase.as_ref()? {
            AnchorPhase::Root(anchor) => Some(anchor),
            AnchorPhase::Trail(trail) => Some(&trail.anchor),
        }
    }
}

impl SourceEvidence {
    fn record(&mut self, tool: GraphCorrelationToolV1, turn: usize) {
        match tool {
            GraphCorrelationToolV1::TracePath => {
                self.trace_turn = Some(turn);
                self.source_reads_after_trace = 0;
            }
            GraphCorrelationToolV1::GetCodeSnippet
                if self.trace_turn.is_some_and(|trace_turn| turn > trace_turn) =>
            {
                self.source_reads_after_trace = self.source_reads_after_trace.saturating_add(1);
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.trace_turn.is_some() && self.source_reads_after_trace >= 2
    }
}

fn anchor_output(name: &str, output: &ToolOutput) -> Option<AnchorOutput> {
    if output.is_error || !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return None;
    }
    let details = output.details.as_ref()?;
    let correlation: GraphCorrelationV1 =
        serde_json::from_value(details.get(SAFE_GRAPH_CORRELATION_DETAIL_KEY)?.clone()).ok()?;
    if !correlation.is_valid() || correlation.tool.public_name() != name {
        return None;
    }
    let evidence: DecisionAnchorEvidenceV1 =
        serde_json::from_value(details.get(SAFE_DECISION_ANCHOR_DETAIL_KEY)?.clone()).ok()?;
    evidence.is_valid().then_some(AnchorOutput {
        evidence,
        tool: correlation.tool,
    })
}

fn argument_digests(value: &serde_json::Value) -> BTreeSet<String> {
    let mut digests = BTreeSet::new();
    collect_argument_digests(value, &mut digests);
    digests
}

fn collect_argument_digests(value: &serde_json::Value, digests: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(value) => {
            if let Some(digest) = GraphCorrelationV1::target_digest(value) {
                digests.insert(digest);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_argument_digests(value, digests);
            }
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                if key != "project" && key != "repo" {
                    collect_argument_digests(value, digests);
                }
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}
