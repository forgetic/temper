// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::path::Path;

use temper_protocol_activity::CaptureModeV1;

#[test]
fn absent_observability_uses_shared_metadata_defaults_and_durable_roots() {
    let env: BTreeMap<String, String> =
        BTreeMap::from([("XDG_STATE_HOME".to_string(), "/var/lib/user".to_string())]);
    let resolved = resolve(&Config::default(), &Credentials::default(), &env).expect("resolves");
    let traces = &resolved.observability.agent_traces;

    assert_eq!(traces.policy.capture, CaptureModeV1::Metadata);
    assert_eq!(traces.policy.retention_days, 14);
    assert_eq!(traces.policy.max_run_bytes, 50_000_000);
    assert!(!traces.policy.capture_thinking);
    assert_eq!(
        traces.engine_journal_root.as_deref(),
        Some(Path::new("/var/lib/user/temper/agent-traces/journal"))
    );
    assert_eq!(
        traces.worker_spool_root.as_deref(),
        Some(Path::new("/var/lib/user/temper/agent-traces/worker-spool"))
    );
    assert!(!traces.transcript_queries_enabled());
    assert!(
        !traces
            .engine_journal_root
            .as_ref()
            .expect("journal root")
            .starts_with(&resolved.worker.workspace_root)
    );
    assert!(
        !traces
            .worker_spool_root
            .as_ref()
            .expect("spool root")
            .starts_with(&resolved.worker.workspace_root)
    );
}

#[test]
fn every_capture_mode_resolves_and_diagnostic_may_capture_thinking() {
    for (name, expected) in [
        ("off", CaptureModeV1::Off),
        ("metadata", CaptureModeV1::Metadata),
        ("transcript", CaptureModeV1::Transcript),
        ("diagnostic", CaptureModeV1::Diagnostic),
    ] {
        let thinking = name == "diagnostic";
        let config = parse_config(&format!(
            "schema_version = 1\n[observability.agent_traces]\ncapture = \"{name}\"\ncapture_thinking = {thinking}\n"
        ));
        let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("mode resolves");
        assert_eq!(resolved.observability.agent_traces.policy.capture, expected);
        assert_eq!(
            resolved.observability.agent_traces.policy.capture_thinking,
            thinking
        );
    }
}

#[test]
fn invalid_capture_limits_and_combinations_are_rejected() {
    for (body, field) in [
        ("retention_days = 0", "retention_days"),
        ("max_run_bytes = 0", "max_run_bytes"),
        ("max_run_bytes = 1024", "max_run_bytes"),
        (
            "capture = \"transcript\"\ncapture_thinking = true",
            "capture_thinking",
        ),
    ] {
        let config = parse_config(&format!(
            "schema_version = 1\n[observability.agent_traces]\n{body}\n"
        ));
        let error = resolve(&config, &Credentials::default(), &NoEnv)
            .expect_err("invalid trace policy must fail");
        assert!(error.to_string().contains(field), "{field}: {error}");
    }

    let overflowing = Config::parse(
        "schema_version = 1\n[observability.agent_traces]\nmax_run_bytes = 9223372036854775808\n",
        Path::new("config.toml"),
        FileKind::Config,
    )
    .expect_err("overflowing TOML integer must fail parsing");
    assert!(
        overflowing.to_string().contains("number too large"),
        "{overflowing}"
    );

    let error = Config::parse(
        "schema_version = 1\n[observability.agent_traces]\ncapture = \"verbose\"\n",
        Path::new("config.toml"),
        FileKind::Config,
    )
    .expect_err("unknown capture mode must fail parsing");
    assert!(error.to_string().contains("verbose"), "{error}");
}

