//! Model-visible, host-serviced read-only Forge context tools.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use temper_protocol_agent::{
    ArtifactType, ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult,
    ForgeGetItemOperation, ForgeListRelatedOperation, ForgeRelationType,
};
use tongs::error::{Error, Result};
use tongs::model::{ContentBlock, TextContent};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

const MAX_REPO_BYTES: usize = 512;
const MAX_RELATED_DEPTH: usize = 2;
const MAX_RELATED_RESULTS: usize = 50;
const MAX_RENDER_BYTES: usize = 512 * 1024;

pub type ForgeContextFuture = Pin<
    Box<dyn Future<Output = std::result::Result<ForgeContextResult, ForgeContextErrorCode>> + Send>,
>;

/// Per-run callback whose host has already bound worker/job identity and auth.
pub type ForgeContextHost = Arc<dyn Fn(ForgeContextOperation) -> ForgeContextFuture + Send + Sync>;

pub struct ForgeGetItemTool {
    host: ForgeContextHost,
}

impl ForgeGetItemTool {
    pub fn new(host: ForgeContextHost) -> Self {
        Self { host }
    }
}

pub struct ForgeListRelatedTool {
    host: ForgeContextHost,
}

impl ForgeListRelatedTool {
    pub fn new(host: ForgeContextHost) -> Self {
        Self { host }
    }
}

#[async_trait]
impl Tool for ForgeGetItemTool {
    fn name(&self) -> &str {
        "forge_get_item"
    }
    fn label(&self) -> &str {
        "forge_get_item"
    }
    fn description(&self) -> &str {
        "Fetch one issue or pull request from a configured repository through the host's bounded read-only Forge channel. Input: { repo, number, type?, include_comments? }."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["repo", "number"],
            "additionalProperties": false,
            "properties": {
                "repo": {"type":"string", "description":"Configured owner/name repository path."},
                "number": {"type":"integer", "minimum":1},
                "type": {"type":"string", "enum":["issue","pull_request"]},
                "include_comments": {"type":"boolean", "default":false}
            }
        })
    }
    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let operation = ForgeContextOperation::ForgeGetItem(parse_get_item(input)?);
        output("forge_get_item", (self.host)(operation).await)
    }
}

#[async_trait]
impl Tool for ForgeListRelatedTool {
    fn name(&self) -> &str {
        "forge_list_related"
    }
    fn label(&self) -> &str {
        "forge_list_related"
    }
    fn description(&self) -> &str {
        "Follow typed relations from an issue or pull request through the host's bounded read-only Forge channel. Input: { repo, number, type?, relations, depth?, limit? }. Call repeatedly to follow indirect relations."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["repo", "number", "relations"],
            "additionalProperties": false,
            "properties": {
                "repo": {"type":"string"},
                "number": {"type":"integer", "minimum":1},
                "type": {"type":"string", "enum":["issue","pull_request"]},
                "relations": {"type":"array", "minItems":1, "maxItems":7, "uniqueItems":true,
                    "items":{"type":"string", "enum":["parent","child","dependency","dependent","produced_pr","body_reference","referenced_by"]}},
                "depth": {"type":"integer", "minimum":1, "maximum":2},
                "limit": {"type":"integer", "minimum":1, "maximum":50}
            }
        })
    }
    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let operation = ForgeContextOperation::ForgeListRelated(parse_list_related(input)?);
        output("forge_list_related", (self.host)(operation).await)
    }
}

fn parse_get_item(input: serde_json::Value) -> Result<ForgeGetItemOperation> {
    let object = object("forge_get_item", &input)?;
    reject_unknown(
        "forge_get_item",
        object,
        &["repo", "number", "type", "include_comments"],
    )?;
    Ok(ForgeGetItemOperation {
        repo: repo("forge_get_item", object)?,
        number: number("forge_get_item", object)?,
        artifact_type: artifact_type("forge_get_item", object)?,
        include_comments: match object.get("include_comments") {
            Some(value) => value.as_bool().ok_or_else(|| {
                Error::tool("forge_get_item", "include_comments must be a boolean")
            })?,
            None => false,
        },
    })
}

fn parse_list_related(input: serde_json::Value) -> Result<ForgeListRelatedOperation> {
    let object = object("forge_list_related", &input)?;
    reject_unknown(
        "forge_list_related",
        object,
        &["repo", "number", "type", "relations", "depth", "limit"],
    )?;
    let values = object
        .get("relations")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::tool("forge_list_related", "relations must be a non-empty array"))?;
    if values.is_empty() || values.len() > 7 {
        return Err(Error::tool(
            "forge_list_related",
            "relations must contain 1..=7 entries",
        ));
    }
    let mut relations = Vec::with_capacity(values.len());
    for value in values {
        let relation: ForgeRelationType = serde_json::from_value(value.clone()).map_err(|_| {
            Error::tool(
                "forge_list_related",
                "relations contains an unsupported relation",
            )
        })?;
        if relations.contains(&relation) {
            return Err(Error::tool(
                "forge_list_related",
                "relations entries must be unique",
            ));
        }
        relations.push(relation);
    }
    let depth = optional_usize("forge_list_related", object, "depth", MAX_RELATED_DEPTH)?;
    let limit = optional_usize("forge_list_related", object, "limit", MAX_RELATED_RESULTS)?;
    Ok(ForgeListRelatedOperation {
        repo: repo("forge_list_related", object)?,
        number: number("forge_list_related", object)?,
        artifact_type: artifact_type("forge_list_related", object)?,
        relations,
        depth,
        limit,
    })
}

fn object<'a>(
    tool: &str,
    input: &'a serde_json::Value,
) -> Result<&'a serde_json::Map<String, serde_json::Value>> {
    input
        .as_object()
        .ok_or_else(|| Error::tool(tool, format!("{tool} input must be an object")))
}

