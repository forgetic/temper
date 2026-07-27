use super::*;

const WORKFLOW_FIXTURE: &str =
    include_str!("../../../../../scenarios/plan-centric-feature-branch/config/workflow.json");
const JIG_FIXTURE: &str = include_str!(
    "../../../../../scenarios/plan-centric-feature-branch/jig/plan-centric-feature-branch.json"
);

#[test]
fn live_jig_phase_matching_includes_current_role_charter() {
    let jig: serde_json::Value =
        serde_json::from_str(JIG_FIXTURE).expect("plan-centric Jig fixture parses");
    let phases = jig["phases"]
        .as_array()
        .expect("Jig fixture declares phases");
    for (phase_name, role) in [
        ("architect-decompose-plan", "architect"),
        ("architect-plan-feature", "architect"),
        ("engineer-feature-slices", "engineer"),
        ("scenario-author-feature-proof", "scenario_author"),
        ("tester-after-followup", "tester"),
        ("tester-request-followup", "tester"),
    ] {
        let phase = phases
            .iter()
            .find(|phase| phase["name"].as_str() == Some(phase_name))
            .unwrap_or_else(|| panic!("Jig fixture declares phase {phase_name:?}"));
        let messages = phase["when"]["messages_contain"]
            .as_array()
            .expect("phase declares required messages");
        let contract = contract_for_role(role).expect("role guidance contract exists");
        assert!(
            messages
                .iter()
                .any(|message| message.as_str() == Some(contract.role_guidance)),
            "phase {phase_name:?} must require current {role} charter excerpt {:?} so related-artifact context cannot select the wrong role sequence",
            contract.role_guidance
        );
    }
}

#[test]
fn live_guidance_contracts_match_the_workflow_fixture() {
    let workflow: serde_json::Value =
        serde_json::from_str(WORKFLOW_FIXTURE).expect("plan-centric workflow fixture parses");
    let roles = workflow["roles"]
        .as_array()
        .expect("workflow fixture declares roles");

    for contract in GUIDANCE_CONTRACTS {
        let role = roles
            .iter()
            .find(|role| role["id"].as_str() == Some(contract.role))
            .unwrap_or_else(|| panic!("workflow fixture declares role {:?}", contract.role));
        assert_contains(
            role["charter"].as_str(),
            contract.role_guidance,
            contract.role,
            "charter",
        );
        assert_contains(
            role["prompt"]["guidance"].as_str(),
            contract.prompt_guidance,
            contract.role,
            "prompt guidance",
        );

        let tools = role["external_tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{} role declares external tools", contract.role));
        assert!(
            tools.iter().any(|tool| {
                tool["guidance"]
                    .as_str()
                    .is_some_and(|guidance| guidance.contains(contract.tool_guidance))
            }),
            "{} tool guidance must contain live expectation {:?}",
            contract.role,
            contract.tool_guidance
        );
        for expected in contract.constraints {
            assert!(
                tools.iter().any(|tool| {
                    tool["constraints"].as_array().is_some_and(|constraints| {
                        constraints.iter().any(|constraint| {
                            constraint
                                .as_str()
                                .is_some_and(|constraint| constraint.contains(expected))
                        })
                    })
                }),
                "{} tool constraints must contain live expectation {:?}",
                contract.role,
                expected
            );
        }
    }
}

fn assert_contains(actual: Option<&str>, expected: &str, role: &str, field: &str) {
    assert!(
        actual.is_some_and(|actual| actual.contains(expected)),
        "{role} {field} must contain live expectation {expected:?}; got {actual:?}"
    );
}
