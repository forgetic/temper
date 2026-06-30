// SPDX-License-Identifier: MPL-2.0

use std::fmt;

/// A diagnostic severity emitted by the manifest checker.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Severity {
    /// A blocking validation problem.
    Error,
    /// A non-blocking note. The current checker only emits errors, but the
    /// variant keeps the report shape stable for advisory checks.
    Warning,
}

impl Severity {
    /// Stable lowercase text for terminal and machine-readable reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A human-readable manifest validation diagnostic.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Diagnostic {
    /// Diagnostic severity.
    pub severity: Severity,
    /// Manifest field path, when the diagnostic is tied to one field.
    pub field: Option<String>,
    /// Human-readable message.
    pub message: String,
}

impl Diagnostic {
    /// Creates an error diagnostic for a manifest field.
    pub fn error(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            field: Some(field.into()),
            message: message.into(),
        }
    }

    /// Creates an error diagnostic for the whole manifest/document.
    pub fn document_error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            field: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.field {
            Some(field) => write!(f, "{}: {field}: {}", self.severity, self.message),
            None => write!(f, "{}: {}", self.severity, self.message),
        }
    }
}
