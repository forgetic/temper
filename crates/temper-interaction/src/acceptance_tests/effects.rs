use std::sync::Arc;

use temper_forge_model::{Forge, UserId};

use super::{
    StaticResponder, issue_reply, open_session, product_manifest, proposal_id, seeded,
    support_manifest,
};

#[test]
fn arbitrary_profile_issue_creation_acceptance_uses_manifest_effects() {
    temper_engine_io::block_on(async move {
        let manifest = support_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let mut session = open_session(
            human.clone(),
            agent,
            Arc::new(StaticResponder {
                reply: issue_reply(),
            }),
            manifest,
            None,
        )
        .await;

        session
            .send_human_turn("Please file support work.")
            .await
            .unwrap();
        let outcome = session
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();

        assert!(outcome.created);
        assert_eq!(outcome.issue.title, "Support MVP");
        assert_eq!(
            outcome.issue.labels,
            ["proposal-support-mvp", "support-intake"]
        );
        assert_eq!(outcome.issue.assignees, [UserId::new("customer")]);
        assert!(outcome.issue.body.contains("Build a support MVP."));
        assert!(
            outcome
                .issue
                .body
                .contains("Transcript: https://git.example.test/ai/temper/issues/1")
        );
        assert!(
            outcome
                .issue
                .body
                .contains("temper:support-chat-accept-issue=")
        );

        let comments = human
            .list_issue_comments(&session.transcript_issue().id)
            .await
            .unwrap();
        assert!(
            comments
                .iter()
                .any(|comment| comment.body.contains("Accepted support-mvp"))
        );
    })
}

#[test]
fn product_manager_fixture_acceptance_preserves_filed_issue_shape() {
    temper_engine_io::block_on(async move {
        let manifest = product_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let mut session = open_session(
            human,
            agent,
            Arc::new(StaticResponder {
                reply: issue_reply(),
            }),
            manifest,
            None,
        )
        .await;

        session
            .send_human_turn("Please file product work.")
            .await
            .unwrap();
        let outcome = session
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();

        assert_eq!(outcome.issue.labels, ["untriaged"]);
        assert!(
            outcome
                .issue
                .body
                .contains("Transcript: https://git.example.test/ai/temper/issues/1")
        );
        assert!(outcome.issue.body.contains("requested-by: human"));
        assert!(outcome.issue.body.contains("temper:product-chat-file="));
    })
}

#[test]
fn acceptance_retry_is_idempotent_for_issue_and_comment_effects() {
    temper_engine_io::block_on(async move {
        let manifest = support_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let mut session = open_session(
            human.clone(),
            agent,
            Arc::new(StaticResponder {
                reply: issue_reply(),
            }),
            manifest,
            None,
        )
        .await;
        session
            .send_human_turn("Please file support work.")
            .await
            .unwrap();

        let first = session
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();
        let second = session
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();

        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.issue.number, second.issue.number);
        let comments = human
            .list_issue_comments(&session.transcript_issue().id)
            .await
            .unwrap();
        let acceptance_comments = comments
            .iter()
            .filter(|comment| comment.body.contains("Accepted support-mvp"))
            .count();
        assert_eq!(acceptance_comments, 1);
    })
}
