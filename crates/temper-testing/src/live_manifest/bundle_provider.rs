//! Model-provider selection for validation-grade live bundles.

use toml::Value as TomlValue;

pub(super) fn agent_provider_fixture(manifest: &TomlValue) -> Result<String, String> {
    let provider = manifest
        .get("topology")
        .and_then(TomlValue::as_table)
        .and_then(|topology| topology.get("agent_provider"))
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "topology.agent_provider must be a string".to_string())
        })
        .transpose()?
        .unwrap_or("deepseek");
    match provider {
        "deepseek" | "anthropic" => Ok(provider.to_string()),
        other => Err(format!(
            "topology.agent_provider `{other}` is unsupported; expected `deepseek` or `anthropic`"
        )),
    }
}
