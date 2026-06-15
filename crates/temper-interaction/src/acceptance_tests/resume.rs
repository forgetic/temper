use std::sync::Arc;

use serde_json::json;

use crate::{InteractionError, Proposal, ProposalKind};

use super::{
    issue_reply, open_session, proposal_id, seeded, support_manifest, NeverResponder,
    StaticResponder,
};

#[test]
fn restart_resume_reconstructs_latest_proposals_and_accepts() {
    temper_engine_io::block_on(async move {
        let manifest = support_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let mut first = open_session(
            human.clone(),
            agent.clone(),
            Arc::new(StaticResponder {
                reply: issue_reply(),
            }),
            manifest.clone(),
            None,
        )
        .await;
        first
            .send_human_turn("Please file support work.")
            .await
            .unwrap();
        let transcript_number = first.transcript_issue().number;
        drop(first);

        let resumed = open_session(
            human,
            agent,
            Arc::new(NeverResponder),
            manifest,
            Some(transcript_number),
        )
        .await;

        assert_eq!(resumed.latest_proposals()[0].id.as_str(), "support-mvp");
        let outcome = resumed
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();
        assert!(outcome.created);
    })
}

#[test]
fn unsupported_proposal_kind_is_rejected_before_persistence() {
    temper_engine_io::block_on(async move {
        let manifest = support_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let reply = crate::ConversationReply {
            message: "custom proposal".into(),
            proposals: vec![Proposal::custom(
                proposal_id("custom-work"),
                ProposalKind::new("custom-kind").unwrap(),
                "Custom work",
                None,
                json!({ "title": "Custom" }),
            )],
        };
        let mut session = open_session(
            human,
            agent,
            Arc::new(StaticResponder { reply }),
            manifest,
            None,
        )
        .await;
        let error = session
            .send_human_turn("Please file custom work.")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            InteractionError::UnsupportedProposalKind { .. }
        ));
    })
}

#[test]
fn responder_boundary_exposes_no_forge_mutation_handle() {
    assert!(!include_str!("../agent.rs").contains("temper_forge"));
    assert!(!include_str!("../process.rs").contains("temper_forge"));
}
