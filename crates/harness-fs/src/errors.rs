use harness_forge::{ForgeError, ForgeResult};
use std::fmt;

pub(crate) fn unsupported<T>(operation: &str) -> ForgeResult<T> {
    Err(ForgeError::InvalidRequest(format!(
        "filesystem backend does not support {operation} yet"
    )))
}

pub(crate) fn backend_error(context: impl fmt::Display, error: impl fmt::Display) -> ForgeError {
    ForgeError::Backend(format!("{context}: {error}"))
}
