//! Forgejo provisioning used by `temper-provision-forgejo`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use temper_forge::{CreateIssue, IssueQuery, ItemNumber, RepositoryId, RepositoryPath};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::RoleBinding;
use temper_workflow::{ArtifactTarget, Effect, RoleId, ValidatedWorkflow};

use crate::forgejo_prep::commit_ci_sentinel;
use temper_forgejo_ops::forgejo_rest::{self, RestError, ROLE_PASSWORD};
use temper_reference_delivery::{runner_config, workflow};

const WORKFLOW_PATH: &str = ".forgejo/workflows/ci.yml";
const LABEL_COLOR: &str = "#ededed";
/// Automation account the mechanical worker uses for workflow actions such as
/// landing approved, green PRs and reading Forgejo Actions status. It is kept
/// separate from the setup-only site admin so the admin never participates in
/// the workflow.
pub const BOT_USER: &str = "bot";
pub const DEFAULT_INTAKE_TITLE: &str = "Add a configurable greeting to the service banner";
pub const DEFAULT_INTAKE_BODY: &str = "As an operator I want the service banner to show a \
configurable greeting so I can tell environments apart at a glance.\n\n\
Acceptance: a `BANNER_GREETING` setting whose value is printed on startup, \
defaulting to the current text when unset.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeIssueSeed {
    pub title: String,
    pub body: String,
}

impl Default for IntakeIssueSeed {
    fn default() -> Self {
        Self {
            title: DEFAULT_INTAKE_TITLE.into(),
            body: DEFAULT_INTAKE_BODY.into(),
        }
    }
}

// NOTE: this MUST be a raw string. A normal string literal with `\<newline>`
// continuations strips the leading whitespace of each continued source line,
// which is exactly the YAML indentation — the committed workflow then lands
// flush-left, is invalid, and Forgejo silently detects no workflow (no CI runs
// ever fire). See agent lesson 0013.
pub const CI_WORKFLOW: &str = r#"name: ci
on: [push]
jobs:
  build:
    runs-on: host
    steps:
      - name: gate on commit message marker
        run: |
          python3 - <<'PY'
          import json
          import os
          import sys
          import urllib.request

          api = os.environ["GITHUB_API_URL"]
          repo = os.environ["GITHUB_REPOSITORY"]
          sha = os.environ["GITHUB_SHA"]
          token = os.environ["GITHUB_TOKEN"]

          req = urllib.request.Request(
              f"{api}/repos/{repo}/git/commits/{sha}",
              headers={
                  "Authorization": f"token {token}",
                  "Accept": "application/json",
              },
          )
          with urllib.request.urlopen(req, timeout=15) as resp:
              data = json.load(resp)

          msg = data.get("message") or data.get("commit", {}).get("message", "")
          first_line = msg.splitlines()[0] if msg else ""
          print(f"commit {sha}: {first_line}")

          if "[ci-pass]" not in msg:
              print("marker absent; failing")
              sys.exit(1)

          print("marker present; passing")
          PY
"#;

#[derive(Clone)]
pub struct RoleIdentity {
    pub user: String,
    pub email: String,
    pub token: String,
    pub password: String,
}

