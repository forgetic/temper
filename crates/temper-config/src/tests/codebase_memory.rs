// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::{CodebaseMemoryIndex, CodebaseMemoryMode, config_template};

#[test]
fn codebase_memory_absent_and_off_resolve_disabled() {
    let absent = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["engineer"]
"#,
    );
    let resolved = resolve(&absent, &Credentials::default(), &NoEnv).expect("resolves");
    assert!(resolved.agent.tools.is_empty());
    assert!(resolved.agent.tools.codebase_memory.is_none());

    let off = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["engineer"]
[agent.tools.codebase_memory]
mode = "off"
"#,
    );
    let resolved = resolve(&off, &Credentials::default(), &NoEnv).expect("resolves");
    assert!(resolved.agent.tools.codebase_memory.is_none());
}

#[test]
fn codebase_memory_enabled_defaults_are_resolved() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["engineer"]
[agent.tools.codebase_memory]
mode = "auto"
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");
    let tool = resolved
        .agent
        .tools
        .codebase_memory
        .as_ref()
        .expect("codebase-memory enabled");
    assert_eq!(tool.mode, CodebaseMemoryMode::Auto);
    assert_eq!(tool.command, "codebase-memory-mcp");
    assert!(tool.args.is_empty());
    assert_eq!(tool.roles, vec!["*".to_string()]);
    assert_eq!(tool.index, CodebaseMemoryIndex::Background);
    assert_eq!(tool.startup_timeout_secs, 5);
    assert_eq!(tool.index_timeout_secs, 30);
    assert!(tool.retention.enabled);
    assert_eq!(tool.retention.max_obsolete_projects, 64);
    assert_eq!(tool.retention.max_age_days, 30);
    assert_eq!(tool.retention.maintenance_interval_secs, 3600);
    assert_eq!(tool.retention.maintenance_timeout_secs, 30);
    assert_eq!(tool.retention.inventory_page_size, 50);
    assert_eq!(tool.retention.max_inventory_pages, 20);
    assert_eq!(tool.retention.max_deletions_per_run, 16);
    assert!(tool.applies_to_role("engineer"));
    assert!(tool.applies_to_role("architect"));
}

#[test]
fn config_template_enables_codebase_memory_auto_defaults() {
    let template = config_template();
    assert!(
        template.contains("ci_poll_cadence_secs = 60"),
        "starter template should surface the dedicated CI cadence"
    );
    assert!(
        template.contains("ci_missing_grace_secs = 300"),
        "starter template should surface the missing-CI grace"
    );
    let config = parse_config(&template);
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("template resolves");
    assert_eq!(
        resolved.engine.ci_poll_cadence,
        Some(std::time::Duration::from_secs(60))
    );
    assert_eq!(
        resolved.engine.ci_missing_grace,
        std::time::Duration::from_secs(300)
    );
    let tool = resolved
        .agent
        .tools
        .codebase_memory
        .expect("template enables codebase-memory");
    assert_eq!(tool.mode, CodebaseMemoryMode::Auto);
    assert_eq!(tool.command, "codebase-memory-mcp");
    assert!(tool.args.is_empty());
    assert_eq!(tool.roles, vec!["*".to_string()]);
    assert_eq!(tool.index, CodebaseMemoryIndex::Background);
    assert!(tool.retention.enabled);
    assert_eq!(tool.retention.max_obsolete_projects, 64);
    assert_eq!(tool.retention.max_age_days, 30);
    assert_eq!(tool.retention.max_deletions_per_run, 16);
    assert_eq!(
        resolved.agent.operation_limits.tool_timeout,
        std::time::Duration::from_secs(600)
    );
    assert_eq!(
        resolved.worker.liveness_limits.max_no_progress,
        std::time::Duration::from_secs(900)
    );
    assert_eq!(
        resolved.observability.agent_traces.policy.capture,
        crate::CaptureModeV1::Metadata
    );
    assert_eq!(
        resolved.observability.agent_traces.policy.retention_days,
        14
    );
    assert_eq!(
        resolved.observability.agent_traces.policy.max_run_bytes,
        50_000_000
    );
}

