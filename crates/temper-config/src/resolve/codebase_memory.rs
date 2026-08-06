// SPDX-License-Identifier: MPL-2.0

//! Codebase-memory tool and host-retention policy resolution.

use crate::error::ConfigError;
use crate::resolved::{
    AgentToolSettings, CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryRetentionSettings,
    CodebaseMemoryToolSettings,
};
use crate::schema::Config;

use super::{dedup_strings, trimmed};

const DEFAULT_CODEBASE_MEMORY_MODE: &str = "auto";
const DEFAULT_CODEBASE_MEMORY_COMMAND: &str = "codebase-memory-mcp";
const DEFAULT_CODEBASE_MEMORY_INDEX: &str = "background";
const DEFAULT_CODEBASE_MEMORY_STARTUP_TIMEOUT_SECS: u64 = 5;
const DEFAULT_CODEBASE_MEMORY_INDEX_TIMEOUT_SECS: u64 = 30;
const DEFAULT_CODEBASE_MEMORY_RETENTION_ENABLED: bool = true;
const DEFAULT_CODEBASE_MEMORY_MAX_OBSOLETE_PROJECTS: u32 = 64;
const DEFAULT_CODEBASE_MEMORY_MAX_AGE_DAYS: u32 = 30;
const DEFAULT_CODEBASE_MEMORY_MAINTENANCE_INTERVAL_SECS: u64 = 60 * 60;
const DEFAULT_CODEBASE_MEMORY_MAINTENANCE_TIMEOUT_SECS: u64 = 30;
const DEFAULT_CODEBASE_MEMORY_INVENTORY_PAGE_SIZE: u32 = 50;
const DEFAULT_CODEBASE_MEMORY_MAX_INVENTORY_PAGES: u32 = 20;
const DEFAULT_CODEBASE_MEMORY_MAX_DELETIONS_PER_RUN: u32 = 16;

pub(super) fn resolve_agent_tools(config: &Config) -> Result<AgentToolSettings, ConfigError> {
    let codebase_memory = config
        .agent
        .tools
        .codebase_memory
        .as_ref()
        .map(resolve_codebase_memory_tool)
        .transpose()?
        .flatten();
    Ok(AgentToolSettings { codebase_memory })
}

fn resolve_codebase_memory_tool(
    raw: &crate::schema::CodebaseMemoryToolConfig,
) -> Result<Option<CodebaseMemoryToolSettings>, ConfigError> {
    let mode = parse_codebase_memory_mode(
        &trimmed(raw.mode.as_deref()).unwrap_or_else(|| DEFAULT_CODEBASE_MEMORY_MODE.to_string()),
    )?;
    let Some(mode) = mode else {
        return Ok(None);
    };

    let command = match raw.command.as_deref() {
        Some(command) if command.trim().is_empty() => {
            return Err(ConfigError::invalid(
                "agent.tools.codebase_memory.command must not be empty when enabled",
            ));
        }
        Some(command) => command.trim().to_string(),
        None => DEFAULT_CODEBASE_MEMORY_COMMAND.to_string(),
    };

    let args = raw
        .args
        .as_deref()
        .map(|args| dedup_strings(args.iter().filter_map(|arg| trimmed(Some(arg.as_str())))))
        .unwrap_or_default();
    let roles = match raw.roles.as_deref() {
        Some(roles) => resolve_codebase_memory_roles(roles)?,
        None => vec!["*".to_string()],
    };
    let index = parse_codebase_memory_index(
        &trimmed(raw.index.as_deref()).unwrap_or_else(|| DEFAULT_CODEBASE_MEMORY_INDEX.to_string()),
    )?;
    let startup_timeout_secs = positive_secs_value(
        raw.startup_timeout_secs
            .unwrap_or(DEFAULT_CODEBASE_MEMORY_STARTUP_TIMEOUT_SECS),
        "agent.tools.codebase_memory.startup_timeout_secs",
    )?;
    let index_timeout_secs = positive_secs_value(
        raw.index_timeout_secs
            .unwrap_or(DEFAULT_CODEBASE_MEMORY_INDEX_TIMEOUT_SECS),
        "agent.tools.codebase_memory.index_timeout_secs",
    )?;

    let retention = &raw.retention;
    let retention = CodebaseMemoryRetentionSettings {
        enabled: retention
            .enabled
            .unwrap_or(DEFAULT_CODEBASE_MEMORY_RETENTION_ENABLED),
        max_obsolete_projects: bounded_u32_value(
            retention
                .max_obsolete_projects
                .unwrap_or(DEFAULT_CODEBASE_MEMORY_MAX_OBSOLETE_PROJECTS),
            10_000,
            "agent.tools.codebase_memory.retention.max_obsolete_projects",
        )?,
        max_age_days: bounded_positive_u32_value(
            retention
                .max_age_days
                .unwrap_or(DEFAULT_CODEBASE_MEMORY_MAX_AGE_DAYS),
            3_650,
            "agent.tools.codebase_memory.retention.max_age_days",
        )?,
        maintenance_interval_secs: positive_secs_value(
            retention
                .maintenance_interval_secs
                .unwrap_or(DEFAULT_CODEBASE_MEMORY_MAINTENANCE_INTERVAL_SECS),
            "agent.tools.codebase_memory.retention.maintenance_interval_secs",
        )?,
        maintenance_timeout_secs: bounded_positive_u64_value(
            retention
                .maintenance_timeout_secs
                .unwrap_or(DEFAULT_CODEBASE_MEMORY_MAINTENANCE_TIMEOUT_SECS),
            300,
            "agent.tools.codebase_memory.retention.maintenance_timeout_secs",
        )?,
        inventory_page_size: bounded_positive_u32_value(
            retention
                .inventory_page_size
                .unwrap_or(DEFAULT_CODEBASE_MEMORY_INVENTORY_PAGE_SIZE),
            200,
            "agent.tools.codebase_memory.retention.inventory_page_size",
        )?,
        max_inventory_pages: bounded_positive_u32_value(
            retention
                .max_inventory_pages
                .unwrap_or(DEFAULT_CODEBASE_MEMORY_MAX_INVENTORY_PAGES),
            100,
            "agent.tools.codebase_memory.retention.max_inventory_pages",
        )?,
        max_deletions_per_run: bounded_positive_u32_value(
            retention
                .max_deletions_per_run
                .unwrap_or(DEFAULT_CODEBASE_MEMORY_MAX_DELETIONS_PER_RUN),
            100,
            "agent.tools.codebase_memory.retention.max_deletions_per_run",
        )?,
    };

    Ok(Some(CodebaseMemoryToolSettings {
        mode,
        command,
        args,
        roles,
        index,
        startup_timeout_secs,
        index_timeout_secs,
        retention,
    }))
}