impl fmt::Debug for RoleIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoleIdentity")
            .field("user", &self.user)
            .field("email", &self.email)
            .field("token", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct Provisioned {
    pub owner: String,
    pub name: String,
    pub repository: RepositoryId,
    pub roles: BTreeMap<RoleId, RoleIdentity>,
    /// The `bot` automation identity used by the mechanical worker.
    pub automation: RoleIdentity,
}

#[derive(Debug)]
pub enum ProvisionError {
    Rest(RestError),
    Forge(temper_forge::ForgeError),
    Shape { what: String, detail: String },
    Io(std::io::Error),
}

impl fmt::Display for ProvisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProvisionError::Rest(err) => write!(formatter, "{err}"),
            ProvisionError::Forge(err) => write!(formatter, "forge operation failed: {err}"),
            ProvisionError::Shape { what, detail } => {
                write!(
                    formatter,
                    "provisioning response '{what}' malformed: {detail}"
                )
            }
            ProvisionError::Io(err) => write!(formatter, "writing secrets file failed: {err}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

impl From<RestError> for ProvisionError {
    fn from(err: RestError) -> Self {
        Self::Rest(err)
    }
}

impl From<temper_forge::ForgeError> for ProvisionError {
    fn from(err: temper_forge::ForgeError) -> Self {
        Self::Forge(err)
    }
}

impl From<std::io::Error> for ProvisionError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

pub type Result<T> = std::result::Result<T, ProvisionError>;

pub async fn provision_world(
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    roles: &[RoleBinding],
    default_branch: &str,
) -> Result<Provisioned> {
    let client = forgejo_rest::http_client()?;
    forgejo_rest::ensure_org(&client, base_url, admin_token, owner).await?;
    let owners_team = forgejo_rest::owners_team_id(&client, base_url, admin_token, owner).await?;

    let mut role_map = BTreeMap::new();
    for binding in roles {
        let login = binding.user.handle.clone();
        let email = format!("{login}@example.invalid");
        forgejo_rest::create_user(&client, base_url, admin_token, &login, &email).await?;
        forgejo_rest::add_team_member(&client, base_url, admin_token, owners_team, &login).await?;
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

    // Automation identity for the mechanical worker. The bot joins the Owners
    // team so it can land approved PRs and read Actions status over the web UI
    // on Forgejo 7.0.x, keeping the setup-only site admin out of the workflow.
    let bot_email = format!("{BOT_USER}@example.invalid");
    forgejo_rest::create_user(&client, base_url, admin_token, BOT_USER, &bot_email).await?;
    forgejo_rest::add_team_member(&client, base_url, admin_token, owners_team, BOT_USER).await?;
    let bot_token = forgejo_rest::mint_user_token(&client, base_url, BOT_USER).await?;
    let automation = RoleIdentity {
        user: BOT_USER.to_string(),
        email: bot_email,
        token: bot_token,
        password: ROLE_PASSWORD.to_string(),
    };

    forgejo_rest::ensure_repo(&client, base_url, admin_token, owner, name, default_branch).await?;
    let repository = upsert_labels(base_url, admin_token, owner, name).await?;
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
    forgejo_rest::enable_actions(&client, base_url, admin_token, owner, name).await?;
    commit_ci_sentinel(base_url, admin_token, owner, name, default_branch).await?;

    Ok(Provisioned {
        owner: owner.into(),
        name: name.into(),
        repository,
        roles: role_map,
        automation,
    })
}

pub async fn seed_intake_issue(
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
    seed: &IntakeIssueSeed,
) -> Result<ItemNumber> {
    let workflow = workflow();
    let labels = intake_labels(&workflow);
    if labels.is_empty() {
        return Err(ProvisionError::Shape {
            what: "intake labels".into(),
            detail: "workflow declares no queued entry issue artifact".into(),
        });
    }

    let config = ForgejoConfig::new(base_url, token).with_default_repo(owner, name);
    let forge = ForgejoForge::new(config);
    let repo = forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await?
        .ok_or_else(|| ProvisionError::Shape {
            what: "repository".into(),
            detail: format!("{owner}/{name} not readable when seeding intake issue"),
        })?;

    let existing = forge.list_issues(&repo.id, IssueQuery::default()).await?;
    if let Some(found) = existing.iter().find(|issue| issue.title == seed.title) {
        return Ok(found.number);
    }

    let issue = forge
        .create_issue(
            &repo.id,
            CreateIssue {
                title: seed.title.clone(),
                body: seed.body.clone(),
                labels,
                assignees: Vec::new(),
            },
        )
        .await?;
    Ok(issue.number)
}

pub fn intake_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    let produced: BTreeSet<&str> = workflow
        .transitions()
        .iter()
        .flat_map(|transition| transition.effects.iter())
        .filter_map(|effect| match effect {
            Effect::AddLabel(label) => Some(label.as_str()),
            _ => None,
        })
        .collect();
    let queue_labels: BTreeSet<&str> = workflow
        .queues()
        .iter()
        .flat_map(|queue| {
            queue
                .labels
                .iter()
                .chain(queue.any_of.iter().flat_map(|set| set.labels.iter()))
        })
        .map(|label| label.as_str())
        .collect();

    workflow
        .artifact_kinds()
        .iter()
        .filter(|kind| kind.target == ArtifactTarget::Issue)
        .find(|kind| {
            !kind.identifying_labels.is_empty()
                && kind.identifying_labels.iter().all(|label| {
                    !produced.contains(label.as_str()) && queue_labels.contains(label.as_str())
                })
        })
        .map(|kind| {
            kind.identifying_labels
                .iter()
                .map(|label| label.to_string())
                .collect()
        })
        .unwrap_or_default()
}

async fn upsert_labels(
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
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

    let workflow = workflow();
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

pub fn format_secrets_env(provisioned: &Provisioned) -> String {
    let mut out = String::new();
    out.push_str("# Generated by temper-provision-forgejo — live credentials, do not commit.\n");
    out.push_str(&format!(
        "TEMPER_FORGEJO_OWNER={}\n",
        sh_quote(&provisioned.owner)
    ));
    out.push_str(&format!(
        "TEMPER_FORGEJO_REPO={}\n",
        sh_quote(&provisioned.name)
    ));
    for (role, identity) in &provisioned.roles {
        let key = env_role_key(role.as_str());
        out.push_str(&format!(
            "TEMPER_FORGEJO_USER_{key}={}\n",
            sh_quote(&identity.user)
        ));
        out.push_str(&format!(
            "TEMPER_FORGEJO_TOKEN_{key}={}\n",
            sh_quote(&identity.token)
        ));
        out.push_str(&format!(
            "TEMPER_FORGEJO_PASSWORD_{key}={}\n",
            sh_quote(&identity.password)
        ));
    }
    // Automation (bot) identity for the mechanical worker. Emitted under a
    // dedicated prefix so launchers never mistake it for a role worker.
    out.push_str(&format!(
        "TEMPER_FORGEJO_BOT_USER={}\n",
        sh_quote(&provisioned.automation.user)
    ));
    out.push_str(&format!(
        "TEMPER_FORGEJO_BOT_TOKEN={}\n",
        sh_quote(&provisioned.automation.token)
    ));
    out.push_str(&format!(
        "TEMPER_FORGEJO_BOT_PASSWORD={}\n",
        sh_quote(&provisioned.automation.password)
    ));
    out
}

pub fn write_secrets_file(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, contents)?;
    restrict_permissions(path)?;
    Ok(())
}

pub async fn provision_and_seed(
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    webhook_url: Option<&str>,
    webhook_secret_file: Option<&Path>,
    intake_seed: Option<&IntakeIssueSeed>,
) -> Result<(Provisioned, Option<ItemNumber>)> {
    let config = runner_config();
    let provisioned = provision_world(
        base_url,
        admin_token,
        owner,
        name,
        &config.role_bindings,
        &config.repository.default_branch,
    )
    .await?;
    if let Some(webhook_url) = webhook_url {
        let Some(secret_file) = webhook_secret_file else {
            return Err(ProvisionError::Shape {
                what: "webhook secret".into(),
                detail: "--webhook-url requires --webhook-secret-file".into(),
            });
        };
        let secret = std::fs::read_to_string(secret_file)?.trim().to_string();
        forgejo_rest::ensure_repo_webhook(
            &forgejo_rest::http_client()?,
            base_url,
            admin_token,
            owner,
            name,
            webhook_url,
            &secret,
        )
        .await?;
    }
    let issue = if let Some(seed) = intake_seed {
        let seed_token = provisioned
            .roles
            .get(&RoleId::new("human"))
            .map(|identity| identity.token.as_str())
            .ok_or_else(|| ProvisionError::Shape {
                what: "intake seed author".into(),
                detail: "workflow provisioning did not create a human role token".into(),
            })?;
        Some(seed_intake_issue(base_url, seed_token, owner, name, seed).await?)
    } else {
        None
    };
    Ok((provisioned, issue))
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn env_role_key(role: &str) -> String {
    role.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_workflow_uses_commit_message_marker() {
        assert!(CI_WORKFLOW.contains("runs-on: host"));
        assert!(CI_WORKFLOW.contains(crate::forgejo_prep::CI_PASS_MARKER));
        assert!(CI_WORKFLOW.contains("GITHUB_SHA"));
        assert!(CI_WORKFLOW.contains("/git/commits/"));
        assert!(!CI_WORKFLOW.contains("github.event.head_commit.message"));
    }

    #[test]
    fn ci_workflow_yaml_is_indented_not_flush_left() {
        // Regression for lesson 0013: a `\<newline>`-continued string literal
        // strips the YAML indentation, producing a flush-left, invalid workflow
        // that Forgejo silently fails to detect. The mapping keys MUST keep
        // their leading spaces.
        assert!(
            CI_WORKFLOW.contains("\n  build:\n"),
            "`build:` must be indented under `jobs:`"
        );
        assert!(
            CI_WORKFLOW.contains("\n    runs-on: host\n"),
            "`runs-on:` must be indented under `build:`"
        );
        assert!(
            CI_WORKFLOW.contains("\n    steps:\n"),
            "`steps:` must be indented under `build:`"
        );
        // No job-level key should appear flush-left (column 0).
        assert!(!CI_WORKFLOW.contains("\nbuild:\n"));
        assert!(!CI_WORKFLOW.contains("\nruns-on:"));
    }

    #[test]
    fn intake_labels_resolve_to_entry_issue() {
        assert_eq!(intake_labels(&workflow()), vec!["untriaged".to_string()]);
    }

    #[test]
    fn secrets_env_escapes_single_quotes() {
        let mut roles = BTreeMap::new();
        roles.insert(
            RoleId::new("code-reviewer"),
            RoleIdentity {
                user: "reviewer".into(),
                email: "reviewer@example.invalid".into(),
                token: "tok".into(),
                password: "pw-with-'-quote".into(),
            },
        );
        let env = format_secrets_env(&Provisioned {
            owner: "acme".into(),
            name: "service".into(),
            repository: RepositoryId::new("r"),
            roles,
            automation: RoleIdentity {
                user: BOT_USER.into(),
                email: "bot@example.invalid".into(),
                token: "bot-tok".into(),
                password: "bot-pw".into(),
            },
        });
        assert!(env.contains("TEMPER_FORGEJO_TOKEN_CODE_REVIEWER='tok'"));
        assert!(env.contains(r"TEMPER_FORGEJO_PASSWORD_CODE_REVIEWER='pw-with-'\''-quote'"));
        assert!(env.contains("TEMPER_FORGEJO_BOT_USER='bot'"));
        assert!(env.contains("TEMPER_FORGEJO_BOT_TOKEN='bot-tok'"));
        assert!(env.contains("TEMPER_FORGEJO_BOT_PASSWORD='bot-pw'"));
    }
}
