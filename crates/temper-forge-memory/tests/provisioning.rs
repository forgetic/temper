//! Behaviour tests for the in-memory provisioning capabilities
//! ([`ForgeContent`] + [`ForgeAdmin`]) and the test-only read-back accessors.
//!
//! The in-memory futures never park, so a hand-rolled `block_on` drives them to
//! completion without an async runtime.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    AccessGrant, AccessScope, CommitFile, CreateBranch, EnsureRepository, ForgeAdmin, ForgeContent,
    NewUser, RepoPermission, RepositoryId, RepositoryPath, TokenScope, WebhookEvents, WebhookSpec,
};

struct NoopWake;
impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory forge futures should not park"),
    }
}

#[test]
fn memory_forge_is_a_provisioning_forge() {
    fn assert_provisioning<T: temper_forge_model::ProvisioningForge>() {}
    assert_provisioning::<MemoryForge>();
}

fn repo_input(owner: &str, name: &str) -> EnsureRepository {
    EnsureRepository {
        owner: owner.into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
        auto_init: true,
    }
}

fn ensure_repo(forge: &MemoryForge, owner: &str, name: &str) -> RepositoryId {
    block_on(forge.ensure_repository(repo_input(owner, name))).expect("repository ensured")
}

fn new_user(login: &str) -> NewUser {
    NewUser {
        login: login.into(),
        email: format!("{login}@example.test"),
        password: "s3cret".into(),
    }
}

#[test]
fn ensure_repository_is_idempotent_and_require_resolves() {
    let forge = MemoryForge::new();

    let first = ensure_repo(&forge, "acme", "service");
    let second = ensure_repo(&forge, "acme", "service");
    assert_eq!(first, second, "re-ensuring returns the same id");

    let path = RepositoryPath::new("acme", "service");
    let resolved = block_on(forge.require_repository(&path)).expect("require resolves");
    assert_eq!(resolved, first);

    let missing = RepositoryPath::new("acme", "absent");
    let error = block_on(forge.require_repository(&missing)).unwrap_err();
    assert!(matches!(error, temper_forge_model::ForgeError::NotFound(_)));
}

#[test]
fn ensure_user_is_idempotent() {
    let forge = MemoryForge::new();

    block_on(forge.ensure_user(new_user("alice"))).expect("first ensure");
    // A second ensure with different data is a no-op: first write wins.
    block_on(forge.ensure_user(NewUser {
        login: "alice".into(),
        email: "changed@example.test".into(),
        password: "different".into(),
    }))
    .expect("second ensure");

    let users = forge.provisioned_users();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].login, "alice");
    assert_eq!(users[0].email, "alice@example.test");
    assert_eq!(users[0].password, "s3cret");
}

#[test]
fn mint_token_is_deterministic_across_runs() {
    fn mint_three() -> Vec<String> {
        let forge = MemoryForge::new();
        let scopes = [TokenScope::WriteRepository, TokenScope::ReadOrg];
        vec![
            block_on(forge.mint_token("bot", &scopes)).expect("mint 1"),
            block_on(forge.mint_token("bot", &scopes)).expect("mint 2"),
            block_on(forge.mint_token("bot", &scopes)).expect("mint 3"),
        ]
    }

    let first = mint_three();
    let second = mint_three();
    assert_eq!(first, second, "same input yields same output across runs");
    assert_eq!(
        first,
        vec![
            "mem-token-bot-1".to_string(),
            "mem-token-bot-2".to_string(),
            "mem-token-bot-3".to_string(),
        ]
    );
}

#[test]
fn minted_tokens_are_per_login_and_read_back() {
    let forge = MemoryForge::new();
    block_on(forge.mint_token("alice", &[TokenScope::WriteUser])).expect("mint alice");
    block_on(forge.mint_token("bob", &[TokenScope::WriteIssue])).expect("mint bob 1");
    block_on(forge.mint_token("bob", &[TokenScope::WriteIssue])).expect("mint bob 2");

    assert_eq!(forge.minted_tokens("alice"), vec!["mem-token-alice-1"]);
    assert_eq!(
        forge.minted_tokens("bob"),
        vec!["mem-token-bob-1", "mem-token-bob-2"]
    );
    assert!(forge.minted_tokens("carol").is_empty());
}

