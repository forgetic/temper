//! Shared live-manifest contract for opaque provider-result anchor scenarios.
//!
//! The historical result-driven guidance scenario and the mapped #991 scenario
//! deliberately share only the generic fixture contract: a later call must use
//! a value minted by a successful earlier provider result, and the resulting
//! trace plus two current-root source reads must precede mutation. The validator
//! inspects ephemeral fixture state only; aggregate run evidence keeps no raw
//! values, paths, source, or provider payload.

use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    super::result_driven_guidance::validate(mcp, calls)
}
