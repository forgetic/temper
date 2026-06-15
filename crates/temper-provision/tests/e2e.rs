//! Network-free end-to-end provisioning tests.
//!
//! These drive the backend-agnostic [`provision`] orchestration against both
//! reference backends — the in-memory and filesystem forges — and assert via
//! the backends' read-back accessors that every step ran: role users and the
//! bot were created with tokens, labels were upserted, the webhook was
//! registered, the CI seed file was committed, grants were recorded, and the
//! intake issue was filed when requested.
//!
//! The two backends' futures never park, so a hand-rolled `block_on` drives the
//! async orchestration to completion without an async runtime.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use temper_forge::{
    AccessScope, CommitFile, Forge, ForgeContent, IssueQuery, RepoPermission, RepositoryId,
    RepositoryPath, TokenScope, WebhookEvents, WebhookSpec,
};
use temper_forge_filesystem::FilesystemForge;
use temper_forge_memory::MemoryForge;
use temper_provision::{
    BOT_USER, IntakeIssueSeed, LabelSpec, ProvisionOptions, ProvisionPlan, Provisioned,
    provision_with,
};
use temper_runner::RoleBinding;
use temper_workflow::ValidatedWorkflow;

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
        Poll::Pending => panic!("reference-backend provisioning futures should not park"),
    }
}

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// A self-cleaning temp directory for the filesystem backend.
struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new() -> Self {
        let id = NEXT_ROOT.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("temper-provision-e2e-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const OWNER: &str = "acme";
const NAME: &str = "service";
const BRANCH: &str = "main";
const CI_PATH: &str = ".forgejo/workflows/ci.yml";
const CI_BODY: &[u8] = b"name: ci\non: [push]\n";
const WEBHOOK_URL: &str = "http://127.0.0.1:9999/forgejo/webhook";

fn workflow() -> ValidatedWorkflow {
    temper_reference_delivery::workflow()
}

/// Role bindings derived from the reference workflow (each queue-subscribing
/// role bound to a Forge user whose handle equals the role id).
fn role_bindings() -> Vec<RoleBinding> {
    temper_reference_delivery::runner_config_for(
        &workflow(),
        temper_reference_delivery::repo_input(),
    )
    .role_bindings
}

/// Builds a plan that provisions a fresh repo, seeds a CI file, registers a
/// webhook, and files an intake issue.
fn full_plan() -> ProvisionPlan {
    let options = ProvisionOptions {
        existing_repo: false,
        roles: role_bindings(),
        automation_login: BOT_USER.to_string(),
        password: "s3cret".into(),
        token_scopes: vec![
            TokenScope::WriteRepository,
            TokenScope::WriteIssue,
            TokenScope::WriteUser,
            TokenScope::ReadOrg,
        ],
        labels: vec![LabelSpec {
            name: "extra-host-label".into(),
            color: Some("#ff0000".into()),
            description: Some("supplied by the adapter".into()),
        }],
        seed_commits: vec![CommitFile {
            path: CI_PATH.into(),
            contents: CI_BODY.to_vec(),
            message: "add CI workflow".into(),
            branch: BRANCH.into(),
        }],
        webhook: Some(WebhookSpec {
            url: WEBHOOK_URL.into(),
            secret: "s3cr3t".into(),
            events: WebhookEvents::All,
        }),
        intake: Some(IntakeIssueSeed::new(
            "Add a configurable greeting",
            "As an operator I want a configurable banner greeting.",
        )),
    };
    ProvisionPlan::from_workflow(
        &workflow(),
        RepositoryPath::new(OWNER, NAME),
        BRANCH.into(),
        AccessScope::RepoCollaborator,
        options,
    )
    .expect("plan builds from the reference workflow")
}

/// Asserts the post-provision state via the backend's read-back accessors. The
/// accessor results are passed in already unwrapped so this works for both the
/// direct (memory) and `ForgeResult` (filesystem) accessor flavors.
struct ProvisionedState {
    provisioned: Provisioned,
    repo: RepositoryId,
    user_logins: Vec<String>,
    minted_for_engineer: Vec<String>,
    ci_committed: Option<Vec<u8>>,
    webhook_urls: Vec<String>,
    grants: std::collections::BTreeMap<String, RepoPermission>,
    ci_enabled: bool,
    intake_titles: Vec<String>,
}

fn assert_full(state: &ProvisionedState) {
    // Every bound role user plus the bot was provisioned.
    for binding in role_bindings() {
        assert!(
            state.user_logins.contains(&binding.user.handle),
            "role user {} should be provisioned (have {:?})",
            binding.user.handle,
            state.user_logins
        );
    }
    assert!(
        state.user_logins.contains(&BOT_USER.to_string()),
        "the bot should be provisioned"
    );

    // Tokens were minted for each role and the bot.
    assert_eq!(
        state.provisioned.automation.user, BOT_USER,
        "automation identity is the bot"
    );
    assert!(
        !state.provisioned.automation.token.is_empty(),
        "bot token minted"
    );
    for identity in state.provisioned.roles.values() {
        assert!(!identity.token.is_empty(), "role token minted");
    }
    // At least one role gets a token, and the engineer (a queue-subscribing
    // role in the reference workflow) had a token minted.
    assert!(
        !state.minted_for_engineer.is_empty(),
        "engineer should have a minted token"
    );

    // Labels were upserted: the workflow labels plus the host-supplied one.
    // (Label read-back is asserted indirectly through the live forge below.)

    // CI seed file landed on the default branch.
    assert_eq!(
        state.ci_committed.as_deref(),
        Some(CI_BODY),
        "CI workflow file should be committed"
    );

    // Webhook registered.
    assert!(
        state.webhook_urls.contains(&WEBHOOK_URL.to_string()),
        "webhook should be registered (have {:?})",
        state.webhook_urls
    );

    // Repo-collaborator grants recorded for each role and the bot at `write`.
    for binding in role_bindings() {
        assert_eq!(
            state.grants.get(&binding.user.handle),
            Some(&RepoPermission::Write),
            "role {} should have repo-scoped write",
            binding.user.handle
        );
    }
    assert_eq!(
        state.grants.get(BOT_USER),
        Some(&RepoPermission::Write),
        "bot should have repo-scoped write"
    );

    // CI enabled.
    assert!(state.ci_enabled, "CI should be enabled");

    // Intake issue filed.
    assert!(
        state
            .intake_titles
            .iter()
            .any(|title| title == "Add a configurable greeting"),
        "intake issue should be filed (have {:?})",
        state.intake_titles
    );

    // The repository field is populated.
    assert_eq!(state.repo, state.provisioned.repository);
}

#[test]
fn provision_against_memory_backend() {
    let forge = MemoryForge::new();
    let plan = full_plan();
    let provisioned = block_on(provision_with(&plan, &forge)).expect("provision succeeds");

    let repo = provisioned.repository.clone();
    let user_logins: Vec<String> = forge
        .provisioned_users()
        .into_iter()
        .map(|user| user.login)
        .collect();
    let webhook_urls: Vec<String> = forge
        .webhooks(&repo)
        .into_iter()
        .map(|hook| hook.url)
        .collect();
    let intake_titles = block_on(forge.list_issues(&repo, IssueQuery::default()))
        .expect("list issues")
        .into_iter()
        .map(|issue| issue.title)
        .collect();

    let state = ProvisionedState {
        repo: repo.clone(),
        user_logins,
        minted_for_engineer: forge.minted_tokens("engineer"),
        ci_committed: forge.committed_file(&repo, BRANCH, CI_PATH),
        webhook_urls,
        grants: forge.grants(&repo),
        ci_enabled: forge.ci_enabled(&repo),
        intake_titles,
        provisioned,
    };
    assert_full(&state);

    // The host-supplied label and a workflow label are both present.
    let label_names: Vec<String> = block_on(forge.list_labels(&repo))
        .expect("list labels")
        .into_iter()
        .map(|label| label.name)
        .collect();
    assert!(
        label_names.contains(&"extra-host-label".to_string()),
        "host-supplied label upserted (have {label_names:?})"
    );
    assert!(
        label_names.len() >= plan.labels.len(),
        "every plan label upserted"
    );
}

#[test]
fn provision_against_filesystem_backend() {
    let root = TestRoot::new();
    let forge = root.forge();
    let plan = full_plan();
    let provisioned = block_on(provision_with(&plan, &forge)).expect("provision succeeds");

    let repo = provisioned.repository.clone();
    let user_logins: Vec<String> = forge
        .provisioned_users()
        .expect("provisioned users")
        .into_iter()
        .map(|user| user.login)
        .collect();
    let webhook_urls: Vec<String> = forge
        .webhooks(&repo)
        .expect("webhooks")
        .into_iter()
        .map(|hook| hook.url)
        .collect();
    let intake_titles = block_on(forge.list_issues(&repo, IssueQuery::default()))
        .expect("list issues")
        .into_iter()
        .map(|issue| issue.title)
        .collect();

    let state = ProvisionedState {
        repo: repo.clone(),
        user_logins,
        minted_for_engineer: forge.minted_tokens("engineer").expect("minted tokens"),
        ci_committed: forge
            .committed_file(&repo, BRANCH, CI_PATH)
            .expect("committed file"),
        webhook_urls,
        grants: forge.grants(&repo).expect("grants"),
        ci_enabled: forge.ci_enabled(&repo).expect("ci enabled"),
        intake_titles,
        provisioned,
    };
    assert_full(&state);

    let label_names: Vec<String> = block_on(forge.list_labels(&repo))
        .expect("list labels")
        .into_iter()
        .map(|label| label.name)
        .collect();
    assert!(
        label_names.contains(&"extra-host-label".to_string()),
        "host-supplied label upserted (have {label_names:?})"
    );
}

#[test]
fn existing_repo_requires_repo_and_skips_seed_commits() {
    // `existing_repo` against a missing repo must error (require_repository).
    let forge = MemoryForge::new();
    let options = ProvisionOptions {
        existing_repo: true,
        roles: role_bindings(),
        automation_login: BOT_USER.to_string(),
        password: "s3cret".into(),
        token_scopes: Vec::new(),
        labels: Vec::new(),
        seed_commits: vec![CommitFile {
            path: CI_PATH.into(),
            contents: CI_BODY.to_vec(),
            message: "add CI workflow".into(),
            branch: BRANCH.into(),
        }],
        webhook: None,
        intake: None,
    };
    let plan = ProvisionPlan::from_workflow(
        &workflow(),
        RepositoryPath::new(OWNER, NAME),
        BRANCH.into(),
        AccessScope::RepoCollaborator,
        options,
    )
    .expect("plan builds");
    let error =
        block_on(provision_with(&plan, &forge)).expect_err("existing repo absent must error");
    assert!(matches!(
        error,
        temper_provision::ProvisionError::Backend(_)
    ));

    // Now pre-create the repo, then provision: the seed CI commit must be
    // skipped under `existing_repo`.
    let forge = MemoryForge::new();
    let repo = block_on(forge.ensure_repository(temper_forge::CreateRepository {
        owner: OWNER.into(),
        name: NAME.into(),
        default_branch: BRANCH.into(),
        description: None,
    }))
    .expect("pre-create repo");
    let provisioned = block_on(provision_with(&plan, &forge)).expect("provision succeeds");
    assert_eq!(provisioned.repository, repo);
    assert!(
        forge.committed_file(&repo, BRANCH, CI_PATH).is_none(),
        "existing_repo must not commit the seed CI file"
    );
    // CI is still enabled even for an existing repo.
    assert!(forge.ci_enabled(&repo), "CI enabled for existing repo");
}

#[test]
fn org_owners_access_records_owners_membership_not_repo_grants() {
    let forge = MemoryForge::new();
    let options = ProvisionOptions {
        existing_repo: false,
        roles: role_bindings(),
        automation_login: BOT_USER.to_string(),
        password: "s3cret".into(),
        token_scopes: Vec::new(),
        labels: Vec::new(),
        seed_commits: Vec::new(),
        webhook: None,
        intake: None,
    };
    let plan = ProvisionPlan::from_workflow(
        &workflow(),
        RepositoryPath::new(OWNER, NAME),
        BRANCH.into(),
        AccessScope::OrgOwners,
        options,
    )
    .expect("plan builds");
    let provisioned = block_on(provision_with(&plan, &forge)).expect("provision succeeds");
    // Under OrgOwners access, no repo-scoped collaborator grants are recorded
    // (membership goes through the org Owners team instead).
    assert!(
        forge.grants(&provisioned.repository).is_empty(),
        "OrgOwners access should not record repo collaborator grants"
    );
}
