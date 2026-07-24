use super::Diagnostic;
use std::error::Error;
use std::fmt;

/// Collection of diagnostics returned when validation fails.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationErrors {
    diagnostics: Vec<Diagnostic>,
}

impl ValidationErrors {
    /// Creates a collection from diagnostics. Intended for use by validation.
    pub(crate) fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self { diagnostics }
    }

    /// Returns the collected diagnostics.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Returns the number of diagnostics.
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Returns `true` if there are no diagnostics.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "workflow validation failed with {} diagnostic(s)",
            self.diagnostics.len()
        )?;
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n  - {diagnostic}")?;
        }
        Ok(())
    }
}

impl Error for ValidationErrors {}
