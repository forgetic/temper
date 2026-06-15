//! Backend-agnostic provisioning orchestration over the Forge capability
//! traits.

use std::collections::BTreeMap;

use temper_forge::{
    AccessGrant, CreateIssue, CreateRepository, Forge, ForgeAdmin, ForgeContent, NewUser,
    ProvisioningForge, RepoPermission, RepositoryId, UpsertLabel,
};

use crate::error::{ProvisionError, Result};
use crate::model::{ProvisionPlan, Provisioned, RoleIdentity, role_email};

/// Provisions a workflow's Forge world from `plan` against the three capability
/// traits.
///
/// The orchestration is backend-agnostic — every step is a trait call — and
/// idempotent end to end, so re-running converges. The steps, in order:
///
/// 1. ensure the repository owner exists;
/// 2. ensure each role user and the automation identity, minting a token for
///    each;
/// 3. ensure (or require, under [`ProvisionPlan::existing_repo`]) the
///    repository;
/// 4. grant each identity access (org Owners-team membership or a repo-scoped
///    collaborator permission, per [`ProvisionPlan::access`]);
/// 5. upsert each label;
/// 6. commit each seed file (skipped under `existing_repo`);
/// 7. enable CI;
/// 8. register the webhook, if any;
/// 9. seed the intake issue, if any.
///
/// Grants are applied *after* the repository exists for both access scopes, so
/// org-Owners membership recorded against the repository owner is well defined
/// on every backend.
pub async fn provision(
    plan: &ProvisionPlan,
    forge: &dyn Forge,
    content: &dyn ForgeContent,
    admin: &dyn ForgeAdmin,
) -> Result<Provisioned> {
    let owner = plan.repo.owner.as_str();
    let name = plan.repo.name.as_str();

    // 1. The owner (organization) must exist before users join it or repos are
    //    created under it.
    admin.ensure_owner(owner).await?;

    // 2. Create the role users and the automation identity, minting each a
    //    token. Access is granted after the repo exists (step 4).
    let mut roles = BTreeMap::new();
    for binding in &plan.roles {
        let login = binding.user.handle.clone();
        let identity = ensure_identity(admin, &login, &plan.password, &plan.token_scopes).await?;
        roles.insert(binding.role.clone(), identity);
    }
    let automation = ensure_identity(
        admin,
        &plan.automation_login,
        &plan.password,
        &plan.token_scopes,
    )
    .await?;

    // 3. `existing_repo` requires a pre-existing repo and must never create one;
    //    the throwaway path creates it.
    let repository = if plan.existing_repo {
        content.require_repository(&plan.repo).await?
    } else {
        content
            .ensure_repository(CreateRepository {
                owner: owner.to_string(),
                name: name.to_string(),
                default_branch: plan.default_branch.clone(),
                description: None,
            })
            .await?
    };

    // 4. Grant each provisioned identity access to the now-existing repo.
    for identity in roles.values() {
        grant_access(admin, &repository, &identity.user, plan.access).await?;
    }
    grant_access(admin, &repository, &automation.user, plan.access).await?;

    // 5. Upsert every label.
    for label in &plan.labels {
        forge
            .upsert_label(
                &repository,
                UpsertLabel {
                    name: label.name.clone(),
                    color: label.color.clone(),
                    description: label.description.clone(),
                },
            )
            .await?;
    }

    // 6. Commit the seed files (e.g. the CI workflow + sentinel) unless this is
    //    an existing repo that owns its own content/history.
    if !plan.existing_repo {
        for seed in &plan.seed_commits {
            content.commit_file(&repository, seed.clone()).await?;
        }
    }

    // 7. Enabling CI is idempotent and wanted in every mode.
    admin.enable_ci(&repository).await?;

    // 8. Register the webhook, if requested.
    if let Some(webhook) = &plan.webhook {
        admin.ensure_webhook(&repository, webhook.clone()).await?;
    }

    let provisioned = Provisioned {
        owner: owner.to_string(),
        name: name.to_string(),
        repository,
        roles,
        automation,
    };

    // 9. Seed the intake issue, if requested, through the supplied `forge`
    //    handle. Find-or-create by title, so re-running converges. The intake
    //    labels were resolved from the workflow at plan-build time.
    if let Some(seed) = &plan.intake {
        seed_intake(forge, &provisioned.repository, seed, &plan.intake_labels).await?;
    }

    Ok(provisioned)
}

/// Files (or reuses) the entry intake issue, returning nothing — the caller's
/// [`Provisioned`] is unchanged.
async fn seed_intake(
    forge: &dyn Forge,
    repo: &RepositoryId,
    seed: &crate::model::IntakeIssueSeed,
    labels: &[String],
) -> Result<()> {
    let existing = forge
        .list_issues(repo, temper_forge::IssueQuery::default())
        .await?;
    if existing.iter().any(|issue| issue.title == seed.title) {
        return Ok(());
    }
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: seed.title.clone(),
                body: seed.body.clone(),
                labels: labels.to_vec(),
                assignees: Vec::new(),
            },
        )
        .await?;
    Ok(())
}

/// Provisions a workflow's Forge world from `plan` against a single backend that
/// implements the full provisioning surface.
///
/// Convenience over [`provision`] for the common case where one backend value
/// provides [`Forge`], [`ForgeContent`], and [`ForgeAdmin`].
pub async fn provision_with<B: ProvisioningForge>(
    plan: &ProvisionPlan,
    backend: &B,
) -> Result<Provisioned> {
    provision(plan, backend, backend, backend).await
}

/// Ensures a user exists and mints a token for it, returning its identity.
async fn ensure_identity(
    admin: &dyn ForgeAdmin,
    login: &str,
    password: &str,
    scopes: &[temper_forge::TokenScope],
) -> Result<RoleIdentity> {
    let email = role_email(login);
    admin
        .ensure_user(NewUser {
            login: login.to_string(),
            email: email.clone(),
            password: password.to_string(),
        })
        .await?;
    let token = admin.mint_token(login, scopes).await?;
    Ok(RoleIdentity {
        user: login.to_string(),
        email,
        token,
        password: password.to_string(),
    })
}

/// Grants `login` access to `repo` under `access`. Repo-collaborator grants use
/// `write`, enough to merge approved, green PRs and read CI status.
async fn grant_access(
    admin: &dyn ForgeAdmin,
    repo: &RepositoryId,
    login: &str,
    access: temper_forge::AccessScope,
) -> Result<()> {
    admin
        .grant_access(AccessGrant {
            login: login.to_string(),
            repo: repo.clone(),
            scope: access,
            permission: RepoPermission::Write,
        })
        .await
        .map_err(ProvisionError::from)
}
