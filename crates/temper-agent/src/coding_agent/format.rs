//! Local rustfmt over an explicitly admitted set of existing source files.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use temper_agent_core::AgentContainmentContext;
use tongs::error::Result;
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

use crate::workspace_format::{FormatRustInput, MAX_FORMAT_PATHS};

mod process;
#[cfg(test)]
mod tests;
mod workspace;

pub(super) struct FormatRustTool {
    cwd: PathBuf,
    containment: AgentContainmentContext,
}

impl FormatRustTool {
    pub(super) fn new(cwd: &Path, containment: AgentContainmentContext) -> Self {
        Self {
            cwd: cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()),
            containment,
        }
    }
}

#[async_trait]
impl Tool for FormatRustTool {
    fn name(&self) -> &str {
        "format_rust"
    }

    fn description(&self) -> &str {
        "Format explicit existing Rust files with local rustfmt and project style settings. \
         Supply workspace-relative .rs paths and the project's Rust edition. Each file must \
         already have been read successfully; read newly created files before formatting. \
         Child modules are never traversed. All files are formatted and checked for changes \
         during formatting before replacement begins. Each replacement is atomic, but an I/O \
         failure during replacement can leave earlier files formatted."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "paths": {"type": "array", "items": {"type": "string"},
                    "minItems": 1, "maxItems": MAX_FORMAT_PATHS, "uniqueItems": true},
                "edition": {"type": "string", "enum": ["2015", "2018", "2021", "2024"]}
            },
            "required": ["paths", "edition"],
            "additionalProperties": false
        })
    }

    fn effects(&self) -> ToolEffects {
        ToolEffects::write()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let result = FormatRustInput::parse(&input).and_then(|input| {
            let formatted = workspace::prepare(&self.cwd, &input, &self.containment)?;
            workspace::commit(&self.cwd, formatted)
        });
        Ok(match result {
            Ok(changed) => ToolOutput::text(format!("Formatted {changed} Rust file(s)")),
            Err(error) => ToolOutput::error(format!("format_rust: {error}")),
        })
    }
}
