// SPDX-License-Identifier: MPL-2.0

//! Lane-aware, bounded wake scopes and deterministic mechanical target priority.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use temper_forge::{ChangeHint, ChangeKind, HintArtifactKind, HintTarget, ItemNumber};
use temper_workflow::RoleId;

use crate::CiTerminalVerdict;

pub(crate) const MAX_TARGETED_ARTIFACTS: usize = 32;

pub(crate) type WakeArtifactAddress = (HintArtifactKind, ItemNumber);
pub(crate) type WakeTargets = BTreeMap<WakeArtifactAddress, WakeTarget>;

/// Origin of an exact CI wake. The order is an explicit coalescing priority:
/// the dedicated poll carries the monitor's terminal snapshot and therefore
/// wins over a concurrent provider edge, independent of arrival order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum CiTriggerSource {
    Webhook,
    CiPoll,
}

impl CiTriggerSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::CiPoll => "ci_poll",
        }
    }
}

/// An expired, head-specific missing-CI observation that must be revalidated
/// before any coalesced work acts on the pull request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MissingCiRecoveryIntent {
    pub(crate) expected_head_sha: String,
    pub(crate) first_observed_at: DateTime<Utc>,
}

impl MissingCiRecoveryIntent {
    fn merge(self, incoming: Self) -> Self {
        let self_key = (self.first_observed_at, self.expected_head_sha.as_str());
        let incoming_key = (
            incoming.first_observed_at,
            incoming.expected_head_sha.as_str(),
        );
        if incoming_key > self_key {
            incoming
        } else {
            self
        }
    }
}

/// Provenance, best-known terminal facts, and an optional missing-CI recovery
/// intent carried with an exact CI target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CiWakeFacts {
    pub(crate) source: CiTriggerSource,
    pub(crate) verdict: Option<CiTerminalVerdict>,
    pub(crate) completed_at: Option<DateTime<Utc>>,
    pub(crate) recovery: Option<MissingCiRecoveryIntent>,
}

impl CiWakeFacts {
    pub(crate) fn merge(self, incoming: Self) -> Self {
        let recovery = match (self.recovery, incoming.recovery) {
            (Some(existing), Some(incoming)) => Some(existing.merge(incoming)),
            (Some(existing), None) => Some(existing),
            (None, Some(incoming)) => Some(incoming),
            (None, None) => None,
        };
        let self_terminal = (self.source, self.verdict, self.completed_at);
        let incoming_terminal = (incoming.source, incoming.verdict, incoming.completed_at);
        let (source, verdict, completed_at) =
            if terminal_facts_key(incoming_terminal) > terminal_facts_key(self_terminal) {
                incoming_terminal
            } else {
                self_terminal
            };
        Self {
            source,
            verdict,
            completed_at,
            recovery,
        }
    }
}

fn terminal_facts_key(
    facts: (
        CiTriggerSource,
        Option<CiTerminalVerdict>,
        Option<DateTime<Utc>>,
    ),
) -> (bool, CiTriggerSource, Option<DateTime<Utc>>, u8) {
    (
        facts.1.is_some(),
        facts.0,
        facts.2,
        verdict_priority(facts.1),
    )
}

fn verdict_priority(verdict: Option<CiTerminalVerdict>) -> u8 {
    match verdict {
        None => 0,
        Some(CiTerminalVerdict::Failed) => 1,
        Some(CiTerminalVerdict::Passed) => 2,
    }
}

/// One bounded exact target plus optional CI trigger metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WakeTarget {
    pub(crate) change: ChangeKind,
    pub(crate) ci: Option<CiWakeFacts>,
}

impl WakeTarget {
    pub(crate) fn new(change: ChangeKind, ci: Option<CiWakeFacts>) -> Self {
        Self {
            change,
            ci: if change == ChangeKind::Ci { ci } else { None },
        }
    }

    pub(crate) fn merge(&mut self, incoming: Self) {
        self.change = merge_change_kind(self.change, incoming.change);
        self.ci = match (self.ci.take(), incoming.ci) {
            (Some(existing), Some(incoming)) => Some(existing.merge(incoming)),
            (Some(existing), None) => Some(existing),
            (None, Some(incoming)) => Some(incoming),
            (None, None) => None,
        };
    }
}

