//! One closed exact-edit contract for admission and execution.

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::workspace_files::{MAX_FILE_BYTES, MAX_FILE_PATHS, validate_relative};

pub(crate) const MAX_EDITS_PER_FILE: usize = 64;
pub(crate) const MAX_EDITS: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditFilesInput {
    pub(crate) files: Vec<FileEdits>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileEdits {
    pub(crate) path: String,
    pub(crate) edits: Vec<ExactEdit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ExactEdit {
    pub(crate) old_text: String,
    pub(crate) new_text: String,
}

impl EditFilesInput {
    pub(crate) fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let input: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid edit_files input: {error}"))?;
        if input.files.is_empty() || input.files.len() > MAX_FILE_PATHS {
            return Err(format!(
                "files must contain 1 to {MAX_FILE_PATHS} existing files"
            ));
        }
        let mut paths = BTreeSet::new();
        let (mut edits, mut bytes) = (0, 0);
        for file in &input.files {
            validate_relative(&file.path)?;
            if !paths.insert(&file.path) {
                return Err(format!("duplicate file path: {}", file.path));
            }
            if file.edits.is_empty() || file.edits.len() > MAX_EDITS_PER_FILE {
                return Err(format!(
                    "each file requires 1 to {MAX_EDITS_PER_FILE} edits"
                ));
            }
            edits += file.edits.len();
            for edit in &file.edits {
                if edit.old_text.is_empty() {
                    return Err("oldText must be nonempty".into());
                }
                bytes += edit.old_text.len() + edit.new_text.len();
            }
        }
        if edits > MAX_EDITS || bytes > MAX_FILE_BYTES {
            return Err(format!(
                "edit batch exceeds {MAX_EDITS} edits or {MAX_FILE_BYTES} text bytes"
            ));
        }
        Ok(input)
    }
}
