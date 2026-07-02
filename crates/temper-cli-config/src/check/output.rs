// SPDX-License-Identifier: MPL-2.0

use temper_config::{Finding, LoadedPaths};

use super::options::CheckOptions;

pub(crate) fn print_validation_human(loaded: &LoadedPaths, findings: &[Finding]) {
    if let Some(path) = &loaded.config {
        println!("config:      {}", path.display());
    } else {
        println!("config:      (none — defaults + environment)");
    }
    if let Some(path) = &loaded.credentials {
        println!("credentials: {}", path.display());
    } else {
        println!("credentials: (none — environment)");
    }
    println!();

    if findings.is_empty() {
        println!("OK — no problems found.");
        return;
    }
    for Finding { error, message } in findings {
        if *error {
            println!("error: {message}");
        } else {
            println!("note:  {message}");
        }
    }
}

pub(super) fn print_validation_json(
    loaded: &LoadedPaths,
    findings: &[Finding],
    options: &CheckOptions,
) -> Result<(), String> {
    let status = if has_blocking_findings(findings, options.strict) {
        "error"
    } else {
        "ok"
    };
    let config_path = loaded
        .config
        .as_ref()
        .map(|path| path.display().to_string());
    let credentials_path = loaded
        .credentials
        .as_ref()
        .map(|path| path.display().to_string());
    let findings = findings
        .iter()
        .map(|finding| {
            serde_json::json!({
                "severity": if finding.error { "error" } else { "note" },
                "message": &finding.message,
            })
        })
        .collect::<Vec<_>>();
    let report = serde_json::json!({
        "status": status,
        "result": status,
        "component": options.component.as_str(),
        "pool": options.pool.as_deref(),
        "strict": options.strict,
        "online": options.online,
        "config_path": config_path.clone(),
        "credentials_path": credentials_path.clone(),
        "paths": {
            "config": config_path,
            "credentials": credentials_path,
        },
        "findings": findings,
    });
    let text = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("serialize validation report: {error}"))?;
    println!("{text}");
    Ok(())
}

pub(crate) fn has_error_findings(findings: &[Finding]) -> bool {
    findings.iter().any(|finding| finding.error)
}

pub(super) fn has_blocking_findings(findings: &[Finding], strict: bool) -> bool {
    has_error_findings(findings) || (strict && !findings.is_empty())
}
