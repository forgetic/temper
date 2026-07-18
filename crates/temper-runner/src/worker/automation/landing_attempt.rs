//! Paired debug measurements for direct automated merge execution.

use std::time::Instant;

use temper_forge::RepositoryId;
use temper_log::strip_provider_scheme;
use temper_workflow::{ArtifactSource, CompiledWorkflow, Effect};

use crate::scan::AutomatedWorkItem;

/// One direct merge execution, paired from immediately before the executor call
/// through its terminal automation outcome.
pub(super) struct LandingAttempt {
    repo: String,
    pr_number: u64,
    queue: String,
    transition: String,
    started: Instant,
}

impl LandingAttempt {
    pub(super) fn start(
        repo: &RepositoryId,
        compiled: &CompiledWorkflow,
        item: &AutomatedWorkItem,
    ) -> Option<Self> {
        let ArtifactSource::PullRequest { number } = item.target else {
            return None;
        };
        let is_merge = compiled
            .transitions()
            .iter()
            .find(|transition| transition.id == item.transition)
            .is_some_and(|transition| {
                transition
                    .effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::MergePullRequest))
            });
        if !is_merge {
            return None;
        }

        let attempt = Self {
            repo: strip_provider_scheme(repo.as_str()).to_string(),
            pr_number: number.get(),
            queue: item.queue.as_str().to_string(),
            transition: item.transition.as_str().to_string(),
            started: Instant::now(),
        };
        tracing::debug!(
            target: "temper::worker",
            measurement = "mechanical.landing_attempt",
            repo = attempt.repo.as_str(),
            pr.number = attempt.pr_number,
            queue = attempt.queue.as_str(),
            transition = attempt.transition.as_str(),
            landing.outcome = "started",
            "mechanical landing attempt started"
        );
        Some(attempt)
    }

    pub(super) fn finish(&self, outcome: &'static str) {
        tracing::debug!(
            target: "temper::worker",
            measurement = "mechanical.landing_attempt",
            repo = self.repo.as_str(),
            pr.number = self.pr_number,
            queue = self.queue.as_str(),
            transition = self.transition.as_str(),
            landing.outcome = outcome,
            duration_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "mechanical landing attempt {outcome}"
        );
    }
}
