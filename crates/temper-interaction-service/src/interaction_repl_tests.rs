use temper_interaction::{
    CommandActionManifest, IssueProposal, Proposal, ProposalId, ProposalKind, RawInteractionSpec,
};

use crate::interaction_commands::{
    BuiltinReplCommand, ParsedReplCommand, parse_repl_command, render_command_help,
    render_proposals, resolve_proposal_selector,
};

const PRODUCT_SPEC: &str =
    include_str!("../../temper-interaction/fixtures/product-manager-interaction-spec.json");

#[test]
fn generic_repl_maps_profile_alias_to_manifest_action() {
    let profile = product_profile_manifest();

    let ParsedReplCommand::Manifest { command, argument } =
        parse_repl_command(&profile, "/file 1").expect("slash command")
    else {
        panic!("expected manifest command")
    };

    assert_eq!(command.id.as_str(), "file-draft");
    assert_eq!(argument, Some("1"));
    match &command.action {
        CommandActionManifest::AcceptProposal {
            proposal_kind,
            acceptance_action,
        } => {
            assert_eq!(proposal_kind, &ProposalKind::issue());
            assert_eq!(acceptance_action.as_str(), "file-draft");
        }
    }
}

#[test]
fn generic_repl_help_and_proposal_rendering_are_manifest_driven() {
    let profile = product_profile_manifest();
    let proposal = Proposal::issue(
        ProposalId::new("mobile-loop".to_string()).unwrap(),
        IssueProposal::with_rationale(
            "Add mobile text loop",
            "Create a mobile-friendly text loop.",
            "Lets humans dogfood from a phone.",
        ),
    )
    .unwrap();

    let help = render_command_help(&profile);
    assert!(help.contains("/file"));
    assert!(help.contains("accept `issue` proposal"));

    let rendered = render_proposals(&profile, std::slice::from_ref(&proposal));
    assert!(rendered.contains("Add mobile text loop"));
    assert!(rendered.contains("mobile-loop"));
    assert!(rendered.contains("Lets humans dogfood from a phone."));

    assert_eq!(
        resolve_proposal_selector(std::slice::from_ref(&proposal), &ProposalKind::issue(), "1")
            .unwrap(),
        proposal.id
    );
}

#[test]
fn generic_repl_builtin_commands_are_local() {
    let profile = product_profile_manifest();
    assert_eq!(
        parse_repl_command(&profile, "/help"),
        Some(ParsedReplCommand::Builtin(BuiltinReplCommand::Help))
    );
    assert_eq!(parse_repl_command(&profile, "hello"), None);
}

fn product_profile_manifest() -> temper_interaction::CompiledProfileManifest {
    let raw: RawInteractionSpec = serde_json::from_str(PRODUCT_SPEC).unwrap();
    raw.validate().unwrap().compile().profiles()[0].clone()
}
