//! Standalone configuration wiring for the fake codebase-memory provider.

use std::fs;
use std::path::Path;

use toml::Value as TomlValue;

use super::FakeMcpServer;

pub(in crate::live_manifest) struct ToolConfiguration {
    pub(in crate::live_manifest) role: String,
    pub(in crate::live_manifest) tool: String,
    pub(in crate::live_manifest) mode: String,
    pub(in crate::live_manifest) index: String,
    pub(in crate::live_manifest) tool_timeout_secs: Option<u64>,
}

pub(in crate::live_manifest) fn tune_codebase_memory_config(
    config_path: &Path,
    fake_mcp: &FakeMcpServer,
    configuration: &ToolConfiguration,
) -> Result<(), String> {
    let text = fs::read_to_string(config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let mut doc: TomlValue = text
        .parse()
        .map_err(|error| format!("parse {} as TOML: {error}", config_path.display()))?;
    let root = doc
        .as_table_mut()
        .ok_or_else(|| "config.toml root must be a table".to_string())?;
    let agent = root
        .entry("agent".to_string())
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| "config.toml [agent] must be a table".to_string())?;
    if let Some(timeout) = configuration.tool_timeout_secs {
        let deadlines = agent
            .entry("deadlines".to_string())
            .or_insert_with(|| TomlValue::Table(Default::default()))
            .as_table_mut()
            .ok_or_else(|| "config.toml [agent.deadlines] must be a table".to_string())?;
        deadlines.insert(
            "tool_timeout_secs".to_string(),
            TomlValue::Integer(i64::try_from(timeout).expect("bounded timeout fits i64")),
        );
    }
    let tools = agent
        .entry("tools".to_string())
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| "config.toml [agent.tools] must be a table".to_string())?;
    let mut codebase = toml::map::Map::new();
    codebase.insert(
        "mode".to_string(),
        TomlValue::String(configuration.mode.clone()),
    );
    codebase.insert(
        "command".to_string(),
        TomlValue::String("python3".to_string()),
    );
    codebase.insert(
        "args".to_string(),
        TomlValue::Array(vec![
            TomlValue::String("-u".to_string()),
            TomlValue::String(fake_mcp.script_path.display().to_string()),
            TomlValue::String(fake_mcp.log_path.display().to_string()),
            TomlValue::String("demo".to_string()),
            TomlValue::String(fake_mcp.project.clone()),
            TomlValue::String(
                serde_json::to_string(&fake_mcp.safe_tools)
                    .map_err(|error| format!("serialize declared safe MCP tools: {error}"))?,
            ),
            TomlValue::String(
                serde_json::to_string(&fake_mcp.hidden_tools)
                    .map_err(|error| format!("serialize declared hidden MCP tools: {error}"))?,
            ),
            TomlValue::String(fake_mcp.readiness_delay_ms.to_string()),
            TomlValue::String(
                fake_mcp
                    .forced_systemic_failure
                    .as_ref()
                    .map(|failure| failure.tool.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            TomlValue::String(
                fake_mcp
                    .forced_systemic_failure
                    .as_ref()
                    .map(|failure| failure.after_calls)
                    .unwrap_or_default()
                    .to_string(),
            ),
            TomlValue::String(
                fake_mcp
                    .lifecycle_profile
                    .as_deref()
                    .unwrap_or("")
                    .to_string(),
            ),
        ]),
    );
    codebase.insert(
        "roles".to_string(),
        TomlValue::Array(vec![TomlValue::String(configuration.role.clone())]),
    );
    codebase.insert(
        "index".to_string(),
        TomlValue::String(configuration.index.clone()),
    );
    codebase.insert("startup_timeout_secs".to_string(), TomlValue::Integer(2));
    codebase.insert("index_timeout_secs".to_string(), TomlValue::Integer(3));
    tools.insert(configuration.tool.clone(), TomlValue::Table(codebase));
    fs::write(
        config_path,
        toml::to_string_pretty(&doc).map_err(|error| format!("serialize tuned config: {error}"))?,
    )
    .map_err(|error| format!("write tuned config {}: {error}", config_path.display()))
}
