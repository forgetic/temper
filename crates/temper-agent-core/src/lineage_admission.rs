//! Run-local pre-provider resolution for codebase-memory lineage selectors.
//!
//! The implementation lives in the trusted wrapper tier. Core passes a
//! normalized call to it and receives only closed policy facts; raw arguments
//! and provider-shaped registry values never become machine state or protocol.

use std::fmt;
use std::sync::Arc;

use serde_json::Value;
use temper_protocol_activity::{
    DecisionAnchorLineageV1, DecisionAnchorTargetKindV1, DecisionEvidenceKindV1,
    GraphCorrelationToolV1, GraphRecoveryReferenceDispositionV1,
};

/// Shared process-local resolver installed for one agent run.
pub type LineageAdmissionHandle = Arc<dyn LineageAdmissionResolver>;

/// Wrapper-owned resolver queried synchronously at the trusted tool boundary.
///
/// Graph selectors are resolved before provider dispatch. Source targets are
/// resolved only after a successful typed wrapper result, while ordinary
/// filesystem targets are resolved only after invocation canonicalization.
/// Every returned target is an opaque run-local identity.
pub trait LineageAdmissionResolver: Send + Sync {
    fn resolve(&self, tool_name: &str, arguments: &Value) -> LineageAdmissionOutcome;

    /// Resolves a selector against the machine-selected active root. The root
    /// is process-local policy state and is never exposed to the model.
    fn resolve_for_active_root(
        &self,
        tool_name: &str,
        arguments: &Value,
        _active_root: Option<&str>,
    ) -> LineageAdmissionOutcome {
        self.resolve(tool_name, arguments)
    }

    /// Resolves an active-root call and, when applicable, atomically reserves
    /// one recognized opaque trace reference before ordinary selector
    /// eligibility is evaluated. The closed disposition contains no selector,
    /// reference, root, or provider value.
    fn resolve_for_active_root_with_recovery(
        &self,
        tool_name: &str,
        arguments: &Value,
        active_root: Option<&str>,
    ) -> (
        LineageAdmissionOutcome,
        Option<GraphRecoveryReferenceDispositionV1>,
    ) {
        (
            self.resolve_for_active_root(tool_name, arguments, active_root),
            None,
        )
    }

    /// Returns the model-safe selector reference for a caller traversal of the
    /// exact active implementation root. The malformed call that prompted
    /// this lookup is still denied locally; only a later model turn may use
    /// the reference in the public `function_name` field.
    fn trace_recovery_selector(
        &self,
        _active_root: &str,
    ) -> Option<OpaqueRecoverySelectorReference> {
        None
    }

    fn resolve_source_target(&self, _lineage: &DecisionAnchorLineageV1) -> TargetAdmissionOutcome {
        TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget)
    }

    fn resolve_invocation_targets(
        &self,
        _tool_name: &str,
        _arguments: &Value,
    ) -> InvocationTargetAdmission {
        InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::UnsupportedTool)
    }
}

/// Closed reasons why a source, read, or mutation target is not eligible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetAdmissionStatus {
    UnknownTarget,
    AmbiguousTarget,
    MalformedTarget,
    OutsideWorkspace,
    UnsupportedTool,
    CompetingTargets,
}

/// One target represented only by an unguessable identity scoped to this run.
#[derive(Clone, Eq, PartialEq)]
pub struct EligibleWorkspaceTarget {
    identity: OpaqueWorkspaceTargetIdentity,
}

impl EligibleWorkspaceTarget {
    pub fn new(identity: String) -> Option<Self> {
        Some(Self {
            identity: OpaqueWorkspaceTargetIdentity::new(identity)?,
        })
    }

    /// Policy comparison without exposing the underlying process-local value.
    pub fn matches(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl fmt::Debug for EligibleWorkspaceTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EligibleWorkspaceTarget")
            .field("identity", &self.identity)
            .finish()
    }
}

/// An unguessable, run-local reference that is safe to return to the model and
/// accepted as a public graph selector. The trusted wrapper expands it to the
/// exact provider-returned value immediately before provider dispatch.
#[derive(Clone, Eq, PartialEq)]
pub struct OpaqueRecoverySelectorReference(String);

impl OpaqueRecoverySelectorReference {
    const PREFIX: &'static str = "temper-recovery-selector:";

    pub fn new(value: String) -> Option<Self> {
        value
            .strip_prefix(Self::PREFIX)
            .is_some_and(valid_run_local_identity)
            .then_some(Self(value))
    }

    /// The deliberately model-visible value for the public selector field.
    pub fn as_public_selector(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OpaqueRecoverySelectorReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<opaque-recovery-selector>")
    }
}

/// Closed resolution for one independently exposed filesystem target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetAdmissionOutcome {
    Eligible(EligibleWorkspaceTarget),
    Ineligible(TargetAdmissionStatus),
}

