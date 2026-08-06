// SPDX-License-Identifier: MPL-2.0

//! Operator-only maintenance commands for the unified CLI.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use temper_cli_common::{EnvMap, GlobalOptions, OutputFormat, PathResolver};
use temper_config::{EX_USAGE, LoadInputs};
use temper_worker_service::{
    CodebaseMemoryRecoveryMode, CodebaseMemoryRecoveryReport, codebase_memory_maintenance_config,
    codebase_memory_recovery_target, run_codebase_memory_recovery,
};

pub const USAGE: &str = "\
Host-controlled, dry-run-first maintenance.\n\
\n\
Usage: temper [GLOBAL OPTIONS] maintenance codebase-memory [OPTIONS]\n\
\n\
Options:\n\
      --apply                 Apply only the exact verified dry-run deletion class\n\
      --plan SHA256           Dry-run plan_id to bind --apply to reviewed evidence\n\
      --repository OWNER/NAME Verify the configured stable logical project\n\
      --rebuild-from PATH     Rebuild --repository from this explicit checkout (requires --apply)\n\
  -h, --help                  Print help\n\
\n\
Dry-run is the default. Global --config, --secrets, and --format human|json\n\
must precede `maintenance`. Provider deletion and rebuild remain host-only and\n\
are never exposed through coding-agent tools.";

const MAX_OUTPUT_RECORDS_PER_CLASS: usize = 100;
const MAX_HUMAN_FIELD_CHARS: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    apply: bool,
    plan_id: Option<String>,
    repository: Option<String>,
    rebuild_from: Option<PathBuf>,
}

enum ParseOutcome {
    Help,
    Run(Options),
}

