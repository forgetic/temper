//! Read-only machine state accessors used by the shell and deterministic tests.

use tongs::model::Message;

use super::{AgentMachine, Phase};
use crate::machine::{BatchGeneration, OperationGeneration};

impl AgentMachine {
    pub(super) fn next_operation_generation(&mut self) -> OperationGeneration {
        let generation = self.next_operation_generation;
        self.next_operation_generation = self
            .next_operation_generation
            .checked_add(1)
            .expect("agent operation generation exhausted");
        generation
    }

    pub(super) fn next_batch_generation(&mut self) -> BatchGeneration {
        let generation = self.next_batch_generation;
        self.next_batch_generation = self
            .next_batch_generation
            .checked_add(1)
            .expect("agent batch generation exhausted");
        generation
    }

    /// The current conversation (test/observability accessor).
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The operation/batch identity currently allowed to complete. This is
    /// primarily useful to deterministic protocol tests that synthesize shell
    /// completions without running an executor.
    pub fn active_generations(&self) -> Option<(OperationGeneration, BatchGeneration)> {
        match self.phase {
            Phase::AwaitingLlm => self.active_llm.map(|operation| (operation, 0)),
            Phase::AwaitingTools => self.active_tool_batch.as_ref().and_then(|batch| {
                batch
                    .operations
                    .values()
                    .next()
                    .copied()
                    .map(|operation| (operation, batch.generation))
            }),
            Phase::Cancelling => self.cancellation_generation,
            Phase::Done => None,
        }
    }

    /// The operation/batch identity currently allowed to complete for `id`.
    /// Returns `None` unless that exact tool call is in the active batch.
    pub fn active_tool_generations(
        &self,
        id: &str,
    ) -> Option<(OperationGeneration, BatchGeneration)> {
        let batch = self.active_tool_batch.as_ref()?;
        batch
            .operations
            .get(id)
            .copied()
            .map(|operation| (operation, batch.generation))
    }
}
