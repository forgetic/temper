use std::path::{Path, PathBuf};
use std::time::Duration;

use super::codebase_memory::CodebaseMemoryFake;
use super::fake_llm::SinglePullRequestFake;
use super::handoff::fake::HandoffFake;
use super::plan_feature::fake::PlanFeatureFake;
use super::{ConvergenceStrategy, FakeLlmEvidence, LateStreamFailureFixture};

pub(super) enum ManifestFake {
    Single {
        script_path: PathBuf,
        fake: SinglePullRequestFake,
    },
    CodebaseMemory {
        script_path: PathBuf,
        fake: CodebaseMemoryFake,
    },
    Handoff {
        script_path: PathBuf,
        fake: HandoffFake,
    },
    PlanFeature {
        script_path: PathBuf,
        fake: PlanFeatureFake,
    },
}

impl ManifestFake {
    pub(super) fn start(
        strategy: ConvergenceStrategy,
        script_path: &Path,
        late_stream_failure: Option<&LateStreamFailureFixture>,
    ) -> Result<Self, String> {
        if late_stream_failure.is_some() && strategy != ConvergenceStrategy::SinglePullRequest {
            return Err(
                "late streamed failure injection currently requires single-pull-request convergence"
                    .to_string(),
            );
        }
        let script_path = script_path.to_path_buf();
        match strategy {
            ConvergenceStrategy::SinglePullRequest
            | ConvergenceStrategy::ImplementationPrTerminalCi => Ok(Self::Single {
                fake: SinglePullRequestFake::start(&script_path, late_stream_failure)?,
                script_path,
            }),
            ConvergenceStrategy::CodebaseMemory => Ok(Self::CodebaseMemory {
                fake: CodebaseMemoryFake::start(&script_path)?,
                script_path,
            }),
            ConvergenceStrategy::ImplementationPrHandoff => Ok(Self::Handoff {
                fake: HandoffFake::start(&script_path)?,
                script_path,
            }),
            ConvergenceStrategy::PlanFeatureLanding => Ok(Self::PlanFeature {
                fake: PlanFeatureFake::start(&script_path)?,
                script_path,
            }),
        }
    }

    pub(super) fn script_path(&self) -> &Path {
        match self {
            Self::Single { script_path, .. }
            | Self::CodebaseMemory { script_path, .. }
            | Self::Handoff { script_path, .. }
            | Self::PlanFeature { script_path, .. } => script_path,
        }
    }

    pub(super) fn base_url(&self) -> String {
        match self {
            Self::Single { fake, .. } => fake.base_url(),
            Self::CodebaseMemory { fake, .. } => fake.base_url(),
            Self::Handoff { fake, .. } => fake.base_url(),
            Self::PlanFeature { fake, .. } => fake.base_url(),
        }
    }

    pub(super) fn log_tail(&self) -> String {
        match self {
            Self::Single { fake, .. } => fake.log_tail(),
            Self::CodebaseMemory { fake, .. } => fake.log_tail(),
            Self::Handoff { fake, .. } => fake.log_tail(),
            Self::PlanFeature { fake, .. } => fake.log_tail(),
        }
    }

    pub(super) fn evidence(&self, log_path: &Path) -> FakeLlmEvidence {
        let (architect_requests, engineer_requests, tester_requests) = match self {
            Self::Single { fake, .. } => (fake.architect_requests(), fake.engineer_requests(), 0),
            Self::CodebaseMemory { fake, .. } => (0, fake.engineer_requests(), 0),
            Self::Handoff { fake, .. } => (0, fake.engineer_requests(), 0),
            Self::PlanFeature { fake, .. } => (
                fake.architect_requests(),
                fake.engineer_requests(),
                fake.tester_requests(),
            ),
        };
        FakeLlmEvidence {
            base_url: self.base_url(),
            architect_requests,
            engineer_requests,
            tester_requests,
            log_path: log_path.to_path_buf(),
        }
    }

    pub(super) fn validate_after_convergence(
        &self,
        strategy: ConvergenceStrategy,
    ) -> Result<(), String> {
        match (strategy, self) {
            (
                ConvergenceStrategy::SinglePullRequest
                | ConvergenceStrategy::ImplementationPrTerminalCi,
                Self::Single { fake, .. },
            ) => {
                if fake.architect_requests() < 2 {
                    return Err(format!(
                        "fake LLM never served the architect tool loop\n{}",
                        fake.log_tail()
                    ));
                }
                if fake.engineer_requests() < 2 {
                    return Err(format!(
                        "fake LLM never served the engineer tool loop\n{}",
                        fake.log_tail()
                    ));
                }
                Ok(())
            }
            (
                ConvergenceStrategy::SinglePullRequest
                | ConvergenceStrategy::ImplementationPrTerminalCi,
                _,
            ) => Err(
                "single implementation PR convergence requires its declared Jig runtime"
                    .to_string(),
            ),
            _ => Ok(()),
        }
    }

    pub(super) fn wait_for_handoff_refresh(&self, timeout: Duration) -> Result<(), String> {
        if let Self::Handoff { fake, .. } = self {
            fake.wait_for_refresh_started(timeout)?;
        }
        Ok(())
    }

    pub(super) fn allow_handoff_refresh(&self) {
        if let Self::Handoff { fake, .. } = self {
            fake.allow_refresh_continue();
        }
    }

    pub(super) fn codebase(&self) -> Result<&CodebaseMemoryFake, String> {
        match self {
            Self::CodebaseMemory { fake, .. } => Ok(fake),
            _ => Err("codebase-memory convergence requires its declared Jig runtime".to_string()),
        }
    }

    pub(super) fn plan_feature(&self) -> Result<&PlanFeatureFake, String> {
        match self {
            Self::PlanFeature { fake, .. } => Ok(fake),
            _ => Err("plan-feature convergence requires its declared Jig runtime".to_string()),
        }
    }
}
