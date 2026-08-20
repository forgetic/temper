use std::sync::Mutex;

use futures::lock::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use temper_agent_core::ToolFailureCategory;

use crate::mcp::McpCancellationHandle;

/// Circuit state shared by every codebase-memory wrapper in one toolset/run.
///
/// The async gate mirrors the serving client's serialized stdio transport, but
/// lets a queued wrapper re-check the circuit before it enters MCP. Without
/// that check, parallel wrappers could line up behind a request that has
/// already made the shared process unusable.
pub(super) struct CodebaseMemoryHealth {
    state: Mutex<HealthState>,
    rpc_gate: AsyncMutex<()>,
    cancellation: McpCancellationHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HealthState {
    Healthy,
    Open { cause: ToolFailureCategory },
}

impl CodebaseMemoryHealth {
    pub(super) fn new(cancellation: McpCancellationHandle) -> Self {
        Self {
            state: Mutex::new(HealthState::Healthy),
            rpc_gate: AsyncMutex::new(()),
            cancellation,
        }
    }

    pub(super) fn open_cause(&self) -> Option<ToolFailureCategory> {
        match *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            HealthState::Healthy => None,
            HealthState::Open { cause } => Some(cause),
        }
    }

    pub(super) async fn acquire_rpc(&self) -> AsyncMutexGuard<'_, ()> {
        self.rpc_gate.lock().await
    }

    /// Opens the run-local circuit for systemic failures only. The first cause
    /// is retained deterministically and raw failure text is never stored.
    pub(super) fn record_failure(&self, category: ToolFailureCategory) {
        if !opens_run_circuit(category) {
            return;
        }
        let opened = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if matches!(*state, HealthState::Healthy) {
                *state = HealthState::Open { cause: category };
                true
            } else {
                false
            }
        };
        if opened {
            // There can be no useful future request on this serving process.
            // Cleanup is owned outside the MCP request mutex, so this remains
            // safe when a timeout or cancellation has a request in flight.
            self.cancellation.request_cancel();
        }
    }
}

fn opens_run_circuit(category: ToolFailureCategory) -> bool {
    !matches!(
        category,
        // Both outcomes are request/lifecycle local rather than evidence that
        // the shared provider process is unusable.
        ToolFailureCategory::InvalidModelInput | ToolFailureCategory::GraphLifecycleDenial
    )
}