fn reject_unknown(
    tool: &str,
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
) -> Result<()> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(Error::tool(tool, format!("unsupported argument `{key}`")));
    }
    Ok(())
}

fn repo(tool: &str, object: &serde_json::Map<String, serde_json::Value>) -> Result<String> {
    let value = object
        .get("repo")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::tool(tool, "repo must be an owner/name string"))?;
    if value.is_empty()
        || value.len() > MAX_REPO_BYTES
        || value.split('/').count() != 2
        || value.split('/').any(str::is_empty)
    {
        return Err(Error::tool(
            tool,
            "repo must be a non-empty owner/name path of at most 512 bytes",
        ));
    }
    Ok(value.to_string())
}

fn number(tool: &str, object: &serde_json::Map<String, serde_json::Value>) -> Result<u64> {
    object
        .get("number")
        .and_then(serde_json::Value::as_u64)
        .filter(|number| *number > 0)
        .ok_or_else(|| Error::tool(tool, "number must be a positive integer"))
}

fn artifact_type(
    tool: &str,
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<ArtifactType>> {
    object
        .get("type")
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|_| Error::tool(tool, "type must be `issue` or `pull_request`"))
        })
        .transpose()
}

fn optional_usize(
    tool: &str,
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    max: usize,
) -> Result<Option<usize>> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0 && *value <= max)
        .ok_or_else(|| Error::tool(tool, format!("{key} must be in 1..={max}")))?;
    Ok(Some(value))
}

fn output(
    tool: &str,
    result: std::result::Result<ForgeContextResult, ForgeContextErrorCode>,
) -> Result<ToolOutput> {
    match result {
        Ok(result) => {
            let details = serde_json::to_value(&result).ok();
            let mut text = serde_json::to_string_pretty(&result)
                .unwrap_or_else(|_| "{\"error\":\"result_serialization_failed\"}".to_string());
            truncate_utf8(&mut text, MAX_RENDER_BYTES);
            Ok(ToolOutput {
                content: vec![ContentBlock::Text(TextContent {
                    text,
                    text_signature: None,
                })],
                details,
                is_error: false,
            })
        }
        Err(code) => {
            let stable = serde_json::to_value(code)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "forge_unavailable".to_string());
            Ok(ToolOutput {
                content: vec![ContentBlock::Text(TextContent {
                    text: format!("{tool} failed: {stable}"),
                    text_signature: None,
                })],
                details: Some(serde_json::json!({"code": stable})),
                is_error: true,
            })
        }
    }
}

fn truncate_utf8(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> ForgeContextHost {
        Arc::new(|_| Box::pin(async { Err(ForgeContextErrorCode::NotFound) }))
    }

    #[test]
    fn in_process_async_host_fetches_item_and_follows_indirect_relation() {
        use std::sync::Mutex;
        use tongs::tools::Tool;

        temper_agent_io::block_on(async {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let seen_for_host = Arc::clone(&seen);
            let host: ForgeContextHost = Arc::new(move |operation| {
                seen_for_host.lock().expect("seen").push(operation.clone());
                Box::pin(async move {
                    let value = match operation {
                        ForgeContextOperation::ForgeGetItem(operation) => serde_json::json!({
                            "result":"item",
                            "item":{"artifact":{"repository":{"id":"r","path":operation.repo},"artifact_type":"issue","number":operation.number},"title":"Root","body":"body","state":"open"},
                            "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":false}
                        }),
                        ForgeContextOperation::ForgeListRelated(operation) => {
                            let next = operation.number - 1;
                            serde_json::json!({
                                "result":"related",
                                "root":{"repository":{"id":"r","path":operation.repo},"artifact_type":"issue","number":operation.number},
                                "items":[{"artifact":{"repository":{"id":"r","path":"ai/temper"},"artifact_type":"issue","number":next},"title":"Next","state":"open"}],
                                "edges":[],
                                "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":false}
                            })
                        }
                    };
                    Ok(serde_json::from_value(value).expect("result parses"))
                })
            });
            let get = ForgeGetItemTool::new(host.clone());
            let list = ForgeListRelatedTool::new(host);
            let item = get
                .execute(
                    "get",
                    serde_json::json!({"repo":"ai/temper","number":3,"type":"issue"}),
                    None,
                )
                .await
                .expect("get item");
            assert!(!item.is_error);
            let first = list
                .execute(
                    "list-1",
                    serde_json::json!({"repo":"ai/temper","number":3,"relations":["parent"]}),
                    None,
                )
                .await
                .expect("first relation");
            assert_eq!(
                first.details.as_ref().unwrap()["items"][0]["artifact"]["number"],
                2
            );
            let second = list
                .execute(
                    "list-2",
                    serde_json::json!({"repo":"ai/temper","number":2,"relations":["parent"]}),
                    None,
                )
                .await
                .expect("indirect relation");
            assert_eq!(
                second.details.as_ref().unwrap()["items"][0]["artifact"]["number"],
                1
            );
            assert_eq!(seen.lock().expect("seen").len(), 3);
        });
    }

    #[test]
    fn tools_are_read_only_and_validate_limits_locally() {
        let get = ForgeGetItemTool::new(host());
        let list = ForgeListRelatedTool::new(host());
        assert_eq!(get.effects(), ToolEffects::read());
        assert_eq!(list.effects(), ToolEffects::read());
        assert!(parse_get_item(serde_json::json!({"repo":"bad", "number":1})).is_err());
        assert!(parse_list_related(serde_json::json!({"repo":"ai/temper", "number":1, "relations":["parent"], "depth":3})).is_err());
        assert!(parse_list_related(serde_json::json!({"repo":"ai/temper", "number":1, "relations":[], "mutation":"close"})).is_err());
    }
}
