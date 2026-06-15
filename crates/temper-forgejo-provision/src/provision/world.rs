//! Provisioning the Forgejo "world": org, users, bot, repo, labels, and CI.

use std::collections::BTreeMap;

use temper_forge::{RepositoryId, RepositoryPath};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::RoleBinding;
use temper_workflow::ValidatedWorkflow;

use crate::forgejo_prep::commit_ci_sentinel;
use temper_forgejo_ops::forgejo_rest::{self, ROLE_PASSWORD};

use super::model::{
    AccessScope, CI_WORKFLOW, LABEL_COLOR, ProvisionError, ProvisionOptions, Provisioned,
    REPO_COLLABORATOR_PERMISSION, Result, RoleIdentity, WORKFLOW_PATH,
};

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn provision_world(
    cx: &temper_engine_io::Cx,
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    roles: &[RoleBinding],
    default_branch: &str,
    workflow: &ValidatedWorkflow,
    options: ProvisionOptions,
) -> Result<Provisioned> {
    let client = forgejo_rest::http_client(cx.clone())?;
    // The org must exist under both access scopes; only the Owners-team
    // membership differs.
    forgejo_rest::ensure_org(&client, base_url, admin_token, owner).await?;
    let owners_team = match options.access {
        AccessScope::OrgOwners => {
            Some(forgejo_rest::owners_team_id(&client, base_url, admin_token, owner).await?)
        }
        AccessScope::RepoCollaborator => None,
    };

    // Create the users (and mint their tokens) first. Repo-scoped collaborator
    // grants need the target repo to exist, so under `RepoCollaborator` access
    // is granted after `ensure_repo`/`require_repo` below; under `OrgOwners` the
    // team membership is added here, before the repo, exactly as before.
    let mut role_map = BTreeMap::new();
    for binding in roles {
        let login = binding.user.handle.clone();
        let email = format!("{login}@example.invalid");
        forgejo_rest::create_user(&client, base_url, admin_token, &login, &email).await?;
        if let Some(team) = owners_team {
            forgejo_rest::add_team_member(&client, base_url, admin_token, team, &login).await?;
        }
        let token = forgejo_rest::mint_user_token(&client, base_url, &login).await?;
        role_map.insert(
            binding.role.clone(),
            RoleIdentity {
                user: login,
                email,
                token,
                password: ROLE_PASSWORD.to_string(),
            },
        );
    }

    let automation = provision_bot(&client, base_url, admin_token, owners_team).await?;

    // `--existing-repo` requires a pre-existing repo and must never create one
    // or rewrite its CI/history; the throwaway path creates the repo as before.
    if options.existing_repo {
        forgejo_rest::require_repo(&client, base_url, admin_token, owner, name).await?;
    } else {
        forgejo_rest::ensure_repo(&client, base_url, admin_token, owner, name, default_branch)
            .await?;
    }

    // Under repo-scoped access, grant each identity collaborator `write` on the
    // now-existing repo instead of org ownership.
    if matches!(options.access, AccessScope::RepoCollaborator) {
        grant_repo_collaborators(&client, base_url, admin_token, owner, name, &role_map).await?;
    }

    let repository = upsert_labels(base_url, admin_token, owner, name, workflow).await?;

    // The marker CI commit and the sentinel commit mutate the repo's history;
    // skip both when provisioning onto a repo that owns its own CI. Actions
    // enablement is idempotent and wanted in every mode.
    if !options.existing_repo {
        forgejo_rest::commit_file(
            &client,
            base_url,
            admin_token,
            owner,
            name,
            WORKFLOW_PATH,
            CI_WORKFLOW,
            "add CI workflow (runs-on: host)",
            default_branch,
        )
        .await?;
    }
    forgejo_rest::enable_actions(&client, base_url, admin_token, owner, name).await?;
    if !options.existing_repo {
        commit_ci_sentinel(cx, base_url, admin_token, owner, name, default_branch).await?;
    }

    Ok(Provisioned {
        owner: owner.into(),
        name: name.into(),
        repository,
        roles: role_map,
        automation,
    })
}

/// Creates the `bot` automation identity for the mechanical worker and mints its
/// token. Under `OrgOwners` the bot joins the Owners team so it can land
/// approved PRs and read Actions status over the web UI on Forgejo 7.0.x,
/// keeping the setup-only site admin out of the workflow; under
/// `RepoCollaborator` it gets the same capability scoped to this repo (granted
/// later, once the repo is ensured).
async fn provision_bot(
    client: &forgejo_rest::Client,
    base_url: &str,
    admin_token: &str,
    owners_team: Option<i64>,
) -> Result<RoleIdentity> {
    use super::model::BOT_USER;

    let bot_email = format!("{BOT_USER}@example.invalid");
    forgejo_rest::create_user(client, base_url, admin_token, BOT_USER, &bot_email).await?;
    if let Some(team) = owners_team {
        forgejo_rest::add_team_member(client, base_url, admin_token, team, BOT_USER).await?;
    }
    let bot_token = forgejo_rest::mint_user_token(client, base_url, BOT_USER).await?;
    Ok(RoleIdentity {
        user: BOT_USER.to_string(),
        email: bot_email,
        token: bot_token,
        password: ROLE_PASSWORD.to_string(),
    })
}

/// Grants every role identity and the `bot` repo-scoped collaborator `write` on
/// the target repo.
async fn grant_repo_collaborators(
    client: &forgejo_rest::Client,
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    role_map: &BTreeMap<temper_workflow::RoleId, RoleIdentity>,
) -> Result<()> {
    use super::model::BOT_USER;

    for identity in role_map.values() {
        forgejo_rest::add_repo_collaborator(
            client,
            base_url,
            admin_token,
            owner,
            name,
            &identity.user,
            REPO_COLLABORATOR_PERMISSION,
        )
        .await?;
    }
    forgejo_rest::add_repo_collaborator(
        client,
        base_url,
        admin_token,
        owner,
        name,
        BOT_USER,
        REPO_COLLABORATOR_PERMISSION,
    )
    .await?;
    Ok(())
}

async fn upsert_labels(
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    workflow: &ValidatedWorkflow,
) -> Result<RepositoryId> {
    use temper_forge::UpsertLabel;

    let config = ForgejoConfig::new(base_url, admin_token).with_default_repo(owner, name);
    let forge = ForgejoForge::new(config);
    let repo = forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await?
        .ok_or_else(|| ProvisionError::Shape {
            what: "repository".into(),
            detail: format!("{owner}/{name} not readable after creation"),
        })?;

    let compiled = workflow.compile();
    for label in compiled.labels().labels() {
        forge
            .upsert_label(
                &repo.id,
                UpsertLabel {
                    name: label.id.to_string(),
                    color: Some(LABEL_COLOR.to_string()),
                    description: None,
                },
            )
            .await?;
    }
    Ok(repo.id)
}