#[test]
fn relative_state_roots_resolve_from_config_and_do_not_follow_checkout_cleanup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let config = parse_config(
        "schema_version = 1\n[paths]\nstate_dir = \"state\"\nworkspace_dir = \"workspaces\"\n",
    );
    let resolved = resolve_with_options(
        &config,
        &Credentials::default(),
        &NoEnv,
        &ResolveOptions::from_config_base_dir(&bundle),
    )
    .expect("relative paths resolve");
    let traces = &resolved.observability.agent_traces;
    let journal = bundle.join("state/agent-traces/journal");
    let spool = bundle.join("state/agent-traces/worker-spool");
    assert_eq!(
        traces.engine_journal_root.as_deref(),
        Some(journal.as_path())
    );
    assert_eq!(traces.worker_spool_root.as_deref(), Some(spool.as_path()));

    let checkout = resolved.worker.workspace_root.join("engineer/workstream");
    std::fs::create_dir_all(&checkout).expect("checkout exists");
    std::fs::remove_dir_all(&checkout).expect("checkout removed");
    assert_eq!(
        traces.engine_journal_root.as_deref(),
        Some(journal.as_path())
    );
    assert_eq!(traces.worker_spool_root.as_deref(), Some(spool.as_path()));
}

#[test]
fn trace_storage_rejects_workspace_ancestors_and_disables_without_state() {
    let overlapping = parse_config(
        "schema_version = 1\n[paths]\nstate_dir = \"/srv/workspaces/state\"\nworkspace_dir = \"/srv/workspaces\"\n",
    );
    let error = resolve(&overlapping, &Credentials::default(), &NoEnv)
        .expect_err("trace roots below workspace must fail");
    assert!(
        error.to_string().contains("outside and separate"),
        "{error}"
    );

    let resolved = resolve(&Config::default(), &Credentials::default(), &NoEnv).expect("resolves");
    let traces = &resolved.observability.agent_traces;
    assert!(traces.capture_requested());
    assert!(traces.engine_journal_root.is_none());
    assert!(traces.worker_spool_root.is_none());
    assert_eq!(traces.policy_for_storage(None).capture, CaptureModeV1::Off);
}

#[test]
fn named_read_token_resolves_runtime_value_and_redacts_debug() {
    let config = parse_config(
        "schema_version = 1\n[observability.agent_traces]\nread_token = \"trace-reader\"\n",
    );
    let credentials = parse_credentials(
        "schema_version = 1\n[secrets]\ntrace-reader = \"trace-token-super-secret\"\n",
    );
    let resolved = resolve(&config, &credentials, &NoEnv).expect("token resolves");
    let traces = &resolved.observability.agent_traces;
    assert_eq!(
        traces
            .read_token
            .as_ref()
            .map(|reference| (reference.name.as_str(), reference.available)),
        Some(("trace-reader", true))
    );
    assert_eq!(
        traces
            .read_token_value
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("trace-token-super-secret")
    );
    assert!(traces.transcript_queries_enabled());
    let debug = format!("{resolved:?}");
    assert!(!debug.contains("trace-token-super-secret"), "{debug}");
    assert!(debug.contains("[REDACTED]"), "{debug}");
}

#[test]
fn missing_empty_and_unsafe_read_tokens_are_rejected() {
    let config = parse_config(
        "schema_version = 1\n[observability.agent_traces]\nread_token = \"trace-reader\"\n",
    );
    let missing = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("missing token must fail strict resolution");
    assert!(
        missing
            .to_string()
            .contains("observability.agent_traces.read_token"),
        "{missing}"
    );

    let empty_credentials =
        parse_credentials("schema_version = 1\n[secrets]\ntrace-reader = \"   \"\n");
    let empty = resolve(&config, &empty_credentials, &NoEnv)
        .expect_err("empty token must fail strict resolution");
    assert!(
        empty.to_string().contains("non-empty text value"),
        "{empty}"
    );

    let unsafe_config = parse_config(
        "schema_version = 1\n[observability.agent_traces]\nread_token = \"../trace-reader\"\n",
    );
    let unsafe_error = resolve(&unsafe_config, &Credentials::default(), &NoEnv)
        .expect_err("path-like token name must fail");
    assert!(
        unsafe_error.to_string().contains("secret name"),
        "{unsafe_error}"
    );
}
