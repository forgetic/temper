// SPDX-License-Identifier: MPL-2.0

//! Pure pre-I/O validation for bounded context operations.

use temper_protocol_context::{ForgeContextErrorCode, ForgeContextOperation};

use super::response::{resolve_repository, validate_identity};
use super::{
    DEFAULT_RELATED_DEPTH, DEFAULT_RELATED_RESULTS, MAX_RELATED_DEPTH, MAX_RELATED_RESULTS,
};
use crate::artifact_context::catalog::ConfiguredRepositoryCatalog;

/// Validates identity, repository authorization, and operation-specific limits
/// without touching Forge. Transport handlers call this before scheduling a
/// read; the service repeats the check as defense in depth.
pub fn validate_context_operation(
    operation: &ForgeContextOperation,
    catalog: &ConfiguredRepositoryCatalog,
) -> Result<(), ForgeContextErrorCode> {
    validate_identity(operation.repository(), operation.number())?;
    resolve_repository(catalog, operation.repository())?;
    if let ForgeContextOperation::ForgeListRelated(operation) = operation {
        if operation.relations.is_empty() {
            return Err(ForgeContextErrorCode::InvalidRequest);
        }
        if operation.relations.len() > 7 {
            return Err(ForgeContextErrorCode::LimitExceeded);
        }
        let depth = operation.depth.unwrap_or(DEFAULT_RELATED_DEPTH);
        let limit = operation.limit.unwrap_or(DEFAULT_RELATED_RESULTS);
        if depth == 0 || depth > MAX_RELATED_DEPTH || limit == 0 || limit > MAX_RELATED_RESULTS {
            return Err(ForgeContextErrorCode::LimitExceeded);
        }
    }
    Ok(())
}
