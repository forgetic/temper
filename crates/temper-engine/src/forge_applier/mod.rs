// SPDX-License-Identifier: MPL-2.0

//! Forge-backed [`ResultApplier`] for daemon-accepted worker results.
//!
//! Successful issue-targeted worker results carrying a branch are turned into the
//! same implementation-PR creation input the runner workspace paths use, then
//! passed to [`temper_workflow::Executor::ensure_pull_request`] with the
//! deterministic workspace correlation key. Permanent/protocol worker failures
//! mark the source issue for human attention and add an audit comment. Verdict
//! results route through the compiled workflow. It deliberately does not acquire
//! or release leases; compose it under [`crate::LeaseApplier`] when real daemon
//! application is enabled.
//!
//! The implementation is split by responsibility across child modules:
//! [`success`] (PR opening), [`failure`] (audit + attention), [`verdict`] and
//! [`verdict_children`] (routed transitions), [`resolve`] (Forge artifact
//! lookups), and [`progress`] (the trait impl).

mod body_merge;
mod body_update;
mod claim;
mod coordinated;
mod failure;
mod progress;
mod resolve;
mod run_ledger;
mod success;
mod verdict;
mod verdict_children;

use std::sync::Arc;

use temper_workflow::{CompiledWorkflow, ValidatedWorkflow};

use temper_forge::Forge;

/// Forge-backed applier for daemon-accepted worker results. See the module docs
/// for the application semantics.
pub struct ForgeApplier<F: Forge + ?Sized> {
    forge: Arc<F>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: CompiledWorkflow,
    attention_labels: Vec<String>,
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub fn new(forge: Arc<F>, workflow: Arc<ValidatedWorkflow>) -> Self {
        let compiled = workflow.compile();
        Self {
            forge,
            workflow,
            compiled,
            attention_labels: vec!["needs-human".to_string()],
        }
    }

    pub fn with_attention_labels(mut self, labels: Vec<String>) -> Self {
        let labels = labels
            .into_iter()
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .collect::<Vec<_>>();
        self.attention_labels = if labels.is_empty() {
            vec!["needs-human".to_string()]
        } else {
            labels
        };
        self
    }
}
