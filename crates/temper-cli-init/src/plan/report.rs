use std::collections::BTreeSet;

use serde::Serialize;
use temper_cli_common::OutputFormat;
use temper_forge::WebhookEvents;
use temper_provision::BOT_USER;

use super::{DeploymentInspector, ForgeInspection, PlanBundle, desired_users, non_empty};

/// Top-level JSON/human report for `temper plan`.
#[derive(Clone, Debug, Serialize)]
pub struct DeploymentPlanReport {
    pub status: String,
    pub result: String,
    pub loaded: LoadedReport,
    pub deployment: DeploymentReport,
    pub forge: ForgeReport,
    pub repository: RepositoryReport,
    pub workflow: WorkflowReport,
    pub labels: Vec<LabelReport>,
    pub webhook: WebhookReport,
    pub identities: IdentityReport,
    pub metadata: MetadataReport,
    pub findings: Vec<PlanFinding>,
}

impl DeploymentPlanReport {
    pub(super) fn has_error_findings(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == "error")
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LoadedReport {
    pub config_path: Option<String>,
    pub credentials_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeploymentReport {
    pub name: Option<String>,
    pub topology: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ForgeReport {
    pub kind: String,
    pub url: String,
    pub inspected: bool,
    pub inspection_note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RepositoryReport {
    pub path: String,
    pub existing_repo_required: bool,
    pub exists: Option<bool>,
    pub id: Option<String>,
    pub default_branch: String,
    pub ci_enabled: Option<bool>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkflowReport {
    pub name: String,
    pub path: Option<String>,
    pub roles: Vec<String>,
    pub queued_roles: Vec<String>,
    pub labels: usize,
    pub artifact_kinds: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LabelReport {
    pub name: String,
    pub present: Option<bool>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebhookReport {
    pub url: String,
    pub secret: String,
    pub events: String,
    pub configured: Option<bool>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentityReport {
    pub admin: AdminIdentityReport,
    pub automation: AutomationIdentityReport,
    pub roles: Vec<RoleIdentityReport>,
    pub users: Vec<UserReadinessReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AdminIdentityReport {
    pub key: String,
    pub user: String,
    pub password: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AutomationIdentityReport {
    pub user: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoleIdentityReport {
    pub role: String,
    pub user: String,
    pub email: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct UserReadinessReport {
    pub user: String,
    pub present: Option<bool>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetadataReport {
    pub compatible: bool,
    pub checked_artifacts: usize,
    pub invalid: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanFinding {
    pub severity: String,
    pub category: String,
    pub message: String,
}

pub(super) fn build_report(
    bundle: &PlanBundle,
    inspector: &mut dyn DeploymentInspector,
) -> Result<DeploymentPlanReport, String> {
    let inspection = match inspector.inspect(bundle) {
        Ok(inspection) => inspection,
        Err(error) => ForgeInspection {
            inspected: false,
            unavailable_reason: Some(format!("forge inspection failed: {error}")),
            ..ForgeInspection::default()
        },
    };
    let mut findings = Vec::new();
    if let Some(reason) = &inspection.unavailable_reason {
        findings.push(note("forge", reason));
    }

    let present_labels: BTreeSet<String> = inspection.labels.iter().cloned().collect();
    let labels = bundle
        .provision_plan
        .labels
        .iter()
        .map(|label| {
            let present = inspection
                .repository
                .as_ref()
                .map(|_| present_labels.contains(&label.name));
            LabelReport {
                name: label.name.clone(),
                present,
                action: match present {
                    Some(true) => "none".to_string(),
                    Some(false) => "upsert".to_string(),
                    None => "unknown".to_string(),
                },
            }
        })
        .collect::<Vec<_>>();

    let repository_action = match (inspection.repository.as_ref(), bundle.request.existing_repo) {
        (Some(_), _) => "none",
        (None, true) if inspection.inspected => "require_existing",
        (None, false) if inspection.inspected => "create",
        (None, _) => "unknown",
    };
    if inspection.inspected && inspection.repository.is_none() && bundle.request.existing_repo {
        findings.push(error(
            "repository",
            format!(
                "repository {}/{} is required by --existing-repo but was not found",
                bundle.request.owner, bundle.request.name
            ),
        ));
    }

    let desired_webhook = bundle
        .provision_plan
        .webhook
        .as_ref()
        .ok_or_else(|| "deployment plan did not contain a webhook".to_string())?;
    let webhook_configured = inspection.repository.as_ref().map(|_| {
        inspection
            .webhooks
            .iter()
            .any(|webhook| webhook.url == desired_webhook.url)
    });
    let webhook = WebhookReport {
        url: desired_webhook.url.clone(),
        secret: "<redacted>".to_string(),
        events: webhook_events_label(&desired_webhook.events),
        configured: webhook_configured,
        action: match webhook_configured {
            Some(true) => "none".to_string(),
            Some(false) => "register".to_string(),
            None => "unknown".to_string(),
        },
    };

    for invalid in &inspection.metadata.invalid {
        findings.push(error("metadata", invalid.clone()));
    }

    let identities = identity_report(bundle, &inspection);
    let workflow = workflow_report(bundle);
    let compatible = inspection.metadata.invalid.is_empty();
    let status = if findings.iter().any(|finding| finding.severity == "error") {
        "needs_attention"
    } else {
        "ready"
    }
    .to_string();

    Ok(DeploymentPlanReport {
        result: if status == "ready" { "ok" } else { "error" }.to_string(),
        status,
        loaded: LoadedReport {
            config_path: bundle
                .loaded
                .config
                .as_ref()
                .map(|path| path.display().to_string()),
            credentials_path: bundle
                .loaded
                .credentials
                .as_ref()
                .map(|path| path.display().to_string()),
        },
        deployment: DeploymentReport {
            name: bundle.resolved.deployment.name.clone(),
            topology: bundle
                .resolved
                .deployment
                .topology
                .map(|topology| topology.as_str().to_string()),
        },
        forge: ForgeReport {
            kind: "forgejo".to_string(),
            url: bundle.request.base_url.clone(),
            inspected: inspection.inspected,
            inspection_note: inspection.unavailable_reason.clone(),
        },
        repository: RepositoryReport {
            path: format!("{}/{}", bundle.request.owner, bundle.request.name),
            existing_repo_required: bundle.request.existing_repo,
            exists: if inspection.inspected {
                Some(inspection.repository.is_some())
            } else {
                None
            },
            id: inspection
                .repository
                .as_ref()
                .map(|repository| repository.id.as_str().to_string()),
            default_branch: bundle.provision_plan.default_branch.clone(),
            ci_enabled: inspection.ci_enabled,
            action: repository_action.to_string(),
        },
        workflow,
        labels,
        webhook,
        identities,
        metadata: MetadataReport {
            compatible,
            checked_artifacts: inspection.metadata.checked_artifacts,
            invalid: inspection.metadata.invalid,
        },
        findings,
    })
}

fn workflow_report(bundle: &PlanBundle) -> WorkflowReport {
    let compiled = bundle.workflow.compile();
    WorkflowReport {
        name: bundle.workflow.name().to_string(),
        path: bundle
            .request
            .workflow_path
            .as_ref()
            .map(|path| path.display().to_string()),
        roles: bundle
            .workflow
            .roles()
            .iter()
            .map(|role| role.id.as_str().to_string())
            .collect(),
        queued_roles: bundle
            .workflow
            .roles()
            .iter()
            .filter(|role| !role.queues.is_empty())
            .map(|role| role.id.as_str().to_string())
            .collect(),
        labels: compiled.labels().labels().len(),
        artifact_kinds: bundle
            .workflow
            .artifact_kinds()
            .iter()
            .map(|kind| kind.id.as_str().to_string())
            .collect(),
    }
}

fn identity_report(bundle: &PlanBundle, inspection: &ForgeInspection) -> IdentityReport {
    let admin_user = bundle.credentials.forge.users.get(&bundle.admin_key);
    let role_tokens = &bundle.resolved.forge.role_tokens;
    let roles = bundle
        .provision_plan
        .roles
        .iter()
        .map(|binding| RoleIdentityReport {
            role: binding.role.as_str().to_string(),
            user: binding.user.handle.clone(),
            email: binding
                .user
                .email
                .clone()
                .unwrap_or_else(|| format!("{}@example.invalid", binding.user.handle)),
            token: if role_tokens.contains_key(binding.role.as_str()) {
                "present"
            } else {
                "will_mint"
            }
            .to_string(),
        })
        .collect::<Vec<_>>();
    let users = desired_users(bundle)
        .into_iter()
        .map(|user| {
            let present = if inspection.inspected {
                Some(inspection.users.get(&user).copied().unwrap_or(false))
            } else {
                None
            };
            UserReadinessReport {
                user,
                present,
                action: match present {
                    Some(true) => "none".to_string(),
                    Some(false) => "create_or_reuse".to_string(),
                    None => "unknown".to_string(),
                },
            }
        })
        .collect();
    IdentityReport {
        admin: AdminIdentityReport {
            key: bundle.admin_key.clone(),
            user: bundle.admin_user.clone(),
            password: if admin_user
                .and_then(|user| non_empty(user.password.as_deref()))
                .is_some()
            {
                "set"
            } else {
                "missing"
            }
            .to_string(),
            token: if bundle.resolved.forge.admin_token.is_some() {
                "present"
            } else {
                "will_mint"
            }
            .to_string(),
        },
        automation: AutomationIdentityReport {
            user: if bundle.provision_plan.automation_login.is_empty() {
                BOT_USER.to_string()
            } else {
                bundle.provision_plan.automation_login.clone()
            },
            token: "will_mint".to_string(),
        },
        roles,
        users,
    }
}

fn webhook_events_label(events: &WebhookEvents) -> String {
    match events {
        WebhookEvents::All => "all".to_string(),
        WebhookEvents::Only(events) => events.join(","),
    }
}

fn note(category: &str, message: impl Into<String>) -> PlanFinding {
    PlanFinding {
        severity: "note".to_string(),
        category: category.to_string(),
        message: message.into(),
    }
}

fn error(category: &str, message: impl Into<String>) -> PlanFinding {
    PlanFinding {
        severity: "error".to_string(),
        category: category.to_string(),
        message: message.into(),
    }
}

pub(super) fn print_report(
    report: &DeploymentPlanReport,
    format: OutputFormat,
) -> Result<(), String> {
    match format {
        OutputFormat::Json => {
            let text = serde_json::to_string_pretty(report)
                .map_err(|error| format!("serialize plan report: {error}"))?;
            println!("{text}");
        }
        OutputFormat::Human => print_human(report),
    }
    Ok(())
}

fn print_human(report: &DeploymentPlanReport) {
    println!("Deployment plan: {}", report.status);
    if let Some(path) = &report.loaded.config_path {
        println!("config:      {path}");
    }
    if let Some(path) = &report.loaded.credentials_path {
        println!("credentials: {path}");
    }
    println!();
    println!("Forge");
    println!("  url:       {}", report.forge.url);
    println!(
        "  inspected: {}",
        if report.forge.inspected { "yes" } else { "no" }
    );
    if let Some(note) = &report.forge.inspection_note {
        println!("  note:      {note}");
    }
    println!();
    println!("Repository");
    println!("  path:      {}", report.repository.path);
    println!("  action:    {}", report.repository.action);
    if let Some(exists) = report.repository.exists {
        println!("  exists:    {exists}");
    }
    if let Some(ci) = report.repository.ci_enabled {
        println!("  ci:        {}", if ci { "enabled" } else { "disabled" });
    }
    println!();
    println!("Workflow");
    println!("  name:      {}", report.workflow.name);
    println!("  labels:    {}", report.workflow.labels);
    println!("  roles:     {}", report.workflow.roles.join(", "));
    println!();
    println!("Labels");
    for label in &report.labels {
        println!("  {:<8} {}", label.action, label.name);
    }
    println!();
    println!("Webhook");
    println!("  url:       {}", report.webhook.url);
    println!("  secret:    {}", report.webhook.secret);
    println!("  action:    {}", report.webhook.action);
    println!();
    println!("Identities");
    println!(
        "  admin:    {} (password {}, token {})",
        report.identities.admin.user,
        report.identities.admin.password,
        report.identities.admin.token
    );
    println!(
        "  bot:      {} (token {})",
        report.identities.automation.user, report.identities.automation.token
    );
    for role in &report.identities.roles {
        println!(
            "  role:     {} -> {} <{}> (token {})",
            role.role, role.user, role.email, role.token
        );
    }
    println!();
    println!("Workflow metadata");
    println!(
        "  compatible: {} (checked {} artifact(s))",
        if report.metadata.compatible {
            "yes"
        } else {
            "no"
        },
        report.metadata.checked_artifacts
    );
    for invalid in &report.metadata.invalid {
        println!("  error:     {invalid}");
    }
    if !report.findings.is_empty() {
        println!();
        println!("Findings");
        for finding in &report.findings {
            println!(
                "  {} [{}] {}",
                finding.severity, finding.category, finding.message
            );
        }
    }
}
