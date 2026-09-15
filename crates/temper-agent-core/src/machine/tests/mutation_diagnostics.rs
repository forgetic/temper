//! Denied shell starts retain their closed disposition and hide arguments.

use std::{collections::BTreeMap, sync::Arc};
use temper_agent_io::{EngineTime, Machine};
use temper_protocol_activity::ShellDiscoveryDispositionV1;
use tongs::tools::ToolEffects;

use super::common::{assistant_tool_call_with_args, complete, llm_responded, user};
use crate::machine::{
    AgentEvent, AgentMachine, AgentRequest, ToolCallDenial, ToolStartPresentation,
};
use crate::{
    InvocationTargetAdmission, LineageAdmissionOutcome, LineageAdmissionResolver,
    LineageAdmissionStatus, TargetAdmissionStatus,
};

struct InvalidTargets(TargetAdmissionStatus);

impl LineageAdmissionResolver for InvalidTargets {
    fn resolve(&self, _: &str, _: &serde_json::Value) -> LineageAdmissionOutcome {
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnsupportedTool)
    }

    fn resolve_invocation_targets(
        &self,
        _: &str,
        _: &serde_json::Value,
    ) -> InvocationTargetAdmission {
        InvocationTargetAdmission::Ineligible(self.0)
    }
}

#[test]
fn mutation_diagnostics_keep_denied_shell_starts_private_and_excluded() {
    for (status, expected) in [
        (
            TargetAdmissionStatus::MalformedTarget,
            ToolCallDenial::MalformedMutationTarget,
        ),
        (
            TargetAdmissionStatus::CompetingTargets,
            ToolCallDenial::ConflictingMutationTargets,
        ),
        (
            TargetAdmissionStatus::UnknownTarget,
            ToolCallDenial::DecisionAnchorMutation,
        ),
    ] {
        let effects = BTreeMap::from([
            (
                "codebase_memory_search_graph".to_string(),
                ToolEffects::read(),
            ),
            ("bash".to_string(), ToolEffects::process()),
        ]);
        let mut machine = AgentMachine::with_effects(vec![user("inspect")], 10, effects)
            .with_lineage_admission(Arc::new(InvalidTargets(status)))
            .with_arg_preview(Arc::new(|_, _| -> ToolStartPresentation {
                panic!("denied arguments must never reach presentation")
            }));
        let _ = machine.on_start(EngineTime::ZERO);
        let requests = complete(
            &mut machine,
            llm_responded(assistant_tool_call_with_args(
                "denied",
                "bash",
                serde_json::json!({"command":"PRIVATE-COMMAND /private/path CREDENTIAL"}),
            )),
        );
        assert!(requests.iter().any(|request| matches!(request,
            AgentRequest::RunTool { denial: Some(denial), .. } if denial == &expected
        )));
        let start = requests
            .iter()
            .find_map(|request| match request {
                AgentRequest::Emit(event @ AgentEvent::ToolStart { .. }) => Some(event),
                _ => None,
            })
            .expect("denied tool start");
        let AgentEvent::ToolStart {
            arg_preview,
            diagnostic_arguments,
            shell_discovery_disposition,
            ..
        } = start
        else {
            unreachable!()
        };
        assert_eq!(arg_preview, &None);
        assert_eq!(diagnostic_arguments, &None);
        assert_eq!(
            *shell_discovery_disposition,
            Some(ShellDiscoveryDispositionV1::excluded_never_executed_local_policy_denial())
        );
        let rendered = format!("{start:?}");
        for secret in ["PRIVATE-COMMAND", "/private/path", "CREDENTIAL"] {
            assert!(!rendered.contains(secret));
        }
    }
}