pub fn main(
    args: Vec<String>,
    env: &EnvMap,
    paths: &PathResolver,
    globals: GlobalOptions,
) -> ExitCode {
    let options = match parse(args) {
        Ok(ParseOutcome::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ParseOutcome::Run(options)) => options,
        Err(error) => {
            eprintln!("temper maintenance: {error}\n\n{USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };

    let loaded = temper_config::load_explicit(&LoadInputs {
        explicit_config: globals.load.config,
        explicit_credentials: globals.load.credentials,
        env,
        paths,
    });
    let (resolved, _) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("temper maintenance: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(config) = codebase_memory_maintenance_config(&resolved) else {
        eprintln!(
            "temper maintenance: codebase-memory is not configured in [agent.tools.codebase_memory]"
        );
        return ExitCode::FAILURE;
    };
    let target = match options.repository.as_deref() {
        Some(repository) => {
            match codebase_memory_recovery_target(&resolved, repository, options.rebuild_from) {
                Ok(target) => Some(target),
                Err(error) => {
                    eprintln!("temper maintenance: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        None => None,
    };
    let mode = if options.apply {
        CodebaseMemoryRecoveryMode::Apply
    } else {
        CodebaseMemoryRecoveryMode::DryRun
    };
    let report =
        run_codebase_memory_recovery(&config, mode, options.plan_id.as_deref(), target, &|| false);
    match globals.format {
        OutputFormat::Human => println!("{}", render_human(&report)),
        OutputFormat::Json => match render_json(&report) {
            Ok(rendered) => println!("{rendered}"),
            Err(error) => {
                eprintln!("temper maintenance: encode JSON report failed: {error}");
                return ExitCode::FAILURE;
            }
        },
    }
    if report.succeeded() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse(args: Vec<String>) -> Result<ParseOutcome, String> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err("missing maintenance command (expected `codebase-memory`)".to_string());
    };
    if matches!(command.as_str(), "-h" | "--help" | "help") {
        return Ok(ParseOutcome::Help);
    }
    if command != "codebase-memory" {
        return Err(format!("unknown maintenance command `{command}`"));
    }

    let mut options = Options {
        apply: false,
        plan_id: None,
        repository: None,
        rebuild_from: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParseOutcome::Help),
            "--apply" if !options.apply => options.apply = true,
            "--apply" => return Err("--apply may be specified only once".to_string()),
            "--plan" if options.plan_id.is_none() => {
                options.plan_id = Some(next_value(&mut args, "--plan")?);
            }
            "--plan" => return Err("--plan may be specified only once".to_string()),
            "--repository" if options.repository.is_none() => {
                options.repository = Some(next_value(&mut args, "--repository")?);
            }
            "--repository" => {
                return Err("--repository may be specified only once".to_string());
            }
            "--rebuild-from" if options.rebuild_from.is_none() => {
                options.rebuild_from =
                    Some(PathBuf::from(next_value(&mut args, "--rebuild-from")?));
            }
            "--rebuild-from" => {
                return Err("--rebuild-from may be specified only once".to_string());
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    if options.apply && options.plan_id.is_none() {
        return Err("--apply requires --plan SHA256 from a reviewed dry-run".to_string());
    }
    if !options.apply && options.plan_id.is_some() {
        return Err("--plan is accepted only with --apply".to_string());
    }
    if options
        .plan_id
        .as_deref()
        .is_some_and(|plan| !valid_plan_id(plan))
    {
        return Err("--plan must be `sha256:` followed by 64 lowercase hex digits".to_string());
    }
    if options.rebuild_from.is_some() && options.repository.is_none() {
        return Err("--rebuild-from requires --repository OWNER/NAME".to_string());
    }
    if options.rebuild_from.is_some() && !options.apply {
        return Err("--rebuild-from requires the explicit --apply flag".to_string());
    }
    Ok(ParseOutcome::Run(options))
}

fn valid_plan_id(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .filter(|value| !value.starts_with('-') && !value.trim().is_empty())
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn bounded_report(report: &CodebaseMemoryRecoveryReport) -> (CodebaseMemoryRecoveryReport, usize) {
    let mut bounded = report.clone();
    let mut omitted = 0;
    macro_rules! truncate {
        ($records:expr) => {
            if $records.len() > MAX_OUTPUT_RECORDS_PER_CLASS {
                omitted += $records.len() - MAX_OUTPUT_RECORDS_PER_CLASS;
                $records.truncate(MAX_OUTPUT_RECORDS_PER_CLASS);
            }
        };
    }
    truncate!(bounded.retention.candidates);
    truncate!(bounded.retention.proposed);
    truncate!(bounded.retention.deleted);
    truncate!(bounded.retention.failed);
    truncate!(bounded.retention.preserved);
    (bounded, omitted)
}

fn render_json(report: &CodebaseMemoryRecoveryReport) -> Result<String, serde_json::Error> {
    let (bounded, omitted) = bounded_report(report);
    let mut value = serde_json::to_value(bounded)?;
    value
        .as_object_mut()
        .expect("recovery report serializes as an object")
        .insert(
            "output_bound".to_string(),
            json!({
                "max_records_per_class": MAX_OUTPUT_RECORDS_PER_CLASS,
                "records_omitted": omitted,
            }),
        );
    serde_json::to_string_pretty(&value)
}

fn render_human(report: &CodebaseMemoryRecoveryReport) -> String {
    let (bounded, omitted) = bounded_report(report);
    let retention = &bounded.retention;
    let mode = match bounded.mode {
        CodebaseMemoryRecoveryMode::DryRun => "dry-run",
        CodebaseMemoryRecoveryMode::Apply => "apply",
    };
    let mut lines = vec![
        format!("mode: {mode}"),
        format!(
            "plan id: {}",
            bounded.plan_id.as_deref().unwrap_or("unavailable")
        ),
        format!(
            "provider: {}",
            bounded.provider.as_ref().map_or_else(
                || "unverified".to_string(),
                |provider| format!(
                    "{} {} (cache instance {})",
                    human_field(&provider.name),
                    human_field(&provider.version),
                    human_field(
                        provider
                            .cache_instance_id
                            .as_deref()
                            .unwrap_or("unverified")
                    )
                )
            )
        ),
        format!(
            "inventory: complete={} records={}",
            retention.inventory_complete, retention.inventory_record_count
        ),
        format!(
            "cache bytes: {}",
            retention
                .cache_bytes
                .map(|bytes| bytes.to_string())
                .unwrap_or_else(|| "unavailable (provider did not report bytes)".to_string())
        ),
        format!(
            "configured bounds: max_obsolete_projects={} max_age_days={} timeout_secs={} page_size={} max_pages={} max_deletions={}",
            bounded.configured_bounds.max_obsolete_projects,
            bounded.configured_bounds.max_age_days,
            bounded.configured_bounds.maintenance_timeout_secs,
            bounded.configured_bounds.inventory_page_size,
            bounded.configured_bounds.max_inventory_pages,
            bounded.configured_bounds.max_deletions_per_run,
        ),
        format!(
            "actions: preserved={} candidates={} proposed={} deleted={} failed={} preflight_verified={}",
            report.retention.preserved.len(),
            report.retention.candidates.len(),
            report.retention.proposed.len(),
            report.retention.deleted.len(),
            report.retention.failed.len(),
            report.preflight_verified,
        ),
    ];
    for candidate in &retention.candidates {
        lines.push(format!(
            "candidate: {} path={} bytes={} reason={}",
            human_field(&candidate.project),
            candidate.repo_path.as_deref().map_or_else(
                || "unavailable".to_string(),
                |path| human_field(&path.display().to_string())
            ),
            candidate
                .estimated_bytes
                .map(|bytes| bytes.to_string())
                .unwrap_or_else(|| "unavailable".to_string()),
            human_field(&candidate.reason),
        ));
    }
    for proposed in &retention.proposed {
        lines.push(format!(
            "proposed action: delete {} path={} bytes={}",
            human_field(&proposed.project),
            proposed.repo_path.as_deref().map_or_else(
                || "unavailable".to_string(),
                |path| human_field(&path.display().to_string())
            ),
            proposed
                .estimated_bytes
                .map(|bytes| bytes.to_string())
                .unwrap_or_else(|| "unavailable".to_string()),
        ));
    }
    for preserved in &retention.preserved {
        lines.push(format!(
            "preserve: {} path={} reason={}",
            human_field(&preserved.project),
            preserved.repo_path.as_deref().map_or_else(
                || "unavailable".to_string(),
                |path| human_field(&path.display().to_string())
            ),
            human_field(&preserved.reason),
        ));
    }
    for deleted in &retention.deleted {
        lines.push(format!(
            "deleted action: {} path={} bytes={}",
            human_field(&deleted.project),
            deleted.repo_path.as_deref().map_or_else(
                || "unavailable".to_string(),
                |path| human_field(&path.display().to_string())
            ),
            deleted
                .estimated_bytes
                .map(|bytes| bytes.to_string())
                .unwrap_or_else(|| "unavailable".to_string()),
        ));
    }
    for failed in &retention.failed {
        lines.push(format!(
            "failed action: {} path={} error={}",
            human_field(&failed.record.project),
            failed.record.repo_path.as_deref().map_or_else(
                || "unavailable".to_string(),
                |path| human_field(&path.display().to_string())
            ),
            human_field(&failed.error),
        ));
    }
    if omitted > 0 {
        lines.push(format!(
            "output bounded: {omitted} records omitted (maximum {MAX_OUTPUT_RECORDS_PER_CLASS} per class)"
        ));
    }
    if let Some(project) = &bounded.stable_project {
        lines.push(format!(
            "stable project: {} key={} status={} ready={} rebuild_completed={} lookup_ms={} safe_probe={}",
            human_field(&project.logical_repository),
            human_field(&project.provider_key),
            human_field(project.status.as_deref().unwrap_or("unavailable")),
            project.ready,
            project.rebuild_completed,
            project
                .lookup_latency_ms
                .map(|latency| latency.to_string())
                .unwrap_or_else(|| "unavailable".to_string()),
            project.safe_probe_succeeded,
        ));
    }
    if let Some(reason) = retention.no_op_reason.as_deref() {
        lines.push(format!("refused: {}", human_field(reason)));
    }
    if let Some(failure) = bounded.failure.as_deref() {
        lines.push(format!("failure: {}", human_field(failure)));
    }
    lines.join("\n")
}

/// Makes provider/config-derived report fields safe for one-line terminal
/// evidence while retaining a deterministic, explicit representation of
/// control characters.
fn human_field(value: &str) -> String {
    let mut rendered = String::new();
    let mut chars = value.chars();
    for character in chars.by_ref().take(MAX_HUMAN_FIELD_CHARS) {
        rendered.extend(character.escape_default());
    }
    if chars.next().is_some() {
        rendered.push_str("...[field truncated]");
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use temper_worker_service::{
        CodebaseMemoryRetentionFailure, CodebaseMemoryRetentionPolicy,
        CodebaseMemoryRetentionRecordResult, CodebaseMemoryRetentionReport,
    };

    #[test]
    fn dry_run_is_default_and_rebuild_requires_apply_and_logical_repo() {
        let ParseOutcome::Run(options) = parse(vec!["codebase-memory".to_string()]).unwrap() else {
            panic!("expected run");
        };
        assert!(!options.apply);
        assert!(options.plan_id.is_none());
        assert!(options.repository.is_none());

        let error = parse(vec!["codebase-memory".to_string(), "--apply".to_string()])
            .err()
            .expect("unbound apply rejected");
        assert!(error.contains("--plan"));

        let ParseOutcome::Run(apply) = parse(vec![
            "codebase-memory".to_string(),
            "--apply".to_string(),
            "--plan".to_string(),
            format!("sha256:{}", "a".repeat(64)),
        ])
        .expect("bound apply parses") else {
            panic!("expected apply run");
        };
        assert!(apply.apply);
        assert!(apply.plan_id.is_some());

        let error = parse(vec![
            "codebase-memory".to_string(),
            "--rebuild-from".to_string(),
            "/tmp/repo".to_string(),
        ])
        .err()
        .expect("unsafe rebuild rejected");
        assert!(error.contains("--repository") || error.contains("--apply"));
    }

    #[test]
    fn json_output_marks_unavailable_bytes_and_enforces_record_bound() {
        let mut report = CodebaseMemoryRecoveryReport {
            mode: CodebaseMemoryRecoveryMode::DryRun,
            provider: None,
            configured_bounds: CodebaseMemoryRetentionPolicy::default(),
            plan_id: Some(format!("sha256:{}", "a".repeat(64))),
            preflight_verified: false,
            retention: Default::default(),
            stable_project: None,
            failure: None,
        };
        report.retention.preserved = (0..101)
            .map(|index| CodebaseMemoryRetentionRecordResult {
                project: format!("project-{index}"),
                repo_path: None,
                reason: "preserved".to_string(),
                estimated_bytes: None,
            })
            .collect();
        let rendered = render_json(&report).expect("JSON renders");
        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["output_bound"]["records_omitted"], 1);
        assert_eq!(
            value["retention"]["preserved"].as_array().unwrap().len(),
            100
        );
        assert!(value["retention"]["cache_bytes"].is_null());
    }

    #[test]
    fn human_output_escapes_controls_and_lists_apply_results() {
        let record = CodebaseMemoryRetentionRecordResult {
            project: "unsafe\nproject".to_string(),
            repo_path: Some(PathBuf::from("/workspace/unsafe\npath")),
            reason: "test".to_string(),
            estimated_bytes: None,
        };
        let report = CodebaseMemoryRecoveryReport {
            mode: CodebaseMemoryRecoveryMode::Apply,
            provider: None,
            configured_bounds: CodebaseMemoryRetentionPolicy::default(),
            plan_id: None,
            preflight_verified: true,
            retention: CodebaseMemoryRetentionReport {
                deleted: vec![record.clone()],
                failed: vec![CodebaseMemoryRetentionFailure {
                    record,
                    error: "provider\nfailed".to_string(),
                }],
                ..Default::default()
            },
            stable_project: None,
            failure: None,
        };

        let rendered = render_human(&report);
        assert!(rendered.contains("deleted action: unsafe\\nproject"));
        assert!(rendered.contains("failed action: unsafe\\nproject"));
        assert!(rendered.contains("provider\\nfailed"));
        assert!(!rendered.contains("unsafe\nproject"));
    }
}