#[test]
fn codebase_memory_valid_config_trims_deduplicates_and_filters_roles() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["engineer", "architect"]
[agent.tools.codebase_memory]
mode = " required "
command = " codebase-memory-mcp "
args = [" --cache ", "--cache", "", "  ", "local"]
roles = [" engineer ", "architect", "engineer"]
index = "blocking"
startup_timeout_secs = 7
index_timeout_secs = 90

[agent.tools.codebase_memory.retention]
enabled = false
max_obsolete_projects = 3
max_age_days = 7
maintenance_interval_secs = 600
maintenance_timeout_secs = 11
inventory_page_size = 25
max_inventory_pages = 4
max_deletions_per_run = 2
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");
    let tool = resolved.agent.tools.codebase_memory.expect("enabled");
    assert_eq!(tool.mode, CodebaseMemoryMode::Required);
    assert_eq!(tool.command, "codebase-memory-mcp");
    assert_eq!(tool.args, vec!["--cache".to_string(), "local".to_string()]);
    assert_eq!(
        tool.roles,
        vec!["engineer".to_string(), "architect".to_string()]
    );
    assert_eq!(tool.index, CodebaseMemoryIndex::Blocking);
    assert_eq!(tool.startup_timeout_secs, 7);
    assert_eq!(tool.index_timeout_secs, 90);
    assert!(!tool.retention.enabled);
    assert_eq!(tool.retention.max_obsolete_projects, 3);
    assert_eq!(tool.retention.max_age_days, 7);
    assert_eq!(tool.retention.maintenance_interval_secs, 600);
    assert_eq!(tool.retention.maintenance_timeout_secs, 11);
    assert_eq!(tool.retention.inventory_page_size, 25);
    assert_eq!(tool.retention.max_inventory_pages, 4);
    assert_eq!(tool.retention.max_deletions_per_run, 2);
    assert!(tool.applies_to_role("engineer"));
    assert!(tool.applies_to_role("architect"));
    assert!(!tool.applies_to_role("reviewer"));
}

#[test]
fn codebase_memory_invalid_values_are_rejected() {
    for (toml, expected) in [
        (
            r#"[agent.tools.codebase_memory]
mode = "sometimes"
"#,
            "agent.tools.codebase_memory.mode",
        ),
        (
            r#"[agent.tools.codebase_memory]
index = "eventually"
"#,
            "agent.tools.codebase_memory.index",
        ),
        (
            r#"[agent.tools.codebase_memory]
command = "   "
"#,
            "agent.tools.codebase_memory.command",
        ),
        (
            r#"[agent.tools.codebase_memory]
roles = ["engineer", "  "]
"#,
            "agent.tools.codebase_memory.roles",
        ),
        (
            r#"[agent.tools.codebase_memory]
startup_timeout_secs = 0
"#,
            "agent.tools.codebase_memory.startup_timeout_secs",
        ),
        (
            r#"[agent.tools.codebase_memory]
index_timeout_secs = 0
"#,
            "agent.tools.codebase_memory.index_timeout_secs",
        ),
        (
            r#"[agent.tools.codebase_memory.retention]
max_age_days = 0
"#,
            "agent.tools.codebase_memory.retention.max_age_days",
        ),
        (
            r#"[agent.tools.codebase_memory.retention]
max_deletions_per_run = 0
"#,
            "agent.tools.codebase_memory.retention.max_deletions_per_run",
        ),
        (
            r#"[agent.tools.codebase_memory.retention]
inventory_page_size = 201
"#,
            "agent.tools.codebase_memory.retention.inventory_page_size",
        ),
    ] {
        let config = parse_config(&format!(
            "schema_version = 1\n[engine]\nrepos = [\"a/b\"]\nroles = [\"engineer\"]\n{toml}"
        ));
        let error = resolve(&config, &Credentials::default(), &NoEnv)
            .expect_err("invalid codebase-memory config should fail");
        assert!(
            format!("{error}").contains(expected),
            "error `{error}` should contain `{expected}`"
        );
    }
}
