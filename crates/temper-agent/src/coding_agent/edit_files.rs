//! Batched exact edits over explicitly admitted existing files.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tongs::error::Result;
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

use crate::workspace_edits::{EditFilesInput, MAX_EDITS_PER_FILE};
use crate::workspace_files::{MAX_FILE_BYTES, MAX_FILE_PATHS};

use super::file_updates::{self, FileUpdate};

mod replace;
#[cfg(test)]
mod tests;

pub(super) struct EditFilesTool {
    cwd: PathBuf,
}

impl EditFilesTool {
    pub(super) fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()),
        }
    }
}

#[async_trait]
impl Tool for EditFilesTool {
    fn name(&self) -> &str {
        "edit_files"
    }

    fn description(&self) -> &str {
        "Edit a complete set of already-read existing files in one call. Supply explicit \
         workspace-relative paths and edits with oldText/newText. Every oldText must match \
         exactly once in its original file; matches must not overlap. Whitespace, line endings \
         and all unedited bytes are preserved exactly. Edits refer to original contents, never \
         earlier replacements. All targets and matches are checked before replacement begins. \
         Each replacement is atomic; a late I/O failure can leave earlier files changed. \
         This tool cannot create files."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object", "additionalProperties": false, "required": ["files"],
            "properties": {"files": {"type": "array", "minItems": 1, "maxItems": MAX_FILE_PATHS,
                "items": {"type": "object", "additionalProperties": false, "required": ["path", "edits"],
                    "properties": {
                        "path": {"type": "string", "minLength": 1, "maxLength": 4096},
                        "edits": {"type": "array", "minItems": 1, "maxItems": MAX_EDITS_PER_FILE,
                            "items": {"type": "object", "additionalProperties": false,
                                "required": ["oldText", "newText"], "properties": {
                                    "oldText": {"type": "string", "minLength": 1, "maxLength": MAX_FILE_BYTES},
                                    "newText": {"type": "string", "maxLength": MAX_FILE_BYTES}
                                }}}
                    }}}}
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
        let result = EditFilesInput::parse(&input).and_then(|input| {
            let files = prepare(&self.cwd, &input)?;
            file_updates::commit(&self.cwd, files, "edited")
        });
        Ok(match result {
            Ok(changed) => ToolOutput::text(format!("Edited {changed} file(s)")),
            Err(error) => ToolOutput::error(format!("edit_files: {error}")),
        })
    }
}

fn prepare(root: &Path, input: &EditFilesInput) -> std::result::Result<Vec<FileUpdate>, String> {
    let mut files = file_updates::load(root, input.files.iter().map(|file| file.path.as_str()))?;
    let mut outputs = Vec::new();
    let mut output_bytes = 0;
    for (file, edits) in files.iter().zip(&input.files) {
        let output = replace::apply(file.original(), &edits.edits)
            .map_err(|error| format!("{}: {error}", edits.path))?;
        output_bytes += output.len();
        if output_bytes > MAX_FILE_BYTES * 2 {
            return Err("aggregate edited output exceeds limit".into());
        }
        outputs.push(output);
    }
    // All original matches and outputs have been validated before any staging.
    for (file, output) in files.iter_mut().zip(outputs) {
        file.stage(&output)?;
    }
    Ok(files)
}
