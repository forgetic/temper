use temper_interaction::{AcceptanceEffect, RawInteractionSpec};

const DOGFOOD_PRODUCT_MANAGER_SPEC: &str =
    include_str!("../../../examples/dogfood/config/interaction-profiles/product-manager.json");

#[test]
fn dogfood_example_product_manager_profile_validates_and_compiles() {
    let raw: RawInteractionSpec = serde_json::from_str(DOGFOOD_PRODUCT_MANAGER_SPEC).unwrap();
    let compiled = raw.validate().unwrap().compile();
    let profile = compiled.profiles().first().expect("profile compiles");

    assert_eq!(profile.profile.id.as_str(), "product-manager");
    assert_eq!(profile.responder.id.as_str(), "product-manager-responder");
    assert_eq!(profile.transcript.labels, ["product"]);
    assert_eq!(profile.transcript.title_prefix, "Product conversation");
    assert_eq!(profile.transcript.marker_namespace.as_str(), "product-chat");
    assert_eq!(profile.commands[0].aliases, ["/file"]);

    let action = &profile.acceptance_actions[0];
    assert_eq!(action.proposal_kind.as_str(), "issue");
    let AcceptanceEffect::CreateIssue(effect) = &action.effects[0] else {
        panic!("dogfood accepted action should create an issue");
    };
    assert_eq!(effect.labels(), ["untriaged"]);
    assert_eq!(effect.marker_namespace(), "product-chat");
    assert_eq!(effect.marker_key(), Some("file"));
}
