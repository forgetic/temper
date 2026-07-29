// SPDX-License-Identifier: MPL-2.0

//! Bounded, manifest-declared live scenario stimuli.
//!
//! The parser owns bounds; this module owns orchestration-neutral dispatch. A
//! live topology adapter implements [`StimulusRuntime`] with real process,
//! runner, Forge/CI, and delivery operations. Script assertions never receive
//! this interface, which keeps them as after-convergence probes.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{CommitFile, ForgeContent, ItemNumber, RepositoryId, UpdateIssue};
use temper_workflow::{ArtifactSource, parse_metadata_block};

use super::process::{
    ChildGuard, engine_block_on, spawn_temper_standalone, wait_for_standalone_with_timeout,
};
use super::{LiveLogPaths, ScenarioBundle, TemperCommand};
use crate::forgejo_server::{ForgejoRunner, ForgejoServer};

/// One validated stimulus selected entirely by manifest data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StimulusSpec {
    pub id: String,
    pub kind: StimulusKind,
    pub timeout: Duration,
    pub max_attempts: u64,
}

impl StimulusSpec {
    pub(crate) fn action(&self) -> &'static str {
        self.kind.action()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StimulusKind {
    RestartTemper,
    RestartRunner,
    CiFailure {
        repo_id: String,
        workflow_path: PathBuf,
    },
    CiRecovery {
        repo_id: String,
        workflow_path: PathBuf,
    },
    RepeatDelivery {
        artifact: String,
        deliveries: u64,
    },
    /// Observe (without mutating) a durable provider-recovery generation.
    WaitProviderDeferred {
        artifact: String,
        generation: u32,
    },
    /// Advance an observed deferral with the harness-owned authenticated capability.
    ProviderHealthWake {
        artifact: String,
        expected_generation: u32,
        event_id: String,
    },
}

impl StimulusKind {
    pub fn action(&self) -> &'static str {
        match self {
            Self::RestartTemper => "temper.restart",
            Self::RestartRunner => "forgejo_runner.restart",
            Self::CiFailure { .. } => "ci.fail",
            Self::CiRecovery { .. } => "ci.recover",
            Self::RepeatDelivery { .. } => "delivery.repeat",
            Self::WaitProviderDeferred { .. } => "provider.wait_deferred",
            Self::ProviderHealthWake { .. } => "provider.health_wake",
        }
    }
}

/// Structured result retained for every attempted stimulus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StimulusOutcome {
    pub id: String,
    pub action: String,
    pub status: StimulusStatus,
    pub attempts: u64,
    pub timeout: Duration,
    pub duration: Duration,
    pub details: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StimulusStatus {
    Passed,
    Failed,
    TimedOut,
}

impl StimulusStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        }
    }
}

/// Operations available to declarative stimuli.
///
/// This interface is deliberately not exposed to script assertions. Concrete
/// live execution uses [`LiveStimulusRuntime`], while unit tests can use a
/// recording implementation without booting the real topology.
pub trait StimulusRuntime {
    fn restart_temper(&mut self, timeout: Duration) -> Result<String, String>;
    fn restart_runner(&mut self, timeout: Duration) -> Result<String, String>;
    fn set_ci_failure(
        &mut self,
        repo_id: &str,
        workflow_path: &std::path::Path,
        timeout: Duration,
    ) -> Result<String, String>;
    fn recover_ci(
        &mut self,
        repo_id: &str,
        workflow_path: &std::path::Path,
        timeout: Duration,
    ) -> Result<String, String>;
    fn repeat_delivery(
        &mut self,
        artifact: &str,
        deliveries: u64,
        timeout: Duration,
    ) -> Result<String, String>;
    fn wait_provider_deferred(
        &mut self,
        artifact: &str,
        generation: u32,
        timeout: Duration,
    ) -> Result<String, String>;
    fn provider_health_wake(
        &mut self,
        artifact: &str,
        expected_generation: u32,
        event_id: &str,
        timeout: Duration,
    ) -> Result<String, String>;
}

