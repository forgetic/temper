//! Attributed parent-to-child evidence, with no inherited graph permissions.
use super::*;

pub(crate) struct GraphHandoffTool {
    inner: Box<dyn Tool>,
    coverage: Arc<CoverageService>,
}
impl GraphHandoffTool {
    pub(crate) fn new(inner: Box<dyn Tool>, coverage: Arc<CoverageService>) -> Self {
        Self { inner, coverage }
    }

    fn context(&self, input: &Value) -> Result<(Value, Value), &'static str> {
        let context = input.get("graph_context").ok_or("missing graph context")?;
        if context.to_string().len() > 6 * 1024 {
            return Err("graph context exceeds 6144 bytes");
        }
        let id = context["evidence_id"]
            .as_str()
            .ok_or("graph context requires evidence_id")?;
        let receipt = self.coverage.receipts.lock().unwrap_or_else(|p|p.into_inner()).iter().find(|r|r["evidence_id"] == id).cloned().ok_or("unknown or expired session evidence; recheck coverage after compaction or a fresh session")?;
        let mut context = validate_context(context.clone(), &receipt)?;
        context["coverage"] = json!({
            "evidence_id":receipt["evidence_id"],"project":receipt["project"],"actual_project":receipt["actual_project"],
            "checkout_id":receipt["checkout_id"],"source_head":receipt["source_head"],"source_identity":receipt["source_identity"],
            "generation":receipt["generation"],"status":receipt["status"],"query_bounds":receipt["query_bounds"],
            "pagination_complete":receipt["pagination_complete"],"supports_scoped_claim":receipt["supports_scoped_claim"],
            "metadata":receipt["provider"]["metadata"],"paths":receipt["provider"]["paths"],"scopes":receipt["provider"]["scopes"],
            "scope_pages":receipt["scope_pages"],"limitations":receipt["limitations"]
        });
        if context.to_string().len() > 14 * 1024 {
            return Err("graph context with coverage exceeds bound; narrow the task");
        }
        Ok((context, receipt))
    }

    async fn revalidate(&self, receipt: &Value) -> Result<(), &'static str> {
        let mut request = receipt["query_bounds"].clone();
        request["project"] = receipt["project"].clone();
        request["generation"] = receipt["generation"].clone();
        let current = self.coverage.checked(request).await?;
        for field in [
            "actual_project",
            "checkout_id",
            "source_head",
            "source_identity",
            "generation",
            "status",
        ] {
            if current[field] != receipt[field] {
                return Err(
                    "graph evidence changed; recheck coverage and source before relying on child findings",
                );
            }
        }
        if current["status"] == "stale" || current["status"] == "unavailable" {
            return Err(
                "graph evidence stale or unavailable; use direct source and limit the claim",
            );
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for GraphHandoffTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn label(&self) -> &str {
        self.inner.label()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn effects(&self) -> ToolEffects {
        self.inner.effects()
    }
    fn parameters(&self) -> Value {
        let mut schema = self.inner.parameters();
        schema["properties"]["graph_context"] = json!({"type":"object","description":"Optional bounded parent-supplied graph evidence. Receipt must come from this session's coverage check; no child MCP or inherited mutation authority.",
            "properties":{
                "evidence_id":{"type":"string"},"task_scope":{"type":"string","maxLength":512},
                "tier":{"type":"string","enum":["Verify","Scout","Auditor"],"default":"Verify"},
                "claim":{"type":"string","enum":["positive","scoped"],"default":"positive"},
                "sources":{"type":"array","maxItems":16,"items":{"type":"object","properties":{"path":{"type":"string"},"qualified_symbol":{"type":"string","maxLength":256},"origin":{"type":"string","enum":["graph_snippet","ordinary_read","parent_report"]}},"required":["path","qualified_symbol","origin"],"additionalProperties":false}},
                "relationships":{"type":"array","maxItems":16,"items":{"type":"object","properties":{"from":{"type":"string","maxLength":256},"to":{"type":"string","maxLength":256},"kind":{"type":"string","maxLength":64}},"required":["from","to","kind"],"additionalProperties":false}},
                "query_bounds":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":256}},
                "pagination":{"type":"string","enum":["complete","partial","unknown"]},
                "limitations":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":256}}
            },"required":["evidence_id","task_scope","sources","relationships","query_bounds","pagination","limitations"],"additionalProperties":false});
        schema
    }
    async fn execute(
        &self,
        id: &str,
        mut input: Value,
        update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::error::Result<ToolOutput> {
        if input.get("graph_context").is_none() {
            return self.inner.execute(id, input, update).await;
        }
        let (context, receipt) = match self.context(&input) {
            Ok(v) => v,
            Err(reason) => return Ok(failure(reason)),
        };
        if let Err(reason) = self.revalidate(&receipt).await {
            return Ok(failure(reason));
        }
        let Some(task) = input["task"].as_str().filter(|s| s.len() <= 16 * 1024) else {
            return Ok(failure("bounded task required"));
        };
        input["task"] = json!(format!(
            "{task}\n\nParent graph evidence (attributed, read-only, no inherited authority):\n{context}\nUse Verify by default; Scout findings are provisional. Broader Auditor conclusions require explicit checked scope and complete relevant pagination. Preserve all limitations in your report; cite direct source for any fallback. Do not claim completeness from a tier or clean result. Return findings to the parent; parent must revalidate before relying on them."
        ));
        input.as_object_mut().unwrap().remove("graph_context");
        let mut output = self.inner.execute(id, input, update).await?;
        let validation = self.revalidate(&receipt).await;
        let status = if validation.is_ok() {
            "revalidated"
        } else {
            "invalidated"
        };
        let limitations = validation.err().unwrap_or("Attributed child findings only. Revalidate this session receipt and current source before later reuse; retain limitations across compaction.");
        output.content.push(ContentBlock::Text(TextContent{text:json!({"graph_handoff":{"status":status,"context":context,"limitations":limitations}}).to_string(),text_signature:None}));
        output.is_error |= status == "invalidated";
        tracing::debug!(target:"temper::agent", event="codebase_memory.handoff", handoff.status=status, "agent: child graph evidence returned");
        Ok(output)
    }
}

fn validate_context(mut context: Value, receipt: &Value) -> Result<Value, &'static str> {
    let object = context
        .as_object_mut()
        .ok_or("graph context must be an object")?;
    if object.keys().any(|k| {
        !matches!(
            k.as_str(),
            "evidence_id"
                | "task_scope"
                | "tier"
                | "claim"
                | "sources"
                | "relationships"
                | "query_bounds"
                | "pagination"
                | "limitations"
        )
    }) {
        return Err("unknown graph context field");
    }
    bounded_text(object.get("task_scope").unwrap_or(&Value::Null), 512)?;
    object.entry("tier").or_insert(json!("Verify"));
    object.entry("claim").or_insert(json!("positive"));
    if !matches!(
        object.get("tier").unwrap_or(&Value::Null).as_str(),
        Some("Verify" | "Scout" | "Auditor")
    ) || !matches!(
        object.get("claim").unwrap_or(&Value::Null).as_str(),
        Some("positive" | "scoped")
    ) || !matches!(
        object.get("pagination").unwrap_or(&Value::Null).as_str(),
        Some("complete" | "partial" | "unknown")
    ) {
        return Err("invalid graph reporting contract");
    }
    if object.get("claim").unwrap_or(&Value::Null) == "scoped"
        && (object.get("tier").unwrap_or(&Value::Null) == "Scout"
            || object.get("pagination").unwrap_or(&Value::Null) != "complete"
            || receipt["supports_scoped_claim"] != true)
    {
        return Err("incomplete evidence cannot support a broader claim");
    }
    for (key, max) in [
        ("sources", 16),
        ("relationships", 16),
        ("query_bounds", 8),
        ("limitations", 8),
    ] {
        let items = object
            .get(key)
            .unwrap_or(&Value::Null)
            .as_array()
            .filter(|v| v.len() <= max)
            .ok_or("missing or oversized graph context list")?;
        for item in items {
            match key {
                "sources" => {
                    if item.as_object().is_none_or(|v| v.len() != 3) {
                        return Err("invalid source evidence");
                    }
                    bounded_text(&item["qualified_symbol"], 256)?;
                    let path = item["path"].as_str().ok_or("invalid source path")?;
                    request::safe_path(path, false)?;
                    if !receipt["query_bounds"]["paths"]
                        .as_array()
                        .is_some_and(|p| p.contains(&json!(path)))
                    {
                        return Err("every cited child source requires path coverage");
                    }
                    if !matches!(
                        item["origin"].as_str(),
                        Some("graph_snippet" | "ordinary_read" | "parent_report")
                    ) {
                        return Err("invalid source origin");
                    }
                }
                "relationships" => {
                    if item.as_object().is_none_or(|v| v.len() != 3) {
                        return Err("invalid relationship");
                    }
                    for field in ["from", "to", "kind"] {
                        bounded_text(&item[field], if field == "kind" { 64 } else { 256 })?;
                    }
                }
                _ => bounded_text(item, 256)?,
            }
        }
    }
    Ok(context)
}
fn bounded_text(value: &Value, max: usize) -> Result<(), &'static str> {
    if value
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
    {
        Ok(())
    } else {
        Err("graph evidence strings must be bounded single-line summaries")
    }
}
fn failure(reason: &str) -> ToolOutput {
    ToolOutput {
        content: vec![ContentBlock::Text(TextContent {
            text: reason.to_string(),
            text_signature: None,
        })],
        details: None,
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_handoff_cannot_support_scoped_claim_or_omit_contract_fields() {
        let receipt =
            json!({"supports_scoped_claim":false,"query_bounds":{"paths":["src/lib.rs"]}});
        assert!(validate_context(json!({"evidence_id":"x"}), &receipt).is_err());
        let mut context = json!({"task_scope":"src", "tier":"Auditor","claim":"scoped","sources":[],"relationships":[],"query_bounds":[],"pagination":"partial","limitations":["partial evidence"]});
        assert!(validate_context(context.clone(), &receipt).is_err());
        context["claim"] = json!("positive");
        assert!(validate_context(context.clone(), &receipt).is_ok());
        context["sources"] = json!([{"path":"src/uncited.rs","qualified_symbol":"example","origin":"ordinary_read"}]);
        assert!(validate_context(context, &receipt).is_err());
    }
}
