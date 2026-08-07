// SPDX-License-Identifier: MPL-2.0

//! Runner-enforced benchmark conditions. A condition changes only the
//! codebase-memory availability surface after the normal agent profile has
//! already resolved.

use std::path::Path;

use temper_protocol_agent::{
    AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
};

use super::BenchmarkRunError;
use crate::{BenchmarkConditionV1, ResolvedBenchmarkManifest};

pub(super) fn resolve_condition(
    manifest: &ResolvedBenchmarkManifest,
    requested: Option<BenchmarkConditionV1>,
) -> Result<Option<BenchmarkConditionV1>, BenchmarkRunError> {
    match (manifest.manifest().condition_profile.as_ref(), requested) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(BenchmarkRunError::Invalid(
            "`--condition` requires a benchmark manifest with `condition_profile`".to_string(),
        )),
        (Some(_), None) => Err(BenchmarkRunError::Invalid(
            "profiled benchmark requires `--condition <codebase-memory-enabled|codebase-memory-disabled|codebase-memory-unavailable>`"
                .to_string(),
        )),
        (Some(_), Some(condition)) => Ok(Some(condition)),
    }
}

pub(super) fn harness_tool_config(
    manifest: &ResolvedBenchmarkManifest,
    condition: Option<BenchmarkConditionV1>,
    provider_state_path: &Path,
) -> Result<Option<AgentToolConfig>, BenchmarkRunError> {
    let Some(condition) = condition else {
        return Ok(None);
    };
    if condition == BenchmarkConditionV1::CodebaseMemoryDisabled {
        return Ok(None);
    }
    let provider = manifest.condition_fixture_provider_path().ok_or_else(|| {
        BenchmarkRunError::Invalid(
            "codebase-memory condition profile did not resolve its fixture provider".to_string(),
        )
    })?;
    Ok(Some(AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Required,
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                provider.display().to_string(),
                provider_mode(condition).to_string(),
                "--state".to_string(),
                provider_state_path.display().to_string(),
            ],
            roles: vec![manifest.workspace_context().work_item.role.clone()],
            // The enabled harness provider starts cold, confirms its stable
            // upsert, then makes the same key available to warm graph calls.
            index: CodebaseMemoryIndex::Blocking,
            startup_timeout_secs: 5,
            index_timeout_secs: 5,
        }),
    }))
}

pub(super) fn live_tool_config(
    manifest: &ResolvedBenchmarkManifest,
    condition: Option<BenchmarkConditionV1>,
    mut configured: Option<AgentToolConfig>,
) -> Result<Option<AgentToolConfig>, BenchmarkRunError> {
    let Some(condition) = condition else {
        return Ok(configured);
    };
    let role = &manifest.workspace_context().work_item.role;
    let memory = configured
        .as_mut()
        .and_then(|tools| tools.codebase_memory.as_mut())
        .ok_or_else(|| {
            BenchmarkRunError::LiveConfiguration(
                "codebase-memory benchmark conditions require codebase-memory in the selected production agent profile"
                    .to_string(),
            )
        })?;
    if !memory.applies_to_role(role) {
        return Err(BenchmarkRunError::LiveConfiguration(format!(
            "selected codebase-memory profile does not apply to benchmark role `{role}`"
        )));
    }

    match condition {
        BenchmarkConditionV1::CodebaseMemoryEnabled => {}
        BenchmarkConditionV1::CodebaseMemoryDisabled => return Ok(None),
        BenchmarkConditionV1::CodebaseMemoryUnavailable => {
            let provider = manifest.condition_fixture_provider_path().ok_or_else(|| {
                BenchmarkRunError::Invalid(
                    "codebase-memory condition profile did not resolve its fixture provider"
                        .to_string(),
                )
            })?;
            memory.command = "python3".to_string();
            memory.args = vec![
                "-u".to_string(),
                provider.display().to_string(),
                provider_mode(condition).to_string(),
            ];
        }
    }
    Ok(configured)
}

fn provider_mode(condition: BenchmarkConditionV1) -> &'static str {
    match condition {
        BenchmarkConditionV1::CodebaseMemoryEnabled => "enabled",
        BenchmarkConditionV1::CodebaseMemoryUnavailable => "unavailable",
        BenchmarkConditionV1::CodebaseMemoryDisabled => "disabled",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::load_benchmark_manifest;

    fn manifest() -> ResolvedBenchmarkManifest {
        load_benchmark_manifest(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(
                "../../benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml",
            ),
        )
        .unwrap()
    }

    fn production_tools() -> AgentToolConfig {
        AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode: CodebaseMemoryMode::Required,
                command: "production-codebase-memory".to_string(),
                args: vec!["--production".to_string()],
                roles: vec!["engineer".to_string()],
                index: CodebaseMemoryIndex::Blocking,
                startup_timeout_secs: 7,
                index_timeout_secs: 23,
            }),
        }
    }

    #[test]
    fn harness_enabled_condition_uses_isolated_blocking_stable_state() {
        let configured = harness_tool_config(
            &manifest(),
            Some(BenchmarkConditionV1::CodebaseMemoryEnabled),
            Path::new("fixture-provider-state.json"),
        )
        .unwrap()
        .unwrap()
        .codebase_memory
        .unwrap();

        assert_eq!(configured.index, CodebaseMemoryIndex::Blocking);
        assert_eq!(configured.args[2], "enabled");
        assert_eq!(configured.args[3], "--state");
        assert_eq!(configured.args[4], "fixture-provider-state.json");
    }

    #[test]
    fn live_conditions_change_only_codebase_memory_availability() {
        let manifest = manifest();
        let production = production_tools();
        let enabled = live_tool_config(
            &manifest,
            Some(BenchmarkConditionV1::CodebaseMemoryEnabled),
            Some(production.clone()),
        )
        .unwrap();
        assert_eq!(enabled, Some(production.clone()));

        let disabled = live_tool_config(
            &manifest,
            Some(BenchmarkConditionV1::CodebaseMemoryDisabled),
            Some(production.clone()),
        )
        .unwrap();
        assert!(disabled.is_none());

        let unavailable = live_tool_config(
            &manifest,
            Some(BenchmarkConditionV1::CodebaseMemoryUnavailable),
            Some(production),
        )
        .unwrap()
        .unwrap()
        .codebase_memory
        .unwrap();
        assert_eq!(unavailable.mode, CodebaseMemoryMode::Required);
        assert_eq!(unavailable.index, CodebaseMemoryIndex::Blocking);
        assert_eq!(unavailable.roles, ["engineer"]);
        assert_eq!(unavailable.startup_timeout_secs, 7);
        assert_eq!(unavailable.index_timeout_secs, 23);
        assert_eq!(unavailable.command, "python3");
        assert_eq!(
            unavailable.args.last().map(String::as_str),
            Some("unavailable")
        );
    }

    #[test]
    fn profiled_manifest_requires_an_explicit_condition() {
        let error = resolve_condition(&manifest(), None).unwrap_err();
        assert!(error.to_string().contains("profiled benchmark requires"));
    }
}