/// Failure includes all attempted outcomes so diagnostics remain retainable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StimulusFailure {
    pub message: String,
    pub outcomes: Vec<StimulusOutcome>,
}

impl StimulusFailure {
    pub(super) fn diagnostic(&self) -> String {
        let outcomes = self
            .outcomes
            .iter()
            .map(|outcome| {
                format!(
                    "{} action={} status={} attempts={} timeout_ms={} duration_ms={} details={:?}",
                    outcome.id,
                    outcome.action,
                    outcome.status.as_str(),
                    outcome.attempts,
                    outcome.timeout.as_millis(),
                    outcome.duration.as_millis(),
                    outcome.details
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!("{}; retained stimulus outcomes: {outcomes}", self.message)
    }
}

/// Dispatch stimuli in manifest order and stop after the first bounded failure.
///
/// Calls into external processes are checked against the deadline after they
/// return as well as before each attempt. The real adapters also pass the bound
/// into their own readiness loops, so a successful-but-late operation is never
/// reported as passed.
pub fn execute_stimuli(
    stimuli: &[StimulusSpec],
    runtime: &mut impl StimulusRuntime,
) -> Result<Vec<StimulusOutcome>, StimulusFailure> {
    let mut outcomes = Vec::with_capacity(stimuli.len());
    for stimulus in stimuli {
        let started = Instant::now();
        let mut attempts = 0;
        let mut details = Vec::new();
        let mut succeeded = false;
        let mut last_error = None;

        while attempts < stimulus.max_attempts && started.elapsed() < stimulus.timeout {
            attempts += 1;
            match dispatch(stimulus, runtime) {
                Ok(detail) if started.elapsed() < stimulus.timeout => {
                    details.push(detail);
                    succeeded = true;
                    break;
                }
                Ok(detail) => {
                    details.push(detail);
                    last_error = Some(format!(
                        "operation completed after the {}ms timeout",
                        stimulus.timeout.as_millis()
                    ));
                    break;
                }
                Err(error) => {
                    details.push(format!("attempt {attempts}: {error}"));
                    last_error = Some(error);
                }
            }
        }

        if succeeded {
            outcomes.push(outcome(
                stimulus,
                StimulusStatus::Passed,
                attempts,
                started.elapsed(),
                details,
            ));
            continue;
        }

        let status = if started.elapsed() >= stimulus.timeout {
            StimulusStatus::TimedOut
        } else {
            StimulusStatus::Failed
        };
        let error = last_error.unwrap_or_else(|| {
            format!(
                "no attempt completed within the {}ms timeout",
                stimulus.timeout.as_millis()
            )
        });
        outcomes.push(outcome(
            stimulus,
            status,
            attempts,
            started.elapsed(),
            details,
        ));
        return Err(StimulusFailure {
            message: format!(
                "stimulus `{}` ({}) {} after {attempts} attempt(s): {error}",
                stimulus.id,
                stimulus.action(),
                status.as_str()
            ),
            outcomes,
        });
    }
    Ok(outcomes)
}

fn outcome(
    stimulus: &StimulusSpec,
    status: StimulusStatus,
    attempts: u64,
    duration: Duration,
    details: Vec<String>,
) -> StimulusOutcome {
    StimulusOutcome {
        id: stimulus.id.clone(),
        action: stimulus.action().to_string(),
        status,
        attempts,
        timeout: stimulus.timeout,
        duration,
        details,
    }
}

fn dispatch(stimulus: &StimulusSpec, runtime: &mut impl StimulusRuntime) -> Result<String, String> {
    match &stimulus.kind {
        StimulusKind::RestartTemper => runtime.restart_temper(stimulus.timeout),
        StimulusKind::RestartRunner => runtime.restart_runner(stimulus.timeout),
        StimulusKind::CiFailure {
            repo_id,
            workflow_path,
        } => runtime.set_ci_failure(repo_id, workflow_path, stimulus.timeout),
        StimulusKind::CiRecovery {
            repo_id,
            workflow_path,
        } => runtime.recover_ci(repo_id, workflow_path, stimulus.timeout),
        StimulusKind::RepeatDelivery {
            artifact,
            deliveries,
        } => runtime.repeat_delivery(artifact, *deliveries, stimulus.timeout),
        StimulusKind::WaitProviderDeferred {
            artifact,
            generation,
        } => runtime.wait_provider_deferred(artifact, *generation, stimulus.timeout),
        StimulusKind::ProviderHealthWake {
            artifact,
            expected_generation,
            event_id,
        } => {
            runtime.provider_health_wake(artifact, *expected_generation, event_id, stimulus.timeout)
        }
    }
}

/// Resources borrowed from one running live manifest topology.
pub(super) struct LiveStimulusResources<'a> {
    pub(super) scenario: &'a ScenarioBundle,
    pub(super) server: &'a ForgejoServer,
    pub(super) runner: &'a mut ForgejoRunner,
    pub(super) temper: &'a TemperCommand,
    pub(super) bundle_dir: &'a std::path::Path,
    pub(super) logs: &'a LiveLogPaths,
    pub(super) scenario_run_id: &'a str,
    pub(super) standalone: &'a mut ChildGuard,
    pub(super) forge: &'a ForgejoForge,
    pub(super) repository: &'a RepositoryId,
    pub(super) issue: ItemNumber,
}

/// Execute declared stimuli against real Forgejo, runner, and Temper processes.
pub(super) fn execute_live_stimuli(
    stimuli: &[StimulusSpec],
    resources: LiveStimulusResources<'_>,
) -> Result<Vec<StimulusOutcome>, StimulusFailure> {
    let mut runtime = LiveStimulusRuntime {
        resources,
        temper_restarts: 0,
    };
    execute_stimuli(stimuli, &mut runtime)
}

struct LiveStimulusRuntime<'a> {
    resources: LiveStimulusResources<'a>,
    temper_restarts: u64,
}

impl StimulusRuntime for LiveStimulusRuntime<'_> {
    fn restart_temper(&mut self, timeout: Duration) -> Result<String, String> {
        self.temper_restarts += 1;
        self.resources.standalone.kill_and_wait()?;
        let archive = self.resources.logs.standalone_log.with_file_name(format!(
            "standalone.before-restart-{}.log",
            self.temper_restarts
        ));
        fs::copy(&self.resources.logs.standalone_log, &archive).map_err(|error| {
            format!(
                "archive pre-restart standalone log {} to {}: {error}",
                self.resources.logs.standalone_log.display(),
                archive.display()
            )
        })?;
        *self.resources.standalone = spawn_temper_standalone(
            self.resources.temper,
            self.resources.bundle_dir,
            &self.resources.logs.standalone_log,
            &self.resources.scenario.observability,
            self.resources.scenario_run_id,
        )?;
        wait_for_standalone_with_timeout(self.resources.standalone, timeout)?;
        Ok(format!(
            "standalone Temper restarted and became ready; pre-restart log retained at {}",
            archive.display()
        ))
    }

