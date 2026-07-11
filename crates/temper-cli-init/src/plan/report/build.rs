// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};

use temper_forge::WebhookEvents;
use temper_provision::BOT_USER;

use crate::deployment::{DeploymentBundle, DesiredRepository};

use super::model::*;
use crate::plan::inspection::{DeploymentInspector, ForgeInspection, desired_users};

pub(in crate::plan) fn build_report(
    bundle: &DeploymentBundle,
    inspector: &mut dyn DeploymentInspector,
) -> Result<DeploymentPlanReport, String> {
    let desired_webhook = bundle
        .webhook
        .as_ref()
        .ok_or_else(|| "deployment plan did not contain a webhook".to_string())?;

    // Never short-circuit this loop: every configured repository gets an
    // inspection attempt and a report entry even when an earlier call fails.
    let inspections = bundle
        .repositories
        .iter()
        .map(
            |repository| match inspector.inspect_repository(bundle, repository) {
                Ok(inspection) => inspection,
                Err(error) => ForgeInspection {
                    unavailable_reason: Some(format!("forge inspection failed: {error}")),
                    ..ForgeInspection::default()
                },
            },
        )
        .collect::<Vec<_>>();

    let users = desired_users(bundle);
    let (user_readiness, user_inspection_error) = match inspector.inspect_users(bundle, &users) {
        Ok(readiness) => (Some(readiness), None),
        Err(error) => (None, Some(format!("identity inspection failed: {error}"))),
    };

    let mut repositories = Vec::with_capacity(bundle.repositories.len());
    let mut findings = Vec::new();
    let mut inspection_notes = Vec::new();
    for (desired, inspection) in bundle.repositories.iter().zip(&inspections) {
        let path = repository_path(desired);
        let entry = repository_report(desired, inspection, desired_webhook);
        if let Some(reason) = &inspection.unavailable_reason {
            inspection_notes.push(format!("{path}: {reason}"));
        }
        findings.extend(entry.findings.iter().cloned().map(|mut finding| {
            finding.message = format!("{path}: {}", finding.message);
            finding
        }));
        repositories.push(entry);
    }
    if let Some(error) = &user_inspection_error {
        inspection_notes.push(error.clone());
        findings.push(note("identity", error));
    }

    let all_inspected = inspections.iter().all(|inspection| inspection.inspected)
        && user_inspection_error.is_none();
    let status = if findings.iter().any(|finding| finding.severity == "error") {
        "needs_attention"
    } else {
        "ready"
    }
    .to_string();

    let (repository, labels, webhook, metadata) = if repositories.len() == 1 {
        let only = &repositories[0];
        (
            Some(only.repository.clone()),
            Some(only.labels.clone()),
            Some(only.webhook.clone()),
            Some(only.metadata.clone()),
        )
    } else {
        (None, None, None, None)
    };

    Ok(DeploymentPlanReport {
        report_version: REPORT_VERSION,
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
            url: bundle.forge.base_url.clone(),
            inspected: all_inspected,
            inspection_note: (!inspection_notes.is_empty()).then(|| inspection_notes.join("; ")),
        },
        repositories,
        repository,
        workflow: workflow_report(bundle),
        labels,
        webhook,
        identities: identity_report(bundle, user_readiness.as_ref()),
        metadata,
        findings,
    })
}

fn repository_report(
    desired: &DesiredRepository,
    inspection: &ForgeInspection,
    desired_webhook: &crate::deployment::DesiredWebhook,
) -> RepositoryPlanReport {
    let plan = &desired.plan;
    let mut findings = Vec::new();
    if let Some(reason) = &inspection.unavailable_reason {
        findings.push(note("forge", reason));
    }

    let present_labels: BTreeSet<String> = inspection.labels.iter().cloned().collect();
    let labels = plan
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
                    Some(true) => "none",
                    Some(false) => "upsert",
                    None => "unknown",
                }
                .to_string(),
            }
        })
        .collect();

    let repository_action = match (inspection.repository.as_ref(), plan.existing_repo) {
        (Some(_), _) => "none",
        (None, true) if inspection.inspected => "require_existing",
        (None, false) if inspection.inspected => "create",
        (None, _) => "unknown",
    };
    if inspection.inspected && inspection.repository.is_none() && plan.existing_repo {
        findings.push(error(
            "repository",
            format!(
                "repository {}/{} is required by --existing-repo but was not found",
                plan.repo.owner, plan.repo.name
            ),
        ));
    }

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
            Some(true) => "none",
            Some(false) => "register",
            None => "unknown",
        }
        .to_string(),
    };

    for invalid in &inspection.metadata.invalid {
        findings.push(error("metadata", invalid));
    }

    RepositoryPlanReport {
        repository: RepositoryReport {
            path: repository_path(desired),
            existing_repo_required: plan.existing_repo,
            exists: inspection
                .inspected
                .then_some(inspection.repository.is_some()),
            id: inspection
                .repository
                .as_ref()
                .map(|repository| repository.id.as_str().to_string()),
            default_branch: plan.default_branch.clone(),
            ci_enabled: inspection.ci_enabled,
            action: repository_action.to_string(),
        },
        labels,
        webhook,
        metadata: MetadataReport {
            compatible: inspection.metadata.invalid.is_empty(),
            checked_artifacts: inspection.metadata.checked_artifacts,
            invalid: inspection.metadata.invalid.clone(),
        },
        findings,
    }
}

fn repository_path(repository: &DesiredRepository) -> String {
    format!(
        "{}/{}",
        repository.plan.repo.owner, repository.plan.repo.name
    )
}

fn workflow_report(bundle: &DeploymentBundle) -> WorkflowReport {
    let compiled = bundle.workflow.compile();
    WorkflowReport {
        name: bundle.workflow.name().to_string(),
        path: bundle
            .resolved
            .engine
            .workflow_file
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

fn identity_report(
    bundle: &DeploymentBundle,
    readiness: Option<&BTreeMap<String, bool>>,
) -> IdentityReport {
    let admin_key = bundle.admin_key.as_deref().unwrap_or("<none>");
    let admin_user = bundle.credentials.forge.users.get(admin_key);
    let role_tokens = &bundle.resolved.forge.role_tokens;
    let mut seen_roles = BTreeSet::new();
    let roles = bundle
        .repositories
        .iter()
        .flat_map(|repository| &repository.plan.roles)
        .filter(|binding| {
            seen_roles.insert((
                binding.role.as_str().to_string(),
                binding.user.handle.clone(),
            ))
        })
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
        .collect();
    let users = desired_users(bundle)
        .into_iter()
        .map(|user| {
            let present = readiness.map(|values| values.get(&user).copied().unwrap_or(false));
            UserReadinessReport {
                user,
                present,
                action: match present {
                    Some(true) => "none",
                    Some(false) => "create_or_reuse",
                    None => "unknown",
                }
                .to_string(),
            }
        })
        .collect();
    IdentityReport {
        admin: AdminIdentityReport {
            key: admin_key.to_string(),
            user: bundle
                .forge
                .admin_user
                .clone()
                .unwrap_or_else(|| "<none>".to_string()),
            password: if admin_user
                .and_then(|user| user.password.as_deref())
                .is_some_and(|password| !password.trim().is_empty())
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
            user: bundle
                .repositories
                .first()
                .map(|repository| repository.plan.automation_login.as_str())
                .filter(|login| !login.is_empty())
                .unwrap_or(BOT_USER)
                .to_string(),
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
