//! Format an entire explicit set before replacing any source file.

use std::path::Path;
use std::time::{Duration, Instant};

use temper_agent_core::AgentContainmentContext;

use crate::coding_agent::file_updates::{self, FileUpdate};
use crate::workspace_format::{FormatRustInput, MAX_FORMAT_BYTES};

use super::process::format_source;

pub(super) fn prepare(
    root: &Path,
    input: &FormatRustInput,
    containment: &AgentContainmentContext,
) -> Result<Vec<FileUpdate>, String> {
    let mut files = file_updates::load(root, input.paths.iter().map(String::as_str))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut output_bytes = 0;
    for file in &mut files {
        let output = format_source(
            file.parent()?,
            file.original(),
            input.edition,
            deadline,
            containment,
        )?;
        output_bytes += output.len();
        if output_bytes > MAX_FORMAT_BYTES * 2 {
            return Err("aggregate formatter output exceeds limit".into());
        }
        file.stage(&output)?;
    }
    Ok(files)
}

pub(super) fn commit(root: &Path, files: Vec<FileUpdate>) -> Result<usize, String> {
    file_updates::commit(root, files, "formatted")
}
