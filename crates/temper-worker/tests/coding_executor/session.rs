use super::support::*;

#[test]
fn corrupt_session_state_fails_closed_without_running_agent_or_overwriting_evidence() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        fixture.seed_pr_head_branch(branch);
        let store = temper_worker::AgentSessionStore::for_workspace_root(
            &fixture.workspace_root,
            "engineer",
            "pr-for-code-7",
        )
        .expect("session store");
        fs::create_dir_all(store.path().parent().expect("session parent"))
            .expect("create corrupt session parent");
        let original = "{not valid json";
        fs::write(store.path(), original).expect("write corrupt session");

        let executor = fixture.executor(AgentBehavior::UnexpectedRun.runner(), true);
        let message = expect_failure_class(
            executor
                .execute(pr_fix_assign(branch, "pr-for-code-7"))
                .await,
            FailureClass::Protocol,
        );

        assert!(
            message.contains("state preserved"),
            "unexpected message: {message}"
        );
        assert_eq!(
            fs::read_to_string(store.path()).expect("corrupt state remains"),
            original,
            "fail-closed attachment must not overwrite malformed evidence"
        );
    });
}

#[test]
fn inconsistent_v2_session_state_fails_attachment_without_agent_or_new_session() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        fixture.seed_pr_head_branch(branch);
        let store = temper_worker::AgentSessionStore::for_workspace_root(
            &fixture.workspace_root,
            "engineer",
            "pr-for-code-7",
        )
        .expect("session store");
        store
            .save_sync(&temper_protocol_agent::AgentSessionState::new(
                "authoritative-session",
            ))
            .expect("save valid V2 session");
        let mut document: Value =
            serde_json::from_slice(&fs::read(store.path()).expect("read valid V2 session"))
                .expect("parse valid V2 session");
        document["ledger"]["consecutive_terminal_count"] = json!(1);
        let original = serde_json::to_vec_pretty(&document).expect("encode inconsistent V2");
        fs::write(store.path(), &original).expect("write inconsistent V2 session");

        let executor = fixture.executor(AgentBehavior::UnexpectedRun.runner(), true);
        let message = expect_failure_class(
            executor
                .execute(pr_fix_assign(branch, "pr-for-code-7"))
                .await,
            FailureClass::Protocol,
        );

        assert!(
            message.contains("state preserved"),
            "unexpected message: {message}"
        );
        assert_eq!(
            fs::read(store.path()).expect("inconsistent V2 remains"),
            original,
            "fail-closed attachment must not rewrite or reset recovery evidence"
        );
        let unchanged: Value = serde_json::from_slice(&original).unwrap();
        assert_eq!(
            unchanged["ledger"]["active_session"]["session_id"], "authoritative-session",
            "attachment must not create a replacement session"
        );
    });
}