fn parse_codebase_memory_mode(raw: &str) -> Result<Option<CodebaseMemoryMode>, ConfigError> {
    match raw {
        "off" => Ok(None),
        "auto" => Ok(Some(CodebaseMemoryMode::Auto)),
        "required" => Ok(Some(CodebaseMemoryMode::Required)),
        other => Err(ConfigError::invalid(format!(
            "invalid agent.tools.codebase_memory.mode `{other}` (expected `off`, `auto`, or `required`)"
        ))),
    }
}

fn parse_codebase_memory_index(raw: &str) -> Result<CodebaseMemoryIndex, ConfigError> {
    match raw {
        "off" => Ok(CodebaseMemoryIndex::Off),
        "background" => Ok(CodebaseMemoryIndex::Background),
        "blocking" => Ok(CodebaseMemoryIndex::Blocking),
        other => Err(ConfigError::invalid(format!(
            "invalid agent.tools.codebase_memory.index `{other}` (expected `off`, `background`, or `blocking`)"
        ))),
    }
}

fn resolve_codebase_memory_roles(raw: &[String]) -> Result<Vec<String>, ConfigError> {
    let mut roles = Vec::with_capacity(raw.len());
    for role in raw {
        let role = role.trim();
        if role.is_empty() {
            return Err(ConfigError::invalid(
                "agent.tools.codebase_memory.roles entries must not be empty",
            ));
        }
        roles.push(role.to_string());
    }
    Ok(dedup_strings(roles))
}

fn positive_secs_value(value: u64, field: &str) -> Result<u64, ConfigError> {
    if value == 0 {
        return Err(ConfigError::invalid(format!(
            "{field} must be greater than zero"
        )));
    }
    Ok(value)
}

fn bounded_u32_value(value: u32, maximum: u32, field: &str) -> Result<u32, ConfigError> {
    if value > maximum {
        return Err(ConfigError::invalid(format!(
            "{field} must be at most {maximum}"
        )));
    }
    Ok(value)
}

fn bounded_positive_u32_value(value: u32, maximum: u32, field: &str) -> Result<u32, ConfigError> {
    if value == 0 || value > maximum {
        return Err(ConfigError::invalid(format!(
            "{field} must be between 1 and {maximum}"
        )));
    }
    Ok(value)
}

fn bounded_positive_u64_value(value: u64, maximum: u64, field: &str) -> Result<u64, ConfigError> {
    if value == 0 || value > maximum {
        return Err(ConfigError::invalid(format!(
            "{field} must be between 1 and {maximum}"
        )));
    }
    Ok(value)
}
