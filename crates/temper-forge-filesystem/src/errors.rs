use std::fmt;
use temper_forge_model::ForgeError;

pub(crate) fn backend_error(context: impl fmt::Display, error: impl fmt::Display) -> ForgeError {
    ForgeError::Backend(format!("{context}: {error}"))
}