#[test]
fn ensure_webhook_is_idempotent_on_url() {
    let forge = MemoryForge::new();
    let repo = ensure_repo(&forge, "acme", "service");

    let spec = WebhookSpec {
        url: "https://hooks.test/a".into(),
        secret: "sh".into(),
        events: WebhookEvents::All,
    };
    block_on(forge.ensure_webhook(&repo, spec.clone())).expect("first webhook");
    // Same URL, different secret/events: idempotent on URL, no duplicate.
    block_on(forge.ensure_webhook(
        &repo,
        WebhookSpec {
            url: "https://hooks.test/a".into(),
            secret: "changed".into(),
            events: WebhookEvents::Only(vec!["push".into()]),
        },
    ))
    .expect("second webhook");
    block_on(forge.ensure_webhook(
        &repo,
        WebhookSpec {
            url: "https://hooks.test/b".into(),
            secret: "sh".into(),
            events: WebhookEvents::All,
        },
    ))
    .expect("third webhook");

    let hooks = forge.webhooks(&repo);
    assert_eq!(hooks.len(), 2, "duplicate URL is not registered twice");
    assert_eq!(hooks[0].url, "https://hooks.test/a");
    assert_eq!(hooks[0].secret, "sh", "first write wins on duplicate URL");
    assert_eq!(hooks[1].url, "https://hooks.test/b");
}

#[test]
fn commit_file_and_branch_read_back() {
    let forge = MemoryForge::new();
    let repo = ensure_repo(&forge, "acme", "service");

    block_on(forge.create_branch(
        &repo,
        CreateBranch {
            new_branch: "feature".into(),
            from_branch: "main".into(),
        },
    ))
    .expect("create branch");
    assert!(forge.branch_exists(&repo, "feature"));

    block_on(forge.commit_file(
        &repo,
        CommitFile {
            path: ".forgejo/workflows/ci.yml".into(),
            contents: b"name: ci\n".to_vec(),
            message: "add ci".into(),
            branch: "main".into(),
        },
    ))
    .expect("commit file");

    assert_eq!(
        forge.committed_file(&repo, "main", ".forgejo/workflows/ci.yml"),
        Some(b"name: ci\n".to_vec())
    );
    assert_eq!(forge.committed_file(&repo, "main", "absent"), None);
    assert_eq!(
        forge.committed_file(&repo, "feature", ".forgejo/workflows/ci.yml"),
        None,
        "content is branch-scoped"
    );
    assert!(
        forge.branch_exists(&repo, "main"),
        "commit records its branch"
    );
}

#[test]
fn grant_access_records_repo_collaborator_and_enable_ci() {
    let forge = MemoryForge::new();
    let repo = ensure_repo(&forge, "acme", "service");

    block_on(forge.grant_access(AccessGrant {
        login: "bot".into(),
        repo: repo.clone(),
        scope: AccessScope::RepoCollaborator,
        permission: RepoPermission::Write,
    }))
    .expect("grant");
    // Org-owners grant does not appear in repo collaborator grants.
    block_on(forge.grant_access(AccessGrant {
        login: "admin".into(),
        repo: repo.clone(),
        scope: AccessScope::OrgOwners,
        permission: RepoPermission::Admin,
    }))
    .expect("owners grant");

    let grants = forge.grants(&repo);
    assert_eq!(grants.len(), 1);
    assert_eq!(grants.get("bot"), Some(&RepoPermission::Write));

    assert!(!forge.ci_enabled(&repo));
    block_on(forge.enable_ci(&repo)).expect("enable ci");
    assert!(forge.ci_enabled(&repo));
}
