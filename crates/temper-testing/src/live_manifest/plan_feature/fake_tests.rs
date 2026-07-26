use super::*;

const WORKFLOW_FIXTURE: &str =
    include_str!("../../../../../scenarios/plan-centric-feature-branch/config/workflow.json");

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
