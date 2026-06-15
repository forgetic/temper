//! Backend- and topology-agnostic scenario definitions and runners.

use super::{Stage, StageError};
use crate::driver::RunReport;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use temper_forge::{Forge, RepositoryId};

/// Default tick budget used by [`run_scenario`].
pub const DEFAULT_SCENARIO_BUDGET: u64 = 100;

/// Boxed error type used by scenario seed/assert closures.
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Future returned by a scenario seed or assertion step.
pub type ScenarioFuture<'a> = Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send + 'a>>;

/// Boxed scenario step operating only through the Forge interface.
pub type ScenarioStep =
    Box<dyn for<'a> Fn(&'a dyn Forge, &'a RepositoryId) -> ScenarioFuture<'a> + Send + Sync>;

/// Backend- and topology-agnostic scenario definition.
pub struct Scenario {
    /// Human-readable scenario name.
    pub name: String,
    /// Outside-world input producer, such as a human filing an issue.
    pub seed: ScenarioStep,
    /// End-state assertion that reads only through Forge.
    pub assert: ScenarioStep,
}

impl Scenario {
    /// Creates a scenario from boxed seed and assertion steps.
    pub fn new(name: impl Into<String>, seed: ScenarioStep, assert: ScenarioStep) -> Self {
        Self {
            name: name.into(),
            seed,
            assert,
        }
    }
}

/// Error returned by [`run_scenario`] helpers.
#[derive(Debug)]
pub enum ScenarioError {
    /// Scenario seeding failed.
    Seed { scenario: String, source: BoxError },
    /// Stage execution failed.
    Run {
        scenario: String,
        source: StageError,
    },
    /// Scenario assertion failed.
    Assert { scenario: String, source: BoxError },
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenarioError::Seed { scenario, source } => {
                write!(formatter, "scenario {scenario} seed failed: {source}")
            }
            ScenarioError::Run { scenario, source } => {
                write!(formatter, "scenario {scenario} run failed: {source}")
            }
            ScenarioError::Assert { scenario, source } => {
                write!(formatter, "scenario {scenario} assertion failed: {source}")
            }
        }
    }
}

impl Error for ScenarioError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ScenarioError::Seed { source, .. } | ScenarioError::Assert { source, .. } => {
                Some(source.as_ref())
            }
            ScenarioError::Run { source, .. } => Some(source),
        }
    }
}

/// Seeds a scenario, runs the stage with the default budget, and asserts state.
pub async fn run_scenario<S: Stage + ?Sized>(
    stage: &S,
    scenario: &Scenario,
) -> Result<RunReport, ScenarioError> {
    run_scenario_with_budget(stage, scenario, DEFAULT_SCENARIO_BUDGET).await
}

/// Seeds a scenario, runs the stage with `budget`, and asserts state.
pub async fn run_scenario_with_budget<S: Stage + ?Sized>(
    stage: &S,
    scenario: &Scenario,
    budget: u64,
) -> Result<RunReport, ScenarioError> {
    (scenario.seed)(stage.forge(), stage.repo())
        .await
        .map_err(|source| ScenarioError::Seed {
            scenario: scenario.name.clone(),
            source,
        })?;
    let report = stage
        .run_to_quiescence(budget)
        .await
        .map_err(|source| ScenarioError::Run {
            scenario: scenario.name.clone(),
            source,
        })?;
    (scenario.assert)(stage.forge(), stage.repo())
        .await
        .map_err(|source| ScenarioError::Assert {
            scenario: scenario.name.clone(),
            source,
        })?;
    Ok(report)
}
