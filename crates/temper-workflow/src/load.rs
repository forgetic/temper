// SPDX-License-Identifier: MPL-2.0

//! File loading helpers for workflow specifications.

use std::error::Error;
use std::fmt;
use std::path::Path;

use crate::spec::RawWorkflowSpec;
use crate::validated::ValidatedWorkflow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkflowFileFormat {
    Json,
    Yaml,
}

impl WorkflowFileFormat {
    fn for_path(path: &Path) -> Self {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml") {
            WorkflowFileFormat::Yaml
        } else {
            WorkflowFileFormat::Json
        }
    }

    fn name(self) -> &'static str {
        match self {
            WorkflowFileFormat::Json => "JSON",
            WorkflowFileFormat::Yaml => "YAML",
        }
    }
}

/// Failure loading a runtime-selected workflow from a file.
#[derive(Debug)]
pub enum WorkflowLoadError {
    /// The workflow file could not be read.
    Read {
        path: String,
        source: std::io::Error,
    },
    /// The workflow file is not valid JSON/YAML for [`RawWorkflowSpec`].
    Parse { path: String, detail: String },
    /// The workflow parsed but failed static validation.
    Validate { path: String, detail: String },
}

impl fmt::Display for WorkflowLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkflowLoadError::Read { path, source } => {
                write!(formatter, "failed to read workflow file {path}: {source}")
            }
            WorkflowLoadError::Parse { path, detail } => {
                let format = WorkflowFileFormat::for_path(Path::new(path)).name();
                write!(
                    formatter,
                    "workflow file {path} is not valid {format}: {detail}"
                )
            }
            WorkflowLoadError::Validate { path, detail } => {
                write!(
                    formatter,
                    "workflow file {path} failed validation:\n{detail}"
                )
            }
        }
    }
}

impl Error for WorkflowLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            WorkflowLoadError::Read { source, .. } => Some(source),
            WorkflowLoadError::Parse { .. } | WorkflowLoadError::Validate { .. } => None,
        }
    }
}

/// Parses a workflow document from `contents` using the format selected by
/// `path`'s extension (`.yaml`/`.yml` for YAML, JSON otherwise).
pub fn parse_workflow_spec(
    path: impl AsRef<Path>,
    contents: &str,
) -> Result<RawWorkflowSpec, WorkflowLoadError> {
    let path = path.as_ref();
    let display = path.display().to_string();
    match WorkflowFileFormat::for_path(path) {
        WorkflowFileFormat::Json => {
            serde_json::from_str(contents).map_err(|error| WorkflowLoadError::Parse {
                path: display,
                detail: error.to_string(),
            })
        }
        WorkflowFileFormat::Yaml => {
            serde_yaml::from_str(contents).map_err(|error| WorkflowLoadError::Parse {
                path: display,
                detail: error.to_string(),
            })
        }
    }
}

/// Loads a raw workflow spec from `path` without validating it.
pub fn load_workflow_spec(path: impl AsRef<Path>) -> Result<RawWorkflowSpec, WorkflowLoadError> {
    let path = path.as_ref();
    let display = path.display().to_string();
    let contents = std::fs::read_to_string(path).map_err(|source| WorkflowLoadError::Read {
        path: display,
        source,
    })?;
    parse_workflow_spec(path, &contents)
}

/// Loads and validates a workflow from `path`.
pub fn load_workflow(path: impl AsRef<Path>) -> Result<ValidatedWorkflow, WorkflowLoadError> {
    let path = path.as_ref();
    let display = path.display().to_string();
    let spec = load_workflow_spec(path)?;
    spec.validate()
        .map_err(|errors| WorkflowLoadError::Validate {
            path: display,
            detail: errors.to_string(),
        })
}