/// Target shape of one canonical ordinary invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InvocationTargetAdmission {
    Read(TargetAdmissionOutcome),
    /// Every explicit mutation target has its own entry. An ineligible entry
    /// cannot piggyback on an eligible sibling in a multi-target operation.
    Mutation(Vec<TargetAdmissionOutcome>),
    /// A process invocation classified by the trusted wrapper as having no
    /// direct source-mutation operation.
    SourceNeutralProcess,
    /// A named control-plane operation that does not mutate workspace source.
    ControlPlane,
    Ineligible(TargetAdmissionStatus),
}

#[derive(Clone, Eq, PartialEq)]
struct OpaqueWorkspaceTargetIdentity(String);

impl OpaqueWorkspaceTargetIdentity {
    fn new(value: String) -> Option<Self> {
        valid_run_local_identity(&value).then_some(Self(value))
    }
}

impl fmt::Debug for OpaqueWorkspaceTargetIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<opaque-workspace-target>")
    }
}

/// Bounded reasons why a call cannot be tied to one exact registered root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineageAdmissionStatus {
    UnknownSelector,
    AmbiguousSelector,
    MalformedSelector,
    BroadSelector,
    UnsupportedTool,
    IncapableSelection,
    TraversalNotReady,
    TraversalReadinessExhausted,
}

/// Closed pre-provider result. Neither variant can retain a raw selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LineageAdmissionOutcome {
    Eligible(EligibleLineageAdmission),
    Ineligible(LineageAdmissionStatus),
}

/// An eligible current-root selection containing only policy facts.
#[derive(Clone, Eq, PartialEq)]
pub struct EligibleLineageAdmission {
    root_binding: OpaqueLineageRootBinding,
    selector_kind: DecisionAnchorTargetKindV1,
    tool_kind: GraphCorrelationToolV1,
    evidence_purpose: Option<DecisionEvidenceKindV1>,
    recovery_purpose: Option<DecisionEvidenceKindV1>,
    traversal_readiness_recheck: bool,
}

impl EligibleLineageAdmission {
    pub fn new(
        root_binding: String,
        selector_kind: DecisionAnchorTargetKindV1,
        tool_kind: GraphCorrelationToolV1,
        evidence_purpose: Option<DecisionEvidenceKindV1>,
    ) -> Option<Self> {
        let capable = matches!(
            (tool_kind, selector_kind, evidence_purpose),
            (
                GraphCorrelationToolV1::SearchCode,
                DecisionAnchorTargetKindV1::Pattern,
                None,
            ) | (
                GraphCorrelationToolV1::TracePath,
                DecisionAnchorTargetKindV1::FunctionName,
                None,
            ) | (
                GraphCorrelationToolV1::GetCodeSnippet,
                DecisionAnchorTargetKindV1::QualifiedName,
                Some(_),
            )
        );
        capable.then_some(Self {
            root_binding: OpaqueLineageRootBinding::new(root_binding)?,
            selector_kind,
            tool_kind,
            evidence_purpose,
            recovery_purpose: None,
            traversal_readiness_recheck: false,
        })
    }

    pub fn exact_graph_narrowing(
        root_binding: String,
        selector_kind: DecisionAnchorTargetKindV1,
    ) -> Option<Self> {
        matches!(
            selector_kind,
            DecisionAnchorTargetKindV1::NamePattern
                | DecisionAnchorTargetKindV1::QualifiedNamePattern
        )
        .then_some(())?;
        Some(Self {
            root_binding: OpaqueLineageRootBinding::new(root_binding)?,
            selector_kind,
            tool_kind: GraphCorrelationToolV1::SearchGraph,
            evidence_purpose: None,
            recovery_purpose: None,
            traversal_readiness_recheck: false,
        })
    }

    pub fn implementation_caller_traversal(
        root_binding: String,
        selector_kind: DecisionAnchorTargetKindV1,
    ) -> Option<Self> {
        (selector_kind == DecisionAnchorTargetKindV1::FunctionName).then_some(())?;
        Some(Self {
            root_binding: OpaqueLineageRootBinding::new(root_binding)?,
            selector_kind,
            tool_kind: GraphCorrelationToolV1::TracePath,
            evidence_purpose: None,
            recovery_purpose: None,
            traversal_readiness_recheck: false,
        })
    }

    pub fn implementation_traversal_readiness_recheck(
        root_binding: String,
        selector_kind: DecisionAnchorTargetKindV1,
    ) -> Option<Self> {
        (selector_kind == DecisionAnchorTargetKindV1::QualifiedName).then_some(())?;
        Some(Self {
            root_binding: OpaqueLineageRootBinding::new(root_binding)?,
            selector_kind,
            tool_kind: GraphCorrelationToolV1::GetCodeSnippet,
            evidence_purpose: Some(DecisionEvidenceKindV1::Implementation),
            recovery_purpose: None,
            traversal_readiness_recheck: true,
        })
    }

    pub fn focused_test_traversal(
        root_binding: String,
        selector_kind: DecisionAnchorTargetKindV1,
    ) -> Option<Self> {
        (selector_kind == DecisionAnchorTargetKindV1::FunctionName).then_some(())?;
        Some(Self {
            root_binding: OpaqueLineageRootBinding::new(root_binding)?,
            selector_kind,
            tool_kind: GraphCorrelationToolV1::TracePath,
            evidence_purpose: None,
            recovery_purpose: Some(DecisionEvidenceKindV1::FocusedTest),
            traversal_readiness_recheck: false,
        })
    }

