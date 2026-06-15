mod protocol;
mod session;

use temper_forge::{User, UserId};

use crate::ProposalId;

const TRANSCRIPT_LABEL: &str = "product";
const INTAKE_LABEL: &str = "untriaged";
const MARKER_NAMESPACE: &str = "product-chat";

fn proposal_id(value: &str) -> ProposalId {
    ProposalId::new(value).expect("valid proposal id")
}

fn user(handle: &str) -> User {
    User {
        id: UserId::new(handle),
        handle: handle.to_string(),
        display_name: None,
        email: None,
    }
}

fn product_profile_manifest() -> crate::CompiledProfileManifest {
    let raw: crate::RawInteractionSpec = serde_json::from_str(include_str!(
        "../../fixtures/product-manager-interaction-spec.json"
    ))
    .expect("fixture deserializes");
    raw.validate()
        .expect("fixture validates")
        .compile()
        .profiles()[0]
        .clone()
}
