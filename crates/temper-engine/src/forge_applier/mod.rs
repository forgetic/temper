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
//! [`success`] (PR opening), [`pr_reuse`] (existing PR topology validation),
//! [`failure`] (audit + attention), [`verdict`], [`verdict_children`], and
//! [`verdict_pr`] (routed transitions and their runtime bindings), [`resolve`]
//! (Forge artifact lookups), and [`progress`] (the trait impl).

mod body_merge;
mod body_update;
mod claim;
mod coordinated;
mod failure;
mod pr_repair;
mod pr_reuse;
mod progress;
mod resolve;
mod success;
mod validation_audit;
mod verdict;
mod verdict_child_relations;
mod verdict_children;
mod verdict_pr;

use std::sync::Arc;

use temper_workflow::{
    ChildIssueLifecycleHook, CompiledWorkflow, NEEDS_HUMAN_LABEL, ValidatedWorkflow,
    requires_human_attention,
};

use temper_forge::Forge;

/// Forge-backed applier for daemon-accepted worker results. See the module docs
/// for the application semantics.
pub struct ForgeApplier<F: Forge + ?Sized> {
    pub(crate) forge: Arc<F>,
    pub(crate) workflow: Arc<ValidatedWorkflow>,
    pub(crate) compiled: CompiledWorkflow,
    pub(crate) attention_labels: Vec<String>,
    pub(crate) child_issue_hook: Option<Arc<dyn ChildIssueLifecycleHook>>,
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub fn new(forge: Arc<F>, workflow: Arc<ValidatedWorkflow>) -> Self {
        let compiled = workflow.compile();
        Self {
            forge,
            workflow,
            compiled,
            attention_labels: vec![NEEDS_HUMAN_LABEL.to_string()],
            child_issue_hook: None,
        }
    }

    /// Observes committed staged-child checkpoints. Intended for deterministic
    /// process-crash integration tests; ordinary daemon construction leaves it
    /// unset.
    #[must_use]
    pub fn with_child_issue_hook(mut self, hook: Arc<dyn ChildIssueLifecycleHook>) -> Self {
        self.child_issue_hook = Some(hook);
        self
    }

    pub fn with_attention_labels(mut self, labels: Vec<String>) -> Self {
        let labels = labels
            .into_iter()
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .collect::<Vec<_>>();
        self.attention_labels = labels;
        if !requires_human_attention(&self.attention_labels) {
            self.attention_labels.push(NEEDS_HUMAN_LABEL.to_string());
        }
        self
    }
}