    fn restart_runner(&mut self, _timeout: Duration) -> Result<String, String> {
        let previous = self.resources.runner.name().to_string();
        let mut replacement = ForgejoRunner::register(self.resources.server)
            .map_err(|error| format!("register replacement forgejo-runner: {error}"))?;
        if !replacement.is_running() {
            return Err(format!(
                "replacement forgejo-runner exited immediately: {}",
                replacement.log_tail()
            ));
        }
        let current = replacement.name().to_string();
        *self.resources.runner = replacement;
        Ok(format!(
            "forgejo-runner replaced registration `{previous}` with `{current}`"
        ))
    }

    fn set_ci_failure(
        &mut self,
        repo_id: &str,
        workflow_path: &std::path::Path,
        _timeout: Duration,
    ) -> Result<String, String> {
        self.commit_ci_fixture(
            repo_id,
            workflow_path,
            "test: apply declared failing CI stimulus",
        )?;
        Ok(format!(
            "declared failing CI fixture applied from {}",
            workflow_path.display()
        ))
    }

    fn recover_ci(
        &mut self,
        repo_id: &str,
        workflow_path: &std::path::Path,
        _timeout: Duration,
    ) -> Result<String, String> {
        self.commit_ci_fixture(
            repo_id,
            workflow_path,
            "test: restore declared passing CI fixture",
        )?;
        Ok(format!(
            "declared passing CI fixture restored from {}",
            workflow_path.display()
        ))
    }