/// Independently compacted work within one repository run.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum WakeLane {
    Role(RoleId),
    Mechanical,
}

/// Why a repository-wide wake is retained alongside any exact mechanical work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BroadMode {
    Repository,
    Unknown,
    Push,
    Recovery,
    Poll,
    Startup,
    Overflow,
}

/// Bounded work scope for one lane. Mechanical broad scopes retain exact
/// targets that must be serviced before broad reconciliation. Role broad scopes
/// retain only bounded CI trigger metadata for observability; their broad scan
/// still subsumes exact role execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WakeScope {
    Targeted(WakeTargets),
    Broad {
        mode: BroadMode,
        targets: WakeTargets,
    },
}

impl WakeScope {
    pub(crate) fn from_hint(hint: &ChangeHint, ci: Option<CiWakeFacts>) -> Self {
        match (hint.target, hint.change) {
            (_, ChangeKind::Push) => Self::broad(BroadMode::Push),
            (_, ChangeKind::Unknown) => Self::broad(BroadMode::Unknown),
            (HintTarget::Repository, _) => Self::broad(BroadMode::Repository),
            (HintTarget::Artifact { kind, number }, change) => Self::Targeted(BTreeMap::from([(
                (kind, number),
                WakeTarget::new(change, ci),
            )])),
        }
    }

    pub(crate) fn targeted(kind: HintArtifactKind, number: ItemNumber, change: ChangeKind) -> Self {
        Self::targeted_with_ci(kind, number, change, None)
    }

    pub(crate) fn targeted_with_ci(
        kind: HintArtifactKind,
        number: ItemNumber,
        change: ChangeKind,
        ci: Option<CiWakeFacts>,
    ) -> Self {
        Self::Targeted(BTreeMap::from([(
            (kind, number),
            WakeTarget::new(change, ci),
        )]))
    }

    pub(crate) fn broad(mode: BroadMode) -> Self {
        Self::Broad {
            mode,
            targets: BTreeMap::new(),
        }
    }

    pub(crate) fn target_count(&self) -> usize {
        self.targets().len()
    }

    pub(crate) fn targets(&self) -> &WakeTargets {
        match self {
            Self::Targeted(targets) | Self::Broad { targets, .. } => targets,
        }
    }

    pub(crate) fn broad_mode(&self) -> Option<BroadMode> {
        match self {
            Self::Targeted(_) => None,
            Self::Broad { mode, .. } => Some(*mode),
        }
    }
}

/// Lane-specific work compacted into one repository execution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WakeBatch {
    lanes: BTreeMap<WakeLane, WakeScope>,
}

impl WakeBatch {
    pub(crate) fn is_empty(&self) -> bool {
        self.lanes.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.lanes.len()
    }

    pub(crate) fn target_count(&self) -> usize {
        self.lanes.values().map(WakeScope::target_count).sum()
    }

    pub(crate) fn lanes(&self) -> &BTreeMap<WakeLane, WakeScope> {
        &self.lanes
    }

    pub(crate) fn scope(&self, lane: &WakeLane) -> Option<&WakeScope> {
        self.lanes.get(lane)
    }

    pub(crate) fn merge_scope(&mut self, lane: WakeLane, incoming: WakeScope) -> MergeResult {
        let Some(existing) = self.lanes.get_mut(&lane) else {
            self.lanes.insert(lane, incoming);
            return MergeResult::Accepted;
        };

        match lane {
            WakeLane::Mechanical => merge_mechanical_scope(existing, incoming),
            WakeLane::Role(_) => merge_role_scope(existing, incoming),
        }
    }

