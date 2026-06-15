//! Provisioning error and result types.

use std::fmt;

/// Errors raised while building a [`ProvisionPlan`](crate::ProvisionPlan) or
/// running [`provision`](crate::provision).
#[derive(Debug)]
pub enum ProvisionError {
    /// A backend Forge operation failed.
    ///
    /// This replaces the host-specific REST error the Forgejo adapter used to
    /// surface: every backend call now flows through the portable
    /// [`ForgeError`](temper_forge::ForgeError).
    Backend(temper_forge::ForgeError),
    /// A provisioning input or backend response was structurally wrong (e.g. a
    /// workflow that declares no intake entry point, or a repository that is not
    /// readable after creation).
    Shape { what: String, detail: String },
    /// Reading a webhook-secret file (or another local file) failed.
    Io(std::io::Error),
}

impl fmt::Display for ProvisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProvisionError::Backend(err) => write!(formatter, "forge operation failed: {err}"),
            ProvisionError::Shape { what, detail } => {
                write!(
                    formatter,
                    "provisioning response '{what}' malformed: {detail}"
                )
            }
            ProvisionError::Io(err) => write!(formatter, "reading provisioning file failed: {err}"),
        }
    }
}

impl std::error::Error for ProvisionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProvisionError::Backend(err) => Some(err),
            ProvisionError::Io(err) => Some(err),
            ProvisionError::Shape { .. } => None,
        }
    }
}

impl From<temper_forge::ForgeError> for ProvisionError {
    fn from(err: temper_forge::ForgeError) -> Self {
        Self::Backend(err)
    }
}

impl From<std::io::Error> for ProvisionError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Result alias for provisioning operations.
pub type Result<T> = std::result::Result<T, ProvisionError>;
