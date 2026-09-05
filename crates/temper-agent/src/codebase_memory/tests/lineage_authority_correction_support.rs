struct OrdinaryReadTool;

#[async_trait]
impl Tool for OrdinaryReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "test-only ordinary read"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    }

    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }

    async fn execute(
        &self,
        _: &str,
        _: serde_json::Value,
        _: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> tongs::Result<ToolOutput> {
        unreachable!("the test completes ordinary reads directly")
    }
}

fn opaque_references(text: &str) -> Vec<String> {
    let mut references = Vec::new();
    let mut remaining = text;
    while let Some(index) = remaining.find(REFERENCE_PREFIX) {
        let candidate = &remaining[index..];
        let length = REFERENCE_PREFIX.len() + 36;
        if candidate.len() < length {
            break;
        }
        let reference = candidate[..length].to_string();
        if !references.contains(&reference) {
            references.push(reference);
        }
        remaining = &candidate[length..];
    }
    references
}

fn successful_output() -> ToolOutput {
    ToolOutput {
        content: Vec::new(),
        details: None,
        is_error: false,
    }
}
