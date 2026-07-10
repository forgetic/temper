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
//! [`success`] (PR opening), [`failure`] (audit + attention), [`verdict`],
//! [`verdict_children`], and [`verdict_pr`] (routed transitions and their runtime
//! bindings), [`resolve`] (Forge artifact lookups), and [`progress`] (the trait
//! impl).

mod body_merge;
mod body_update;
mod claim;
mod coordinated;
mod failure;
mod progress;
mod resolve;
mod success;
mod verdict;
mod verdict_child_relations;
mod verdict_children;
mod verdict_pr;

use std::sync::Arc;

use temper_workflow::{CompiledWorkflow, ValidatedWorkflow};

use temper_forge::Forge;

/// Forge-backed applier for daemon-accepted worker results. See the module docs
/// for the application semantics.
pub struct ForgeApplier<F: Forge + ?Sized> {
    pub(crate) forge: Arc<F>,
    pub(crate) workflow: Arc<ValidatedWorkflow>,
    pub(crate) compiled: CompiledWorkflow,
    pub(crate) attention_labels: Vec<String>,
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
        self.attention_labels = labels;
        if !self
            .attention_labels
            .iter()
            .any(|label| label == "needs-human")
        {
            self.attention_labels.push("needs-human".to_string());
        }
        self
    }
}
