//! Scoped diagnostics and run-local receipts; never graph lineage authority.
mod handoff;
#[cfg(test)]
mod integration_tests;
mod projection;
mod request;
mod result;
mod source_identity;
#[cfg(test)]
mod tests;

use super::{CodebaseMemoryTool, ToolFailureCategory};
use crate::mcp::McpError;
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tongs::model::{ContentBlock, TextContent};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

pub(crate) use handoff::GraphHandoffTool;
pub(super) use request::schema;

/// Shared only inside one prepared coding session, including its child wrappers.
pub(crate) struct CoverageService {
    tool: CodebaseMemoryTool,
    receipts: Mutex<VecDeque<Value>>,
    nonce: String,
}

impl CoverageService {
    pub(super) fn new(tool: CodebaseMemoryTool) -> Self {
        Self {
            tool,
            receipts: Mutex::new(VecDeque::new()),
            nonce: format!("{}:{:?}", std::process::id(), std::time::SystemTime::now()),
        }
    }

    async fn inspect(&self, mut input: Value) -> Result<Value, &'static str> {
        let started = Instant::now();
        let alias = input
            .get("project")
            .or_else(|| input.get("repo"))
            .and_then(Value::as_str);
        let project = match alias {
            Some(alias) => self
                .tool
                .scope
                .resolve_alias(alias)
                .map_err(|_| "unknown coverage project")?,
            None => self.tool.scope.primary(),
        };
        let expected = request::validate(&mut input, &project.root)?;
        let source_head = super::scope::current_git_head(&project.root);
        let source_identity = source_identity::fingerprint(&project.root, &input)?;
        let prepared = self
            .tool
            .scope
            .prepare_tool_input(
                "check_index_coverage",
                Some("project"),
                input,
                self.tool.call_timeout,
            )
            .map_err(|_| "coverage index unavailable")?;
        let actual = prepared["project"]
            .as_str()
            .ok_or("coverage project unavailable")?;
        self.status(actual, &project.root, started).await?;
        let provider = self.provider_coverage(&prepared).await?;
        let confirmation = self.provider_coverage(&prepared).await?;
        self.status(actual, &project.root, started).await?;
        let generation = provider["indexed_at"]
            .as_str()
            .ok_or("coverage generation unavailable")?
            .to_owned();
        let mut report = result::normalize(provider, &prepared, &generation, expected.as_deref())?;
        if provider_generation_changed(&report, &confirmation)
            || source_identity != source_identity::fingerprint(&project.root, &prepared)?
        {
            report["status"] = json!("stale");
            report["supports_scoped_claim"] = json!(false);
        }
        report["version"] = json!(1);
        report["project"] = json!(project.canonical_alias);
        report["actual_project"] = json!(actual);
        report["checkout_id"] = json!(format!(
            "{:x}",
            Sha256::digest(project.root.to_string_lossy().as_bytes())
        ));
        report["source_identity"] = json!(source_identity);
        report["source_head"] = json!(source_head);
        self.record(&mut report)?;
        Ok(report)
    }

    async fn provider_coverage(&self, input: &Value) -> Result<Value, &'static str> {
        let mut arguments = input.clone();
        let project = self
            .tool
            .scope
            .projects
            .iter()
            .find(|p| input["project"] == p.actual_project())
            .ok_or("coverage project unavailable")?;
        arguments["project"] = json!(project.canonical_alias);
        let output = self
            .tool
            .execute("coverage", arguments, None)
            .await
            .map_err(|_| "coverage unavailable")?;
        if output.is_error {
            return Err("coverage unavailable");
        }
        let text = output
            .content
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text(t) = b {
                    Some(t.text.as_str())
                } else {
                    None
                }
            })
            .collect::<String>();
        serde_json::from_str(&text).map_err(|_| "malformed coverage output")
    }

    fn record(&self, report: &mut Value) -> Result<(), &'static str> {
        let mut receipts = self.receipts.lock().unwrap_or_else(|p| p.into_inner());
        let offset = report["query_bounds"]["scope_offset"].as_u64().unwrap_or(0);
        if offset > 0 {
            let prior = receipts.iter().rev().find(|r| {
                r["actual_project"] == report["actual_project"]
                    && r["generation"] == report["generation"]
                    && r["checkout_id"] == report["checkout_id"]
                    && r["source_identity"] == report["source_identity"]
                    && r["query_bounds"]["scopes"] == report["query_bounds"]["scopes"]
                    && r["query_bounds"]["paths"] == report["query_bounds"]["paths"]
                    && r["query_bounds"]["scope_limit"] == report["query_bounds"]["scope_limit"]
                    && r["next_scope_offset"] == offset
                    && r["pagination_prefix_complete"] == true
            });
            if let Some(prior) = prior {
                let mut pages = prior["scope_pages"].as_array().cloned().unwrap_or_default();
                pages.push(report["provider"]["scopes"].clone());
                report["scope_pages"] = json!(pages);
                for status in ["flagged", "unavailable", "stale"] {
                    if prior["status"] == status
                        && severity(status)
                            > severity(report["status"].as_str().unwrap_or("unavailable"))
                    {
                        report["status"] = json!(status);
                    }
                }
            }
            report["pagination_prefix_complete"] = json!(prior.is_some());
            report["pagination_complete"] = json!(
                prior.is_some()
                    && report["provider"]["scopes"]
                        .as_array()
                        .is_some_and(|scopes| scopes.iter().all(|s| s["has_more"] == false
                            && s.get("nextCursor").is_none_or(Value::is_null)))
            );
        } else {
            report["pagination_prefix_complete"] = json!(true);
            report["scope_pages"] = json!([report["provider"]["scopes"]]);
        }
        report["next_scope_offset"] =
            json!(offset + report["query_bounds"]["scope_limit"].as_u64().unwrap_or(16));
        report["supports_scoped_claim"] = json!(
            report["status"] == "clean"
                && report["pagination_complete"] == true
                && report["query_bounds"]["scopes"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty())
        );
        if report.to_string().len() > 14 * 1024 {
            return Err("aggregate coverage exceeds bound; narrow scope or limit claim");
        }
        let id = format!(
            "coverage-{:x}",
            Sha256::digest(format!("{}:{}", self.nonce, report))
        );
        report["evidence_id"] = json!(id);
        if receipts.len() == 16 {
            receipts.pop_front();
        }
        receipts.push_back(report.clone());
        Ok(())
    }

    async fn status(
        &self,
        actual: &str,
        root: &std::path::Path,
        started: Instant,
    ) -> Result<(), &'static str> {
        let timeout = self
            .tool
            .call_timeout
            .checked_sub(started.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or("coverage timeout")?;
        if self.tool.health.open_cause().is_some() {
            return Err("coverage unavailable");
        }
        let _guard = temper_agent_io::timeout(timeout, self.tool.health.acquire_rpc())
            .await
            .map_err(|_| "coverage timeout")?;
        let output = self
            .tool
            .client
            .call_tool("index_status", json!({"project":actual}), timeout)
            .await
            .map_err(|error| {
                self.tool
                    .health
                    .record_failure(super::tool::classify_mcp_error(&error));
                if matches!(error, McpError::Timeout { .. }) {
                    "coverage timeout"
                } else {
                    "coverage unavailable"
                }
            })?;
        super::confirmation::confirm_index_status(&output, actual, actual, root)
            .map_err(|_| "coverage current-root mismatch or unavailable")?;
        Ok(())
    }

    async fn checked(&self, input: Value) -> Result<Value, &'static str> {
        temper_agent_io::timeout(self.tool.call_timeout, self.inspect(input))
            .await
            .map_err(|_| {
                self.tool
                    .health
                    .record_failure(ToolFailureCategory::Timeout);
                "coverage timeout"
            })?
    }
}

