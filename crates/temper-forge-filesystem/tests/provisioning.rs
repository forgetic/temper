//! Behaviour tests for the filesystem provisioning capabilities
//! ([`ForgeContent`] + [`ForgeAdmin`]) and the test-only read-back accessors.
//!
//! These mirror the in-memory backend's provisioning tests exactly so the two
//! reference backends stay in parity, including the deterministic token format.

mod support;

use support::{TestRoot, block_on};
use temper_forge_filesystem::FilesystemForge;
use temper_forge_model::{
    AccessGrant, AccessScope, CommitFile, CreateBranch, EnsureRepository, ForgeAdmin, ForgeContent,
    ForgeError, NewUser, RepoPermission, RepositoryId, RepositoryPath, TokenScope, WebhookEvents,
    WebhookSpec,
};

#[test]
fn filesystem_forge_is_a_provisioning_forge() {
    fn assert_provisioning<T: temper_forge_model::ProvisioningForge>() {}
    assert_provisioning::<FilesystemForge>();
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

fn ensure_repo(forge: &FilesystemForge, owner: &str, name: &str) -> RepositoryId {
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
    let root = TestRoot::new("provisioning");
    let forge = root.forge();

    let first = ensure_repo(&forge, "acme", "service");
    let second = ensure_repo(&forge, "acme", "service");
    assert_eq!(first, second, "re-ensuring returns the same id");

    let path = RepositoryPath::new("acme", "service");
    let resolved = block_on(forge.require_repository(&path)).expect("require resolves");
    assert_eq!(resolved, first);

    let missing = RepositoryPath::new("acme", "absent");
    let error = block_on(forge.require_repository(&missing)).unwrap_err();
    assert!(matches!(error, ForgeError::NotFound(_)));
}

#[test]
fn ensure_user_is_idempotent() {
    let root = TestRoot::new("provisioning");
    let forge = root.forge();

    block_on(forge.ensure_user(new_user("alice"))).expect("first ensure");
    block_on(forge.ensure_user(NewUser {
        login: "alice".into(),
        email: "changed@example.test".into(),
        password: "different".into(),
    }))
    .expect("second ensure");

    let users = forge.provisioned_users().expect("read users");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].login, "alice");
    assert_eq!(users[0].email, "alice@example.test");
    assert_eq!(users[0].password, "s3cret");
}

#[test]
fn mint_token_is_deterministic_across_runs() {
    fn mint_three(root: &TestRoot) -> Vec<String> {
        let forge = root.forge();
        let scopes = [TokenScope::WriteRepository, TokenScope::ReadOrg];
        vec![
            block_on(forge.mint_token("bot", &scopes)).expect("mint 1"),
            block_on(forge.mint_token("bot", &scopes)).expect("mint 2"),
            block_on(forge.mint_token("bot", &scopes)).expect("mint 3"),
        ]
    }

    let first = mint_three(&TestRoot::new("provisioning"));
    let second = mint_three(&TestRoot::new("provisioning"));
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
    let root = TestRoot::new("provisioning");
    let forge = root.forge();
    block_on(forge.mint_token("alice", &[TokenScope::WriteUser])).expect("mint alice");
    block_on(forge.mint_token("bob", &[TokenScope::WriteIssue])).expect("mint bob 1");
    block_on(forge.mint_token("bob", &[TokenScope::WriteIssue])).expect("mint bob 2");

    assert_eq!(
        forge.minted_tokens("alice").expect("read alice tokens"),
        vec!["mem-token-alice-1"]
    );
    assert_eq!(
        forge.minted_tokens("bob").expect("read bob tokens"),
        vec!["mem-token-bob-1", "mem-token-bob-2"]
    );
    assert!(
        forge
            .minted_tokens("carol")
            .expect("read carol tokens")
            .is_empty()
    );
}

#[test]
fn ensure_webhook_is_idempotent_on_url() {
    let root = TestRoot::new("provisioning");
    let forge = root.forge();
    let repo = ensure_repo(&forge, "acme", "service");

    block_on(forge.ensure_webhook(
        &repo,
        WebhookSpec {
            url: "https://hooks.test/a".into(),
            secret: "sh".into(),
            events: WebhookEvents::All,
        },
    ))
    .expect("first webhook");
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

    let hooks = forge.webhooks(&repo).expect("read webhooks");
    assert_eq!(hooks.len(), 2, "duplicate URL is not registered twice");
    assert_eq!(hooks[0].url, "https://hooks.test/a");
    assert_eq!(hooks[0].secret, "sh", "first write wins on duplicate URL");
    assert_eq!(hooks[1].url, "https://hooks.test/b");
}

#[test]
fn commit_file_and_branch_read_back() {
    let root = TestRoot::new("provisioning");
    let forge = root.forge();
    let repo = ensure_repo(&forge, "acme", "service");

    block_on(forge.create_branch(
        &repo,
        CreateBranch {
            new_branch: "feature".into(),
            from_branch: "main".into(),
        },
    ))
    .expect("create branch");
    assert!(
        forge
            .branch_exists(&repo, "feature")
            .expect("branch exists")
    );

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
        forge
            .committed_file(&repo, "main", ".forgejo/workflows/ci.yml")
            .expect("read committed file"),
        Some(b"name: ci\n".to_vec())
    );
    assert_eq!(
        forge
            .committed_file(&repo, "main", "absent")
            .expect("read absent file"),
        None
    );
    assert_eq!(
        forge
            .committed_file(&repo, "feature", ".forgejo/workflows/ci.yml")
            .expect("read other branch"),
        None,
        "content is branch-scoped"
    );
    assert!(
        forge.branch_exists(&repo, "main").expect("main recorded"),
        "commit records its branch"
    );
}

#[test]
fn grant_access_records_repo_collaborator_and_enable_ci() {
    let root = TestRoot::new("provisioning");
    let forge = root.forge();
    let repo = ensure_repo(&forge, "acme", "service");

    block_on(forge.grant_access(AccessGrant {
        login: "bot".into(),
        repo: repo.clone(),
        scope: AccessScope::RepoCollaborator,
        permission: RepoPermission::Write,
    }))
    .expect("grant");
    block_on(forge.grant_access(AccessGrant {
        login: "admin".into(),
        repo: repo.clone(),
        scope: AccessScope::OrgOwners,
        permission: RepoPermission::Admin,
    }))
    .expect("owners grant");

    let grants = forge.grants(&repo).expect("read grants");
    assert_eq!(grants.len(), 1);
    assert_eq!(grants.get("bot"), Some(&RepoPermission::Write));

    assert!(!forge.ci_enabled(&repo).expect("ci not enabled"));
    block_on(forge.enable_ci(&repo)).expect("enable ci");
    assert!(forge.ci_enabled(&repo).expect("ci enabled"));
}