    pub(crate) fn merge_batch(&mut self, incoming: WakeBatch) {
        for (lane, scope) in incoming.lanes {
            self.merge_scope(lane, scope);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MergeResult {
    Accepted,
    Coalesced,
    BroadPromoted(BroadMode),
}

fn merge_role_scope(existing: &mut WakeScope, incoming: WakeScope) -> MergeResult {
    match existing {
        WakeScope::Broad { mode, targets } => match incoming {
            WakeScope::Broad {
                mode: incoming_mode,
                targets: incoming_targets,
            } => {
                *mode = merge_broad_mode(*mode, incoming_mode);
                merge_result_for_targets(targets, ci_provenance_targets(incoming_targets), mode)
            }
            WakeScope::Targeted(incoming_targets) => {
                merge_result_for_targets(targets, ci_provenance_targets(incoming_targets), mode)
            }
        },
        WakeScope::Targeted(existing_targets) => match incoming {
            WakeScope::Broad {
                mode,
                targets: incoming_targets,
            } => {
                let target_merge = merge_targets(existing_targets, incoming_targets);
                let retained = ci_provenance_targets(std::mem::take(existing_targets));
                let mode = if target_merge == TargetMerge::Overflowed {
                    BroadMode::Overflow
                } else {
                    mode
                };
                *existing = WakeScope::Broad {
                    mode,
                    targets: retained,
                };
                MergeResult::BroadPromoted(mode)
            }
            WakeScope::Targeted(incoming_targets) => {
                match merge_targets(existing_targets, incoming_targets) {
                    TargetMerge::Accepted => MergeResult::Accepted,
                    TargetMerge::Coalesced => MergeResult::Coalesced,
                    TargetMerge::Overflowed => {
                        let retained = ci_provenance_targets(std::mem::take(existing_targets));
                        *existing = WakeScope::Broad {
                            mode: BroadMode::Overflow,
                            targets: retained,
                        };
                        MergeResult::BroadPromoted(BroadMode::Overflow)
                    }
                }
            }
        },
    }
}

fn ci_provenance_targets(mut targets: WakeTargets) -> WakeTargets {
    targets.retain(|_, target| target.ci.is_some());
    targets
}

fn merge_result_for_targets(
    existing: &mut WakeTargets,
    incoming: WakeTargets,
    mode: &mut BroadMode,
) -> MergeResult {
    match merge_targets(existing, incoming) {
        TargetMerge::Overflowed => {
            *mode = BroadMode::Overflow;
            MergeResult::BroadPromoted(BroadMode::Overflow)
        }
        TargetMerge::Accepted => MergeResult::Accepted,
        TargetMerge::Coalesced => MergeResult::Coalesced,
    }
}

fn merge_mechanical_scope(existing: &mut WakeScope, incoming: WakeScope) -> MergeResult {
    match existing {
        WakeScope::Broad { mode, targets } => match incoming {
            WakeScope::Broad {
                mode: incoming_mode,
                targets: incoming_targets,
            } => {
                *mode = merge_broad_mode(*mode, incoming_mode);
                match merge_targets(targets, incoming_targets) {
                    TargetMerge::Overflowed => {
                        *mode = BroadMode::Overflow;
                        MergeResult::BroadPromoted(BroadMode::Overflow)
                    }
                    TargetMerge::Accepted => MergeResult::Accepted,
                    TargetMerge::Coalesced => MergeResult::Coalesced,
                }
            }
            WakeScope::Targeted(incoming_targets) => {
                match merge_targets(targets, incoming_targets) {
                    TargetMerge::Overflowed => {
                        *mode = BroadMode::Overflow;
                        MergeResult::BroadPromoted(BroadMode::Overflow)
                    }
                    TargetMerge::Accepted => MergeResult::Accepted,
                    TargetMerge::Coalesced => MergeResult::Coalesced,
                }
            }
        },
        WakeScope::Targeted(existing_targets) => match incoming {
            WakeScope::Broad {
                mode,
                targets: incoming_targets,
            } => {
                let target_merge = merge_targets(existing_targets, incoming_targets);
                let retained = std::mem::take(existing_targets);
                let mode = if target_merge == TargetMerge::Overflowed {
                    BroadMode::Overflow
                } else {
                    mode
                };
                *existing = WakeScope::Broad {
                    mode,
                    targets: retained,
                };
                MergeResult::BroadPromoted(mode)
            }
            WakeScope::Targeted(incoming_targets) => {
                match merge_targets(existing_targets, incoming_targets) {
                    TargetMerge::Accepted => MergeResult::Accepted,
                    TargetMerge::Coalesced => MergeResult::Coalesced,
                    TargetMerge::Overflowed => {
                        let retained = std::mem::take(existing_targets);
                        *existing = WakeScope::Broad {
                            mode: BroadMode::Overflow,
                            targets: retained,
                        };
                        MergeResult::BroadPromoted(BroadMode::Overflow)
                    }
                }
            }
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetMerge {
    Accepted,
    Coalesced,
    Overflowed,
}

fn merge_targets(existing: &mut WakeTargets, incoming: WakeTargets) -> TargetMerge {
    let mut accepted = false;
    let mut overflowed = false;
    for (address, target) in incoming {
        if let Some(existing_target) = existing.get_mut(&address) {
            existing_target.merge(target);
            continue;
        }

        existing.insert(address, target);
        accepted = true;
        if existing.len() > MAX_TARGETED_ARTIFACTS {
            overflowed = true;
            let lowest_priority = existing
                .iter()
                .max_by(compare_target_priority)
                .map(|(address, _)| *address)
                .expect("overflow has at least one target");
            existing.remove(&lowest_priority);
        }
    }

    if overflowed {
        TargetMerge::Overflowed
    } else if accepted {
        TargetMerge::Accepted
    } else {
        TargetMerge::Coalesced
    }
}

/// Combines duplicate exact requests without relying on declaration-order
/// `Ord`. CI is deliberately dominant because it selects the PR CI fast path;
/// the remaining precedence keeps the most conservative historical signal.
pub(crate) fn merge_change_kind(left: ChangeKind, right: ChangeKind) -> ChangeKind {
    if change_priority(left) >= change_priority(right) {
        left
    } else {
        right
    }
}

fn change_priority(change: ChangeKind) -> u8 {
    match change {
        ChangeKind::Created => 0,
        ChangeKind::Edited => 1,
        ChangeKind::Body => 2,
        ChangeKind::Title => 3,
        ChangeKind::State => 4,
        ChangeKind::Label => 5,
        ChangeKind::Dependency => 6,
        ChangeKind::Assignee => 7,
        ChangeKind::Comment => 8,
        ChangeKind::Review => 9,
        ChangeKind::Push => 10,
        ChangeKind::Unknown => 11,
        ChangeKind::Ci => 12,
    }
}

/// Compares exact targets in service order: PR CI, other PR, then issues.
/// Explicit artifact-kind and item-number keys make ties deterministic.
fn compare_target_priority(
    left: &(&WakeArtifactAddress, &WakeTarget),
    right: &(&WakeArtifactAddress, &WakeTarget),
) -> std::cmp::Ordering {
    target_priority_key(left.0, left.1.change).cmp(&target_priority_key(right.0, right.1.change))
}

fn target_priority_key(address: &WakeArtifactAddress, change: ChangeKind) -> (u8, u8, u64) {
    let service_class = match (address.0, change) {
        (HintArtifactKind::PullRequest, ChangeKind::Ci) => 0,
        (HintArtifactKind::PullRequest, _) => 1,
        (HintArtifactKind::Issue, _) => 2,
    };
    let artifact_kind = match address.0 {
        HintArtifactKind::PullRequest => 0,
        HintArtifactKind::Issue => 1,
    };
    (service_class, artifact_kind, address.1.get())
}

pub(crate) fn prioritized_targets(targets: &WakeTargets) -> Vec<(WakeArtifactAddress, ChangeKind)> {
    let mut prioritized = targets
        .iter()
        .map(|(address, target)| (*address, target.change))
        .collect::<Vec<_>>();
    prioritized.sort_by(|left, right| {
        target_priority_key(&left.0, left.1).cmp(&target_priority_key(&right.0, right.1))
    });
    prioritized
}

fn merge_broad_mode(left: BroadMode, right: BroadMode) -> BroadMode {
    if broad_priority(left) >= broad_priority(right) {
        left
    } else {
        right
    }
}

fn broad_priority(mode: BroadMode) -> u8 {
    match mode {
        BroadMode::Repository => 0,
        BroadMode::Unknown => 1,
        BroadMode::Push => 2,
        BroadMode::Recovery => 3,
        BroadMode::Poll => 4,
        BroadMode::Startup => 5,
        BroadMode::Overflow => 6,
    }
}
