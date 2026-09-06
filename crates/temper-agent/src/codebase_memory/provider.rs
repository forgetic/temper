use serde_json::Value;

use crate::mcp::{McpError, McpToolDescriptor, StdioMcpClient};

pub(super) const SUPPORTED_PROVIDER_NAME: &str = "codebase-memory-mcp";
pub(super) const MINIMUM_PROVIDER_VERSION: &str = "0.10.8";

/// Enforces the supported release baseline and the provider schema Temper
/// relies on for path-independent project identity before indexing a checkout.
pub(super) fn validate_provider_contract(
    client: &StdioMcpClient,
    advertised: &[McpToolDescriptor],
) -> Result<(), McpError> {
    let metadata = client
        .server_metadata()
        .ok_or_else(|| incompatible("initialize did not return bounded provider metadata"))?;
    let name = metadata.name.as_deref().unwrap_or("<missing>");
    let version = metadata.version.as_deref().unwrap_or("<missing>");
    if name != SUPPORTED_PROVIDER_NAME {
        return Err(incompatible(&format!(
            "initialize identified `{name}` instead of `{SUPPORTED_PROVIDER_NAME}`"
        )));
    }
    if !version_at_least(version, (0, 10, 8)) {
        return Err(incompatible(&format!(
            "provider version `{version}` is older than {MINIMUM_PROVIDER_VERSION}"
        )));
    }
    if !metadata.advertises_capability("tools") {
        return Err(incompatible(
            "initialize did not advertise the MCP `tools` capability",
        ));
    }

    let status = descriptor(advertised, "index_status")
        .ok_or_else(|| incompatible("provider did not advertise `index_status`"))?;
    require_string_property(status, "project", true)?;

    let index = descriptor(advertised, "index_repository")
        .ok_or_else(|| incompatible("provider did not advertise `index_repository`"))?;
    require_string_property(index, "repo_path", true)?;
    require_string_property(index, "name", false)?;
    Ok(())
}

fn descriptor<'a>(
    advertised: &'a [McpToolDescriptor],
    name: &str,
) -> Option<&'a McpToolDescriptor> {
    advertised.iter().find(|descriptor| descriptor.name == name)
}

fn require_string_property(
    descriptor: &McpToolDescriptor,
    property: &str,
    required: bool,
) -> Result<(), McpError> {
    let property_schema = descriptor
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(property));
    if property_schema
        .and_then(|schema| schema.get("type"))
        .and_then(Value::as_str)
        != Some("string")
    {
        return Err(incompatible(&format!(
            "`{}` must advertise a string `{property}` input",
            descriptor.name
        )));
    }
    if required
        && !descriptor
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|properties| properties.iter().any(|value| value == property))
    {
        return Err(incompatible(&format!(
            "`{}` must require its `{property}` input",
            descriptor.name
        )));
    }
    Ok(())
}

fn incompatible(reason: &str) -> McpError {
    McpError::Protocol(format!(
        "incompatible codebase-memory provider: {reason}; upgrade `{SUPPORTED_PROVIDER_NAME}` to >= {MINIMUM_PROVIDER_VERSION} with targeted `index_status(project)` and stable `index_repository(repo_path, name)` upsert support"
    ))
}

fn version_at_least(raw: &str, minimum: (u64, u64, u64)) -> bool {
    let raw = raw.strip_prefix('v').unwrap_or(raw);
    let without_build = raw.split_once('+').map_or(raw, |(version, _)| version);
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, false), |(version, _)| (version, true));
    let mut pieces = core.split('.');
    let parsed = (
        pieces.next().and_then(|value| value.parse().ok()),
        pieces.next().and_then(|value| value.parse().ok()),
        pieces.next().and_then(|value| value.parse().ok()),
    );
    if pieces.next().is_some() {
        return false;
    }
    matches!(
        parsed,
        (Some(major), Some(minor), Some(patch))
            if (major, minor, patch) >= minimum
                && (!prerelease || (major, minor, patch) > minimum)
    )
}

#[cfg(test)]
mod tests {
    use super::version_at_least;

    #[test]
    fn provider_version_comparison_is_numeric_and_rejects_malformed_values() {
        assert!(version_at_least("0.10.8", (0, 10, 8)));
        assert!(version_at_least("v0.10.8+build", (0, 10, 8)));
        assert!(!version_at_least("0.10.8-alpha.1", (0, 10, 8)));
        assert!(!version_at_least("0.10.8.1", (0, 10, 8)));
        assert!(!version_at_least("0.9.0", (0, 10, 8)));
        assert!(!version_at_least("0.10.7", (0, 10, 8)));
        assert!(!version_at_least("unknown", (0, 10, 8)));
    }
}