pub(super) struct CoverageTool(pub(super) Arc<CoverageService>);
#[async_trait]
impl Tool for CoverageTool {
    fn name(&self) -> &str {
        self.0.tool.name()
    }
    fn label(&self) -> &str {
        self.name()
    }
    fn description(&self) -> &str {
        self.0.tool.description()
    }
    fn parameters(&self) -> Value {
        self.0.tool.parameters()
    }
    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }
    async fn execute(
        &self,
        _id: &str,
        input: Value,
        _update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::error::Result<ToolOutput> {
        let report = match self.0.checked(input).await {
            Ok(report) => report,
            Err(reason) => {
                if reason == "coverage timeout" {
                    self.0
                        .tool
                        .health
                        .record_failure(ToolFailureCategory::Timeout);
                }
                json!({"version":1,"status":if reason.contains("unavailable") || reason.contains("timeout") {"unavailable"} else {"malformed"},"reason":reason,"supports_scoped_claim":false,"limitations":["Read affected source directly or limit the claim. Coverage supplies no source or mutation authority; do not retry an unavailable provider."]})
            }
        };
        let status = report["status"].as_str().unwrap_or("malformed");
        tracing::debug!(target:"temper::agent",event="codebase_memory.coverage", coverage.status=status, coverage.pagination_complete=report["pagination_complete"]==true, "agent: coverage diagnostic");
        Ok(ToolOutput {
            content: vec![ContentBlock::Text(TextContent {
                text: report.to_string(),
                text_signature: None,
            })],
            details: Some(json!({"coverage_status":status})),
            is_error: matches!(status, "malformed" | "unavailable"),
        })
    }
}

fn severity(status: &str) -> u8 {
    match status {
        "clean" => 0,
        "flagged" => 1,
        "unavailable" => 2,
        _ => 3,
    }
}
fn provider_generation_changed(report: &Value, confirmation: &Value) -> bool {
    let Some(generation) = report["generation"].as_str() else {
        return true;
    };
    let Ok(checked) = result::normalize(
        confirmation.clone(),
        &report["query_bounds"],
        generation,
        Some(generation),
    ) else {
        return true;
    };
    checked["status"] != report["status"]
        || checked["generation"] != report["generation"]
        || checked["provider"] != report["provider"]
}
