// SPDX-License-Identifier: MPL-2.0

use temper_cli_common::OutputFormat;

use super::model::{DeploymentPlanReport, RepositoryPlanReport};

pub(in crate::plan) fn print_report(
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
    println!("Repositories ({})", report.repositories.len());
    for repository in &report.repositories {
        print_repository(repository);
    }

    println!();
    println!("Workflow");
    println!("  name:      {}", report.workflow.name);
    println!("  labels:    {}", report.workflow.labels);
    println!("  roles:     {}", report.workflow.roles.join(", "));

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

fn print_repository(report: &RepositoryPlanReport) {
    println!();
    println!("  Repository {}", report.repository.path);
    println!("    action:    {}", report.repository.action);
    if let Some(exists) = report.repository.exists {
        println!("    exists:    {exists}");
    }
    if let Some(ci) = report.repository.ci_enabled {
        println!("    ci:        {}", if ci { "enabled" } else { "disabled" });
    }
    println!("    Labels");
    for label in &report.labels {
        println!("      {:<8} {}", label.action, label.name);
    }
    println!("    Webhook");
    println!("      url:     {}", report.webhook.url);
    println!("      secret:  {}", report.webhook.secret);
    println!("      action:  {}", report.webhook.action);
    println!("    Workflow metadata");
    println!(
        "      compatible: {} (checked {} artifact(s))",
        if report.metadata.compatible {
            "yes"
        } else {
            "no"
        },
        report.metadata.checked_artifacts
    );
    for invalid in &report.metadata.invalid {
        println!("      error:   {invalid}");
    }
    for finding in &report.findings {
        println!(
            "      {} [{}] {}",
            finding.severity, finding.category, finding.message
        );
    }
}
