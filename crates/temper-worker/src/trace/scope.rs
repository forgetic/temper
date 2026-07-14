use std::collections::BTreeMap;

use temper_protocol_activity::{AgentScopeKindV1, AgentScopeV1};

use super::TraceError;

/// Maps the child-minted root onto the worker's unique canonical root. Host
/// boundaries are written before a first-party child connects, so this binding
/// keeps one valid ancestry while retaining child IDs for every nested scope.
pub(super) fn canonicalize_child_scope(
    source_main_scope_id: &mut Option<String>,
    canonical_main: &AgentScopeV1,
    mut scope: AgentScopeV1,
) -> Result<AgentScopeV1, TraceError> {
    match scope.kind {
        AgentScopeKindV1::Main => {
            if let Some(source_main) = source_main_scope_id.as_deref() {
                if source_main != scope.id {
                    return Err(TraceError::InvalidSpool(
                        "child attempted to introduce a second main scope".to_string(),
                    ));
                }
            } else {
                *source_main_scope_id = Some(scope.id.clone());
            }
            Ok(canonical_main.clone())
        }
        AgentScopeKindV1::SubAgent => {
            if scope.parent_id.as_deref() == source_main_scope_id.as_deref() {
                scope.parent_id = Some(canonical_main.id.clone());
            }
            Ok(scope)
        }
    }
}

pub(super) fn validate_scope_acceptance(
    scopes: &BTreeMap<String, AgentScopeV1>,
    scope: &AgentScopeV1,
) -> Result<(), TraceError> {
    if let Some(existing) = scopes.get(&scope.id) {
        if existing != scope {
            return Err(TraceError::InvalidSpool(format!(
                "scope {} changed kind or parent",
                scope.id
            )));
        }
        return Ok(());
    }
    match scope.kind {
        AgentScopeKindV1::Main => Err(TraceError::InvalidSpool(
            "child main scope was not canonicalized".to_string(),
        )),
        AgentScopeKindV1::SubAgent => {
            let parent = scope.parent_id.as_deref().ok_or_else(|| {
                TraceError::InvalidSpool("sub-agent scope has no parent".to_string())
            })?;
            if scopes.contains_key(parent) {
                Ok(())
            } else {
                Err(TraceError::InvalidSpool(format!(
                    "scope {} references unaccepted parent {parent}",
                    scope.id
                )))
            }
        }
    }
}