    fn repeat_delivery(
        &mut self,
        artifact: &str,
        deliveries: u64,
        _timeout: Duration,
    ) -> Result<String, String> {
        let issue = self.issue_for_artifact(artifact)?;
        let snapshot = engine_block_on(
            self.resources
                .forge
                .get_issue_by_number(self.resources.repository, issue),
        )
        .map_err(|error| format!("read repeated-delivery issue: {error}"))?
        .ok_or_else(|| format!("repeated-delivery issue #{} disappeared", issue))?;
        for delivery in 1..=deliveries {
            engine_block_on(self.resources.forge.update_issue(
                &snapshot.id,
                UpdateIssue {
                    // Re-applying the exact body emits a bounded, state-equivalent
                    // provider delivery without changing workflow state.
                    body: Some(snapshot.body.clone()),
                    ..UpdateIssue::default()
                },
            ))
            .map_err(|error| format!("repeat delivery {delivery}/{deliveries}: {error}"))?;
        }
        Ok(format!(
            "replayed {deliveries} bounded provider deliveries for `{artifact}`"
        ))
    }

    fn wait_provider_deferred(
        &mut self,
        artifact: &str,
        generation: u32,
        timeout: Duration,
    ) -> Result<String, String> {
        let issue = self.issue_for_artifact(artifact)?;
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = engine_block_on(
                self.resources
                    .forge
                    .get_issue_by_number(self.resources.repository, issue),
            )
            .map_err(|error| format!("read provider-deferred issue: {error}"))?
            .ok_or_else(|| format!("provider-deferred issue #{} disappeared", issue))?;
            if let Some(recovery) = parse_metadata_block(&snapshot.body)
                .map_err(|error| format!("parse provider-deferred metadata: {error}"))?
                .and_then(|metadata| metadata.provider_recovery)
            {
                recovery
                    .validate()
                    .map_err(|error| format!("invalid provider-deferred metadata: {error}"))?;
                if recovery.generation == generation {
                    return Ok(format!(
                        "observed durable provider deferral for `{artifact}` epoch {} generation {} cumulative_failures={} workspace={}",
                        recovery.failure_epoch,
                        recovery.generation,
                        recovery.cumulative_failure_count,
                        recovery.workstream_id,
                    ));
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "durable provider deferral `{artifact}` generation {generation} was not observed"
                ));
            }
            if let Some(status) = self.resources.standalone.try_wait()? {
                return Err(format!(
                    "standalone exited while waiting for provider deferral: {status:?}"
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn provider_health_wake(
        &mut self,
        artifact: &str,
        expected_generation: u32,
        event_id: &str,
        _timeout: Duration,
    ) -> Result<String, String> {
        use secrecy::SecretString;
        use temper_engine::{
            ProviderHealthSignal, ProviderHealthWakeOutcome, ProviderHealthWaker,
            provider_health_signature, system_clock,
        };

        let issue = self.issue_for_artifact(artifact)?;
        let snapshot = engine_block_on(
            self.resources
                .forge
                .get_issue_by_number(self.resources.repository, issue),
        )
        .map_err(|error| format!("read provider-health target: {error}"))?
        .ok_or_else(|| format!("provider-health issue #{} disappeared", issue))?;
        let recovery = parse_metadata_block(&snapshot.body)
            .map_err(|error| format!("parse provider-health target metadata: {error}"))?
            .and_then(|metadata| metadata.provider_recovery)
            .ok_or_else(|| format!("`{artifact}` is not provider-deferred"))?;
        if recovery.generation != expected_generation {
            return Err(format!(
                "provider-health wake expected generation {expected_generation}, observed {}",
                recovery.generation
            ));
        }
        let signal = ProviderHealthSignal {
            workstream_id: recovery.workstream_id.clone(),
            failure_epoch: recovery.failure_epoch,
            expected_generation,
            event_id: event_id.to_string(),
        };
        let signature = provider_health_signature(super::PROVIDER_HEALTH_SECRET, &signal);
        let waker = ProviderHealthWaker::new(
            std::sync::Arc::new(self.resources.forge.clone()),
            SecretString::from(super::PROVIDER_HEALTH_SECRET.to_string()),
            system_clock(),
        );
        let outcome = engine_block_on(waker.advance(
            self.resources.repository,
            ArtifactSource::Issue { number: issue },
            &signal,
            &signature,
        ))
        .map_err(|error| format!("authenticated provider-health wake failed: {error:?}"))?;
        match outcome {
            ProviderHealthWakeOutcome::Advanced | ProviderHealthWakeOutcome::Duplicate => {
                Ok(format!(
                    "authenticated provider-health wake `{event_id}` advanced `{artifact}` from generation {expected_generation}"
                ))
            }
            other => Err(format!(
                "provider-health wake `{event_id}` was not applied: {other:?}"
            )),
        }
    }
}

impl LiveStimulusRuntime<'_> {
    fn issue_for_artifact(&self, artifact: &str) -> Result<ItemNumber, String> {
        let Some(id) = artifact.strip_prefix("issue:").filter(|id| !id.is_empty()) else {
            return Err(format!(
                "live provider/delivery stimulus requires issue:<id>, got `{artifact}`"
            ));
        };
        // Runtime dispatch has already resolved this declared fixture id or
        // binding through its seeded-issue map and placed the exact number in
        // `resources.issue`. Do not resolve it a second time as a fixture id:
        // valid `bind = ...` aliases need not equal the fixture's own id.
        let _resolved_binding = id;
        Ok(self.resources.issue)
    }

    fn commit_ci_fixture(
        &self,
        repo_id: &str,
        workflow_path: &std::path::Path,
        message: &str,
    ) -> Result<(), String> {
        if repo_id != self.resources.scenario.repo.id {
            return Err(format!(
                "stimulus repository `{repo_id}` does not match live repository `{}`",
                self.resources.scenario.repo.id
            ));
        }
        let contents = fs::read(workflow_path).map_err(|error| {
            format!(
                "read CI stimulus fixture {}: {error}",
                workflow_path.display()
            )
        })?;
        engine_block_on(self.resources.forge.commit_file(
            self.resources.repository,
            CommitFile {
                path: self.resources.scenario.repo.ci_target.display().to_string(),
                contents,
                message: message.to_string(),
                branch: self.resources.scenario.repo.default_branch.clone(),
            },
        ))
        .map_err(|error| format!("commit CI stimulus fixture: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[derive(Default)]
    struct RecordingRuntime {
        calls: Vec<String>,
        fail_runner_once: bool,
        delay: Option<Duration>,
    }

    impl StimulusRuntime for RecordingRuntime {
        fn restart_temper(&mut self, _: Duration) -> Result<String, String> {
            self.calls.push("temper.restart".to_string());
            Ok("standalone restarted and became ready".to_string())
        }

        fn restart_runner(&mut self, _: Duration) -> Result<String, String> {
            self.calls.push("forgejo_runner.restart".to_string());
            if let Some(delay) = self.delay {
                thread::sleep(delay);
            }
            if self.fail_runner_once {
                self.fail_runner_once = false;
                Err("runner not ready".to_string())
            } else {
                Ok("runner restarted and registered".to_string())
            }
        }

        fn set_ci_failure(
            &mut self,
            repo: &str,
            _: &std::path::Path,
            _: Duration,
        ) -> Result<String, String> {
            self.calls.push(format!("ci.fail:{repo}"));
            Ok("failing CI fixture applied".to_string())
        }

        fn recover_ci(
            &mut self,
            repo: &str,
            _: &std::path::Path,
            _: Duration,
        ) -> Result<String, String> {
            self.calls.push(format!("ci.recover:{repo}"));
            Ok("passing CI fixture restored".to_string())
        }

        fn repeat_delivery(
            &mut self,
            artifact: &str,
            deliveries: u64,
            _: Duration,
        ) -> Result<String, String> {
            self.calls
                .push(format!("delivery.repeat:{artifact}:{deliveries}"));
            Ok(format!("delivered {deliveries} bounded duplicate wakes"))
        }

        fn wait_provider_deferred(
            &mut self,
            artifact: &str,
            generation: u32,
            _: Duration,
        ) -> Result<String, String> {
            self.calls
                .push(format!("provider.wait_deferred:{artifact}:{generation}"));
            Ok(format!("observed generation {generation}"))
        }

        fn provider_health_wake(
            &mut self,
            artifact: &str,
            expected_generation: u32,
            event_id: &str,
            _: Duration,
        ) -> Result<String, String> {
            self.calls.push(format!(
                "provider.health_wake:{artifact}:{expected_generation}:{event_id}"
            ));
            Ok("authenticated wake advanced".to_string())
        }
    }

    #[test]
    fn dispatches_in_manifest_order_and_retries_within_bounds() {
        let stimuli = vec![
            spec("restart", StimulusKind::RestartRunner, 2),
            spec(
                "ci-down",
                StimulusKind::CiFailure {
                    repo_id: "service".to_string(),
                    workflow_path: PathBuf::from("failing.yml"),
                },
                1,
            ),
            spec(
                "ci-up",
                StimulusKind::CiRecovery {
                    repo_id: "service".to_string(),
                    workflow_path: PathBuf::from("passing.yml"),
                },
                1,
            ),
            spec(
                "duplicate",
                StimulusKind::RepeatDelivery {
                    artifact: "issue:source".to_string(),
                    deliveries: 3,
                },
                1,
            ),
        ];
        let mut runtime = RecordingRuntime {
            fail_runner_once: true,
            ..RecordingRuntime::default()
        };

        let outcomes = execute_stimuli(&stimuli, &mut runtime).expect("stimuli pass");

        assert_eq!(outcomes.len(), 4);
        assert_eq!(outcomes[0].attempts, 2);
        assert_eq!(outcomes[0].status, StimulusStatus::Passed);
        assert_eq!(
            runtime.calls,
            [
                "forgejo_runner.restart",
                "forgejo_runner.restart",
                "ci.fail:service",
                "ci.recover:service",
                "delivery.repeat:issue:source:3",
            ]
        );
    }

    #[test]
    fn late_success_is_a_timed_out_failure_with_retained_diagnostics() {
        let stimulus = StimulusSpec {
            id: "slow-runner".to_string(),
            kind: StimulusKind::RestartRunner,
            timeout: Duration::from_millis(1),
            max_attempts: 1,
        };
        let mut runtime = RecordingRuntime {
            delay: Some(Duration::from_millis(5)),
            ..RecordingRuntime::default()
        };

        let failure = execute_stimuli(&[stimulus], &mut runtime).expect_err("must time out");

        assert_eq!(failure.outcomes.len(), 1);
        assert_eq!(failure.outcomes[0].status, StimulusStatus::TimedOut);
        assert!(failure.diagnostic().contains("completed after"));
    }

    fn spec(id: &str, kind: StimulusKind, max_attempts: u64) -> StimulusSpec {
        StimulusSpec {
            id: id.to_string(),
            kind,
            timeout: Duration::from_secs(1),
            max_attempts,
        }
    }
}
