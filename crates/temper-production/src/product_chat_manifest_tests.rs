use temper_interaction::{AcceptanceEffect, ForgeSessionConfig};

use crate::product_chat::product_profile_manifest;

#[test]
fn product_chat_compatibility_config_comes_from_fixture_manifest() {
    let manifest = product_profile_manifest().expect("fixture compiles");
    let config = ForgeSessionConfig::from_profile_manifest(&manifest).unwrap();

    assert_eq!(config.transcript.profile_id, manifest.profile.id);
    assert_eq!(
        config.transcript.transcript_labels,
        manifest.transcript.labels
    );
    assert_eq!(
        config.transcript.transcript_title_prefix,
        manifest.transcript.title_prefix
    );
    assert_eq!(
        config.transcript.human_participant,
        manifest.profile.human_participant
    );
    assert_eq!(
        config.transcript.agent_participant,
        manifest.profile.agent_participant
    );
    assert_eq!(
        config.transcript.recent_turn_limit,
        manifest.profile.recent_turn_limit
    );
    assert_eq!(config.profile, manifest);

    let AcceptanceEffect::CreateIssue(effect) = &manifest.acceptance_actions[0].effects[0] else {
        panic!("product fixture first effect creates an issue")
    };
    assert_eq!(effect.labels(), ["untriaged"]);
    assert_eq!(effect.marker_namespace(), "product-chat");
}
