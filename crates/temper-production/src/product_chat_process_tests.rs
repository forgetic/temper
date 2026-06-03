use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use temper_forge::{
    CreateRepository, Forge, Repository, RepositoryPath, UpsertLabel, User, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_interaction::{InteractiveResponder, ProcessResponder, ProcessResponderConfig};

use crate::product_chat::{
    ProductChatOpenOptions, ProductChatSession, PRODUCT_LABEL, WORKFLOW_INTAKE_LABEL,
};

fn user(handle: &str) -> User {
    User {
        id: UserId::new(handle),
        handle: handle.to_string(),
        display_name: None,
        email: None,
    }
}

fn temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "temper-product-chat-{name}-{}-{nanos}",
        std::process::id()
    ))
}

async fn seeded() -> (MemoryForge, MemoryForge, Repository) {
    let forge = MemoryForge::new();
    let human = forge.as_user(user("human"));
    let repo = human
        .create_repository(CreateRepository {
            owner: "ai".into(),
            name: "temper".into(),
            default_branch: "main".into(),
            description: None,
        })
        .await
        .unwrap();
    for label in [PRODUCT_LABEL, WORKFLOW_INTAKE_LABEL] {
        human
            .upsert_label(
                &repo.id,
                UpsertLabel {
                    name: label.into(),
                    color: Some("ededed".into()),
                    description: None,
                },
            )
            .await
            .unwrap();
    }
    (human, forge.as_user(user("product-manager")), repo)
}

#[tokio::test]
async fn product_chat_session_runs_configured_process_responder() {
    let script_path = temp_path("responder.sh");
    fs::write(
        &script_path,
        r#"cat >/dev/null
printf '%s\n' '{"message":"process reply","proposals":[{"id":"process-chat-mvp","kind":"issue","title":"Add process-backed product chat MVP","summary":"Exercises the process responder integration.","payload":{"title":"Add process-backed product chat MVP","body":"Drive product chat through a configured process responder.","rationale":"Exercises the process responder integration."}}]}'
"#,
    )
    .expect("script writes");

    let responder = ProcessResponder::new(
        ProcessResponderConfig::new("/bin/sh")
            .with_args([script_path.to_string_lossy().into_owned()])
            .with_timeout(Duration::from_secs(2)),
    )
    .expect("process responder config validates");
    let (human, product_manager, _repo) = seeded().await;
    let mut session = ProductChatSession::open(
        Arc::new(human),
        Arc::new(product_manager),
        Arc::new(responder) as Arc<dyn InteractiveResponder>,
        ProductChatOpenOptions {
            base_url: "https://git.example.test".into(),
            repo_path: RepositoryPath::new("ai", "temper"),
            transcript_issue: None,
        },
    )
    .await
    .unwrap();

    let response = session
        .send_human_turn("Use a process responder.")
        .await
        .unwrap();
    assert_eq!(response.reply, "process reply");
    assert_eq!(response.drafts[0].slug, "process-chat-mvp");

    let filed = session.file_draft(1).await.unwrap();
    assert!(filed.created);
    assert_eq!(filed.issue.title, "Add process-backed product chat MVP");

    let _ = fs::remove_file(script_path);
}
