// SPDX-License-Identifier: MPL-2.0

use temper_protocol_activity::{GraphCorrelationV1, ToolStatusV1};

use super::super::{Action, GraphCall};
use super::decision_evidence;
use crate::{
    GraphConsumptionModeV1, GraphDecisionEvidenceV1, GraphDecisionTargetV1, GraphEvidenceToolV1,
};

/// Selects the closed exact-read proof for one unambiguous typed producer route.
/// Provider prose is deliberately unavailable at this boundary.
pub(super) fn producer_to_exact_read_evidence(
    producer: &GraphCall,
    producer_tool: GraphEvidenceToolV1,
    producer_correlation: &GraphCorrelationV1,
    declarations: &[&GraphDecisionTargetV1],
    actions: &[Action],
) -> Option<GraphDecisionEvidenceV1> {
    let producer_finish = producer.finish_seq?;
    producer
        .decision_anchor_lineage
        .as_ref()
        .filter(|lineage| lineage.is_valid_for(producer_correlation))?;
    let [target] = declarations else {
        return None;
    };

    let mut reads = actions
        .iter()
        .filter(|action| {
            action.name == "read"
                && action.scope_id == producer.scope_id
                && action.status == Some(ToolStatusV1::Succeeded)
                && action
                    .finish_seq
                    .is_some_and(|finish_seq| finish_seq > action.start_seq)
                && action.start_seq > producer_finish
                && action.arguments.as_deref() == Some(target.target.as_str())
        })
        .collect::<Vec<_>>();
    reads.sort_by_key(|action| (action.start_seq, action.finish_seq, &action.call_id));
    let read = reads.into_iter().next()?;

    Some(decision_evidence(
        producer,
        producer_finish,
        producer_tool,
        read.call_id.clone(),
        read.start_seq,
        GraphEvidenceToolV1::Read,
        GraphConsumptionModeV1::Selection,
        target,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use temper_protocol_activity::{
        DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
        GraphCorrelationTargetKindV1, GraphCorrelationToolV1,
    };

    use super::super::super::CallKey;
    use super::super::classify_relevance;
    use super::*;
    use crate::{AnalyzeOptions, GraphDecisionCorrelationV1, GraphDecisionKindV1};

    const SCOPE: &str = "main";
    const ROOT: &str = "00000000-0000-4000-8000-000000000051";
    const PRODUCER: &str = "typed-source";
    const GENERIC_CONSUMER: &str = "generic-consumer";
    const TARGET: &str = "repo/src/route.rs";
    const EARLY_READ: &str = "read-before-source";
    const DENIED_MUTATION: &str = "patch-before-exact-read";
    const EXACT_READ: &str = "read-after-source";
    const FINAL_MUTATION: &str = "patch-after-exact-read";

    struct Fixture {
        options: AnalyzeOptions,
        calls: BTreeMap<CallKey, GraphCall>,
        actions: Vec<Action>,
    }

    impl Fixture {
        fn producer_mut(&mut self) -> &mut GraphCall {
            self.calls.get_mut(&call_key(PRODUCER)).unwrap()
        }

        fn exact_read_mut(&mut self) -> &mut Action {
            self.actions
                .iter_mut()
                .find(|action| action.call_id == EXACT_READ)
                .unwrap()
        }

        fn analyze(&self) -> super::super::RelevanceAnalysis {
            classify_relevance(&self.options, &self.calls, &self.actions)
        }
    }

    fn call_key(call_id: &str) -> CallKey {
        CallKey {
            scope_id: SCOPE.to_string(),
            call_id: call_id.to_string(),
        }
    }

    fn correlation(
        tool: GraphCorrelationToolV1,
        target_kind: GraphCorrelationTargetKindV1,
        target: &str,
    ) -> GraphCorrelationV1 {
        GraphCorrelationV1::new(tool, target_kind, target).unwrap()
    }

    fn declaration(
        target: &str,
        producer_target: &str,
        consumption: Vec<GraphDecisionCorrelationV1>,
    ) -> GraphDecisionTargetV1 {
        GraphDecisionTargetV1 {
            target: target.to_string(),
            kind: GraphDecisionKindV1::Implementation,
            producer: GraphDecisionCorrelationV1 {
                tool: GraphCorrelationToolV1::GetCodeSnippet,
                target_kind: GraphCorrelationTargetKindV1::QualifiedName,
                target: producer_target.to_string(),
            },
            consumption,
        }
    }

    fn lineage(
        stage: DecisionAnchorLineageStageV1,
        target_kind: DecisionAnchorTargetKindV1,
    ) -> Option<DecisionAnchorLineageV1> {
        DecisionAnchorLineageV1::new(
            ROOT.to_string(),
            stage,
            target_kind,
            [DecisionAnchorTargetKindV1::QualifiedName],
        )
    }

    fn graph_call(
        call_id: &str,
        name: &str,
        start_seq: u64,
        finish_seq: u64,
        graph_correlation: GraphCorrelationV1,
        decision_anchor_lineage: Option<DecisionAnchorLineageV1>,
    ) -> GraphCall {
        GraphCall {
            call_id: call_id.to_string(),
            scope_id: SCOPE.to_string(),
            name: name.to_string(),
            start_seq: Some(start_seq),
            finish_seq: Some(finish_seq),
            status: Some(ToolStatusV1::Succeeded),
            graph_correlation: Some(graph_correlation),
            decision_anchor_lineage,
            ..GraphCall::default()
        }
    }

    fn action(
        call_id: &str,
        name: &str,
        start_seq: u64,
        finish_seq: u64,
        arguments: &str,
        status: ToolStatusV1,
    ) -> Action {
        Action {
            call_id: call_id.to_string(),
            scope_id: SCOPE.to_string(),
            name: name.to_string(),
            start_seq,
            finish_seq: Some(finish_seq),
            arguments: Some(arguments.to_string()),
            status: Some(status),
        }
    }

    fn fixture() -> Fixture {
        let root_correlation = correlation(
            GraphCorrelationToolV1::SearchGraph,
            GraphCorrelationTargetKindV1::GraphQuery,
            "typed source discovery",
        );
        let producer_correlation = correlation(
            GraphCorrelationToolV1::GetCodeSnippet,
            GraphCorrelationTargetKindV1::QualifiedName,
            "DeliveryRouter::worker_for",
        );
        let consumer_correlation = correlation(
            GraphCorrelationToolV1::SearchGraph,
            GraphCorrelationTargetKindV1::GraphQuery,
            "generic fallback",
        );
        let mut calls = BTreeMap::new();
        calls.insert(
            call_key("root"),
            graph_call(
                "root",
                GraphCorrelationToolV1::SearchGraph.public_name(),
                1,
                2,
                root_correlation,
                lineage(
                    DecisionAnchorLineageStageV1::Root,
                    DecisionAnchorTargetKindV1::GraphQuery,
                ),
            ),
        );
        calls.insert(
            call_key(PRODUCER),
            graph_call(
                PRODUCER,
                GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                5,
                6,
                producer_correlation,
                lineage(
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionAnchorTargetKindV1::QualifiedName,
                ),
            ),
        );
        calls.insert(
            call_key(GENERIC_CONSUMER),
            graph_call(
                GENERIC_CONSUMER,
                GraphCorrelationToolV1::SearchGraph.public_name(),
                7,
                8,
                consumer_correlation,
                lineage(
                    DecisionAnchorLineageStageV1::CarryForward,
                    DecisionAnchorTargetKindV1::GraphQuery,
                ),
            ),
        );
        let consumption = GraphDecisionCorrelationV1 {
            tool: GraphCorrelationToolV1::SearchGraph,
            target_kind: GraphCorrelationTargetKindV1::GraphQuery,
            target: "generic fallback".to_string(),
        };

        Fixture {
            options: AnalyzeOptions {
                graph_decision_targets: vec![declaration(
                    TARGET,
                    "DeliveryRouter::worker_for",
                    vec![consumption],
                )],
                ..AnalyzeOptions::default()
            },
            calls,
            actions: vec![
                action(EARLY_READ, "read", 3, 4, TARGET, ToolStatusV1::Succeeded),
                action(
                    DENIED_MUTATION,
                    "apply_patch",
                    9,
                    10,
                    &format!(r#"{{"patch":"diff --git a/{TARGET} b/{TARGET}"}}"#),
                    ToolStatusV1::Failed,
                ),
                action(EXACT_READ, "read", 11, 12, TARGET, ToolStatusV1::Succeeded),
                action(
                    FINAL_MUTATION,
                    "apply_patch",
                    13,
                    14,
                    &format!(r#"{{"patch":"diff --git a/{TARGET} b/{TARGET}"}}"#),
                    ToolStatusV1::Succeeded,
                ),
            ],
        }
    }

    fn counts(analysis: &super::super::RelevanceAnalysis) -> (Option<u64>, Option<u64>, u64) {
        (analysis.relevant, analysis.irrelevant, analysis.observed)
    }

    #[test]
    fn exact_typed_read_precedes_earlier_generic_evidence_deterministically() {
        let mut fixture = fixture();
        let baseline = classify_relevance(&fixture.options, &fixture.calls, &[]);
        assert_eq!(counts(&baseline), (Some(2), Some(1), 3));
        assert_eq!(baseline.evidence.len(), 1);
        assert_eq!(
            baseline.evidence[0].consumption_mode,
            GraphConsumptionModeV1::Graph
        );

        fixture.actions.push(action(
            "latest-read",
            "read",
            15,
            16,
            TARGET,
            ToolStatusV1::Succeeded,
        ));
        let analysis = fixture.analyze();
        assert_eq!(counts(&analysis), counts(&baseline));
        assert_eq!(
            analysis
                .evidence
                .iter()
                .map(|evidence| (
                    evidence.graph_call_id.as_str(),
                    evidence.consumer_call_id.as_str(),
                    evidence.graph_finish_seq,
                    evidence.consumer_start_seq,
                    evidence.consumer_tool,
                    evidence.consumption_mode,
                    evidence.target.as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![(
                PRODUCER,
                EXACT_READ,
                6,
                11,
                GraphEvidenceToolV1::Read,
                GraphConsumptionModeV1::Selection,
                TARGET,
            )]
        );
        assert!(analysis.evidence.iter().all(|evidence| {
            ![EARLY_READ, DENIED_MUTATION, FINAL_MUTATION]
                .contains(&evidence.consumer_call_id.as_str())
                && evidence.consumption_mode != GraphConsumptionModeV1::Mutation
        }));
        assert_eq!(
            fixture.analyze().evidence,
            analysis.evidence,
            "equivalent candidates must reduce stably"
        );
        assert_eq!(
            analysis
                .evidence
                .iter()
                .map(|evidence| evidence.graph_call_id.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            analysis.evidence.len(),
        );

        let serialized = serde_json::to_value(&analysis.evidence[0]).unwrap();
        let keys = serialized
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "consumer_call_id",
                "consumer_start_seq",
                "consumer_tool",
                "consumption_mode",
                "graph_call_id",
                "graph_finish_seq",
                "graph_tool",
                "kind",
                "target",
            ])
        );
    }

    #[test]
    fn early_read_and_denied_mutation_cannot_authorize_the_typed_producer() {
        let mut fixture = fixture();
        fixture.options.graph_decision_targets[0]
            .consumption
            .clear();
        fixture
            .actions
            .retain(|action| [EARLY_READ, DENIED_MUTATION].contains(&action.call_id.as_str()));

        let analysis = fixture.analyze();
        assert!(analysis.evidence.iter().all(|evidence| {
            evidence.graph_call_id != PRODUCER
                && evidence.consumer_call_id != EARLY_READ
                && evidence.consumer_call_id != DENIED_MUTATION
                && evidence.consumption_mode != GraphConsumptionModeV1::Mutation
        }));
    }

    type Mutation = Box<dyn Fn(&mut Fixture)>;

    #[test]
    fn exact_typed_read_fails_closed_for_every_invalid_route_or_read() {
        let cases: Vec<(&str, Mutation)> = vec![
            (
                "wrong target",
                Box::new(|fixture| {
                    fixture.exact_read_mut().arguments = Some("repo/src/other.rs".into())
                }),
            ),
            (
                "wrong scope",
                Box::new(|fixture| fixture.exact_read_mut().scope_id = "child".into()),
            ),
            (
                "reversed ordering",
                Box::new(|fixture| fixture.exact_read_mut().start_seq = 6),
            ),
            (
                "failed read",
                Box::new(|fixture| fixture.exact_read_mut().status = Some(ToolStatusV1::Failed)),
            ),
            (
                "incomplete read",
                Box::new(|fixture| fixture.exact_read_mut().finish_seq = None),
            ),
            (
                "invalid read completion ordering",
                Box::new(|fixture| fixture.exact_read_mut().finish_seq = Some(10)),
            ),
            (
                "missing producer correlation",
                Box::new(|fixture| fixture.producer_mut().graph_correlation = None),
            ),
            (
                "invalid producer correlation",
                Box::new(|fixture| {
                    fixture
                        .producer_mut()
                        .graph_correlation
                        .as_mut()
                        .unwrap()
                        .target_digest = "invalid".into()
                }),
            ),
            (
                "missing producer lineage",
                Box::new(|fixture| fixture.producer_mut().decision_anchor_lineage = None),
            ),
            (
                "invalid producer lineage",
                Box::new(|fixture| {
                    fixture
                        .producer_mut()
                        .decision_anchor_lineage
                        .as_mut()
                        .unwrap()
                        .target_kind = DecisionAnchorTargetKindV1::GraphQuery
                }),
            ),
            (
                "ambiguous declarations",
                Box::new(|fixture| {
                    fixture
                        .options
                        .graph_decision_targets
                        .push(fixture.options.graph_decision_targets[0].clone())
                }),
            ),
            (
                "ambiguous producer routes",
                Box::new(|fixture| {
                    fixture.options.graph_decision_targets.push(declaration(
                        "repo/src/other.rs",
                        "DeliveryRouter::other",
                        Vec::new(),
                    ));
                    fixture
                        .producer_mut()
                        .decision_anchor_lineage
                        .as_mut()
                        .unwrap()
                        .canonical_target_digests =
                        vec![GraphCorrelationV1::target_digest("DeliveryRouter::other").unwrap()];
                }),
            ),
            (
                "non-exact arguments",
                Box::new(|fixture| fixture.exact_read_mut().arguments = Some(format!(" {TARGET}"))),
            ),
            (
                "failed producer",
                Box::new(|fixture| fixture.producer_mut().status = Some(ToolStatusV1::Failed)),
            ),
            (
                "non-targeted producer",
                Box::new(|fixture| {
                    fixture.producer_mut().name = "codebase_memory_get_architecture".to_string()
                }),
            ),
            (
                "non-read selection",
                Box::new(|fixture| fixture.exact_read_mut().name = "edit".to_string()),
            ),
        ];

        for (name, mutate) in cases {
            let mut fixture = fixture();
            mutate(&mut fixture);
            let analysis = fixture.analyze();
            assert!(
                analysis.evidence.iter().all(|evidence| {
                    evidence.graph_call_id != PRODUCER
                        || evidence.consumption_mode != GraphConsumptionModeV1::Selection
                }),
                "{name} manufactured exact selection evidence: {:?}",
                analysis.evidence,
            );
        }
    }
}