    pub fn focused_test_semantic_fallback(
        root_binding: String,
        selector_kind: DecisionAnchorTargetKindV1,
    ) -> Option<Self> {
        (selector_kind == DecisionAnchorTargetKindV1::GraphQuery).then_some(())?;
        Some(Self {
            root_binding: OpaqueLineageRootBinding::new(root_binding)?,
            selector_kind,
            tool_kind: GraphCorrelationToolV1::SearchGraph,
            evidence_purpose: None,
            recovery_purpose: Some(DecisionEvidenceKindV1::FocusedTest),
            traversal_readiness_recheck: false,
        })
    }

    pub const fn selector_kind(&self) -> DecisionAnchorTargetKindV1 {
        self.selector_kind
    }

    pub const fn tool_kind(&self) -> GraphCorrelationToolV1 {
        self.tool_kind
    }

    pub const fn evidence_purpose(&self) -> Option<DecisionEvidenceKindV1> {
        self.evidence_purpose
    }

    pub const fn recovery_purpose(&self) -> Option<DecisionEvidenceKindV1> {
        self.recovery_purpose
    }

    pub const fn is_traversal_readiness_recheck(&self) -> bool {
        self.traversal_readiness_recheck
    }

    /// Compares a trusted lineage root without exposing this process-local
    /// binding to callers, formatting, messages, or serialization.
    pub fn matches_root(&self, candidate: &str) -> bool {
        self.root_binding.0 == candidate
    }
}

impl fmt::Debug for EligibleLineageAdmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EligibleLineageAdmission")
            .field("root_binding", &self.root_binding)
            .field("selector_kind", &self.selector_kind)
            .field("tool_kind", &self.tool_kind)
            .field("evidence_purpose", &self.evidence_purpose)
            .field("recovery_purpose", &self.recovery_purpose)
            .field(
                "traversal_readiness_recheck",
                &self.traversal_readiness_recheck,
            )
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
struct OpaqueLineageRootBinding(String);

impl OpaqueLineageRootBinding {
    fn new(value: String) -> Option<Self> {
        valid_root_binding(&value).then_some(Self(value))
    }
}

impl fmt::Debug for OpaqueLineageRootBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<opaque-lineage-root>")
    }
}

fn valid_root_binding(value: &str) -> bool {
    valid_run_local_identity(value)
}

fn valid_run_local_identity(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes().get(14) == Some(&b'4')
        && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && byte == b'-'
                || !matches!(index, 8 | 13 | 18 | 23)
                    && byte.is_ascii_hexdigit()
                    && !byte.is_ascii_uppercase()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_debug_redacts_the_process_local_root() {
        const ROOT: &str = "00000000-0000-4000-8000-000000000001";
        let admission = EligibleLineageAdmission::new(
            ROOT.to_string(),
            DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::GetCodeSnippet,
            Some(DecisionEvidenceKindV1::FocusedTest),
        )
        .expect("closed eligible admission");
        assert!(admission.matches_root(ROOT));
        let debug = format!("{admission:?}");
        assert!(debug.contains("FocusedTest"));
        assert!(!debug.contains(ROOT));
    }

    #[test]
    fn exact_graph_narrowing_accepts_only_closed_graph_pattern_kinds() {
        const ROOT: &str = "00000000-0000-4000-8000-000000000001";
        for kind in [
            DecisionAnchorTargetKindV1::NamePattern,
            DecisionAnchorTargetKindV1::QualifiedNamePattern,
        ] {
            let admission = EligibleLineageAdmission::exact_graph_narrowing(ROOT.to_string(), kind)
                .expect("exact graph selector admission");
            assert!(admission.matches_root(ROOT));
            assert_eq!(admission.tool_kind(), GraphCorrelationToolV1::SearchGraph);
        }
        assert!(
            EligibleLineageAdmission::exact_graph_narrowing(
                ROOT.to_string(),
                DecisionAnchorTargetKindV1::GraphQuery,
            )
            .is_none()
        );
    }

    #[test]
    fn workspace_target_identity_is_comparable_but_never_debug_visible() {
        const FIRST: &str = "00000000-0000-4000-8000-000000000011";
        const SECOND: &str = "00000000-0000-4000-8000-000000000012";
        let first = EligibleWorkspaceTarget::new(FIRST.to_string()).unwrap();
        let same = EligibleWorkspaceTarget::new(FIRST.to_string()).unwrap();
        let second = EligibleWorkspaceTarget::new(SECOND.to_string()).unwrap();

        assert!(first.matches(&same));
        assert!(!first.matches(&second));
        let debug = format!("{first:?}");
        assert!(debug.contains("opaque-workspace-target"));
        assert!(!debug.contains(FIRST));
        assert!(EligibleWorkspaceTarget::new("raw/path.rs".to_string()).is_none());
    }
}
