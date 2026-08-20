//! Bounded, process-only circuit state for ordinary tool invocations.
//!
//! The state deliberately excludes `codebase_memory_*`: those wrappers own a
//! separate lifecycle and health circuit. Invocation identities are computed
//! only after provider normalization, recursively canonicalize object key
//! order, and are retained solely as fixed-width in-memory digests. Neither
//! source arguments nor any digest representation crosses the machine I/O
//! protocol.

use std::collections::VecDeque;

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tongs::model::ToolCall;

use super::{
    CODEBASE_MEMORY_TOOL_PREFIX, ToolFailureCategory, ToolFailureDiagnostic, ToolFailureReason,
};

/// Oldest failure identity is evicted first when this fixed per-run capacity
/// is reached. Existing entries keep their original insertion position when
/// their failure count changes, making eviction independent of lookup order.
pub(super) const ORDINARY_FAILURE_CAPACITY: usize = 64;
/// An identical retryable invocation may execute at most twice in one run: the
/// initial attempt and one retry. A third attempt settles as a local redirect.
pub(super) const ORDINARY_RETRY_EXECUTION_BUDGET: u8 = 2;

#[derive(Clone, Copy, Eq, PartialEq)]
struct InvocationFingerprint([u8; 32]);

#[derive(Clone, Copy)]
enum FailureState {
    NonRetryable,
    Retryable { failed_executions: u8 },
}

struct FailureEntry {
    fingerprint: InvocationFingerprint,
    state: FailureState,
}

#[derive(Default)]
pub(super) struct OrdinaryFailureCircuit {
    entries: VecDeque<FailureEntry>,
}

impl OrdinaryFailureCircuit {
    /// Returns a content-free local redirect for an invocation whose execution
    /// policy is exhausted. Graph calls always bypass this state.
    pub(super) fn redirect_for(&self, call: &ToolCall) -> Option<ToolFailureDiagnostic> {
        let fingerprint = fingerprint(call)?;
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.fingerprint == fingerprint)?;
        let reason = match entry.state {
            FailureState::NonRetryable => ToolFailureReason::RepeatedNonRetryable,
            FailureState::Retryable { failed_executions }
                if failed_executions >= ORDINARY_RETRY_EXECUTION_BUDGET =>
            {
                ToolFailureReason::RetryBudgetExhausted
            }
            FailureState::Retryable { .. } => return None,
        };
        Some(ToolFailureDiagnostic::new(
            ToolFailureCategory::CircuitRedirect,
            reason,
        ))
    }

    /// Records only the closed typed outcome. Successful corrected calls clear
    /// their own identity. Policy-precondition denials are omitted because the
    /// same mutation can become valid after required graph evidence is
    /// consumed; the decision-anchor state remains authoritative for it.
    pub(super) fn record_outcome(
        &mut self,
        call: &ToolCall,
        failure: Option<&ToolFailureDiagnostic>,
    ) {
        let Some(fingerprint) = fingerprint(call) else {
            return;
        };
        let Some(failure) = failure else {
            self.remove(fingerprint);
            return;
        };
        if failure.category == ToolFailureCategory::CircuitRedirect {
            return;
        }
        if failure.reason == ToolFailureReason::PolicyPrecondition {
            self.remove(fingerprint);
            return;
        }

        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.fingerprint == fingerprint)
        {
            entry.state = if failure.retryable {
                match entry.state {
                    FailureState::Retryable { failed_executions } => FailureState::Retryable {
                        failed_executions: failed_executions.saturating_add(1),
                    },
                    FailureState::NonRetryable => FailureState::NonRetryable,
                }
            } else {
                FailureState::NonRetryable
            };
            return;
        }

        if self.entries.len() == ORDINARY_FAILURE_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(FailureEntry {
            fingerprint,
            state: if failure.retryable {
                FailureState::Retryable {
                    failed_executions: 1,
                }
            } else {
                FailureState::NonRetryable
            },
        });
    }

    fn remove(&mut self, fingerprint: InvocationFingerprint) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.fingerprint == fingerprint)
        {
            self.entries.remove(index);
        }
    }
}

fn fingerprint(call: &ToolCall) -> Option<InvocationFingerprint> {
    if call.name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(b"temper-ordinary-tool-invocation-v1\0");
    hash_bytes(&mut digest, call.name.as_bytes());
    hash_value(&mut digest, &call.arguments);
    Some(InvocationFingerprint(digest.finalize().into()))
}

fn hash_bytes(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn hash_value(digest: &mut Sha256, value: &Value) {
    match value {
        Value::Null => digest.update([0]),
        Value::Bool(value) => digest.update([1, u8::from(*value)]),
        Value::Number(value) => {
            digest.update([2]);
            hash_bytes(digest, value.to_string().as_bytes());
        }
        Value::String(value) => {
            digest.update([3]);
            hash_bytes(digest, value.as_bytes());
        }
        Value::Array(values) => {
            digest.update([4]);
            digest.update((values.len() as u64).to_be_bytes());
            for value in values {
                hash_value(digest, value);
            }
        }
        Value::Object(values) => {
            digest.update([5]);
            digest.update((values.len() as u64).to_be_bytes());
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                hash_bytes(digest, key.as_bytes());
                hash_value(digest, &values[key]);
            }
        }
    }
}
