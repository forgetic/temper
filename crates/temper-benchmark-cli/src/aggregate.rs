// SPDX-License-Identifier: MPL-2.0

//! Deterministic aggregation of benchmark repetitions.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{BenchmarkModeV1, RunSummaryV1, RunTerminalStatusV1};

mod metrics;
mod render;

pub(crate) use metrics::metric_values;
pub use metrics::{ADVISORY_METRICS, PRIMARY_METRICS};
pub use render::render_aggregate_markdown;
pub(crate) use render::{markdown_text, metric_label};

pub const BENCHMARK_AGGREGATE_VERSION: u32 = 1;

/// A nearest-rank five-number summary. `count` records how many repetitions
/// exposed the metric; unavailable observations are not converted to zero.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionV1 {
    pub count: u64,
    pub min: u64,
    pub p25: u64,
    pub median: u64,
    pub p75: u64,
    pub max: u64,
}

impl DistributionV1 {
    pub fn from_values(mut values: Vec<u64>) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        values.sort_unstable();
        Some(Self {
            count: values.len() as u64,
            min: values[0],
            p25: nearest_rank(&values, 25),
            median: nearest_rank(&values, 50),
            p75: nearest_rank(&values, 75),
            max: values[values.len() - 1],
        })
    }

    pub(crate) fn validate(&self, metric: &str) -> Result<(), AggregateError> {
        if self.count == 0 {
            return Err(AggregateError::Malformed(format!(
                "metric `{metric}` has a zero sample count"
            )));
        }
        if !(self.min <= self.p25
            && self.p25 <= self.median
            && self.median <= self.p75
            && self.p75 <= self.max)
        {
            return Err(AggregateError::Malformed(format!(
                "metric `{metric}` has unordered statistics"
            )));
        }
        Ok(())
    }
}

fn nearest_rank(values: &[u64], percentile: usize) -> u64 {
    let rank = (percentile * values.len()).div_ceil(100).max(1);
    values[rank - 1]
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunOutcomeCountsV1 {
    pub total: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub incomplete: u64,
}

/// An embedded reference to one underlying run summary. Embedding the summary
/// keeps aggregate artifacts independently auditable after repetition
/// directories are moved or deleted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateRunV1 {
    pub repetition: u32,
    pub summary: RunSummaryV1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "BenchmarkAggregateWireV1")]
pub struct BenchmarkAggregateV1 {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<BenchmarkModeV1>,
    pub outcomes: RunOutcomeCountsV1,
    /// Extensible metric names allow a newer producer's measurements to remain
    /// visible to comparison code without weakening the outer serde contract.
    pub metrics: BTreeMap<String, DistributionV1>,
    pub runs: Vec<AggregateRunV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkAggregateWireV1 {
    version: u32,
    #[serde(default)]
    benchmark: Option<String>,
    #[serde(default)]
    mode: Option<BenchmarkModeV1>,
    outcomes: RunOutcomeCountsV1,
    metrics: BTreeMap<String, DistributionV1>,
    runs: Vec<AggregateRunV1>,
}

impl TryFrom<BenchmarkAggregateWireV1> for BenchmarkAggregateV1 {
    type Error = AggregateError;

    fn try_from(value: BenchmarkAggregateWireV1) -> Result<Self, Self::Error> {
        if value.version != BENCHMARK_AGGREGATE_VERSION {
            return Err(AggregateError::UnsupportedVersion(value.version));
        }
        let aggregate = Self {
            version: value.version,
            benchmark: value.benchmark,
            mode: value.mode,
            outcomes: value.outcomes,
            metrics: value.metrics,
            runs: value.runs,
        };
        aggregate.validate()?;
        Ok(aggregate)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AggregateError {
    #[error("cannot aggregate an empty run set")]
    Empty,
    #[error("unsupported benchmark aggregate version {0}; expected {BENCHMARK_AGGREGATE_VERSION}")]
    UnsupportedVersion(u32),
    #[error("incompatible benchmark repetitions: {0}")]
    Incompatible(String),
    #[error("malformed benchmark aggregate: {0}")]
    Malformed(String),
}

impl BenchmarkAggregateV1 {
    pub(crate) fn validate(&self) -> Result<(), AggregateError> {
        if self.runs.is_empty() {
            return Err(AggregateError::Malformed(
                "at least one run reference is required".to_string(),
            ));
        }
        let expected_total = self.runs.len() as u64;
        if self.outcomes != outcome_counts(&self.runs) {
            return Err(AggregateError::Malformed(
                "outcome counts do not match embedded runs".to_string(),
            ));
        }
        let mut repetitions = BTreeSet::new();
        for run in &self.runs {
            if run.repetition == 0 || !repetitions.insert(run.repetition) {
                return Err(AggregateError::Malformed(format!(
                    "invalid or duplicate repetition {}",
                    run.repetition
                )));
            }
        }
        for (name, statistics) in &self.metrics {
            if name.trim().is_empty() {
                return Err(AggregateError::Malformed(
                    "metric name must not be empty".to_string(),
                ));
            }
            statistics.validate(name)?;
            if statistics.count > expected_total {
                return Err(AggregateError::Malformed(format!(
                    "metric `{name}` has more samples than runs"
                )));
            }
        }
        let expected_metrics = aggregated_metrics(&self.runs);
        for name in PRIMARY_METRICS.iter().chain(ADVISORY_METRICS) {
            if self.metrics.get(*name) != expected_metrics.get(*name) {
                return Err(AggregateError::Malformed(format!(
                    "metric `{name}` does not match embedded runs"
                )));
            }
        }
        validate_declared_identity(self.benchmark.as_deref(), self.mode, &self.runs)
    }
}

/// Aggregates repetitions in caller order, using explicit repetition numbers
/// when the run summaries have benchmark identity and one-based order otherwise.
pub fn aggregate_run_summaries(
    summaries: impl IntoIterator<Item = RunSummaryV1>,
) -> Result<BenchmarkAggregateV1, AggregateError> {
    let summaries = summaries.into_iter().collect::<Vec<_>>();
    if summaries.is_empty() {
        return Err(AggregateError::Empty);
    }

    let (benchmark, mode) = common_identity(&summaries)?;
    let runs = summaries
        .into_iter()
        .enumerate()
        .map(|(index, summary)| AggregateRunV1 {
            repetition: summary
                .benchmark
                .as_ref()
                .map_or(index as u32 + 1, |identity| identity.repetition),
            summary,
        })
        .collect::<Vec<_>>();
    let outcomes = outcome_counts(&runs);
    let metrics = aggregated_metrics(&runs);
    let aggregate = BenchmarkAggregateV1 {
        version: BENCHMARK_AGGREGATE_VERSION,
        benchmark,
        mode,
        outcomes,
        metrics,
        runs,
    };
    aggregate.validate()?;
    Ok(aggregate)
}

fn aggregated_metrics(runs: &[AggregateRunV1]) -> BTreeMap<String, DistributionV1> {
    let mut values = BTreeMap::<String, Vec<u64>>::new();
    for run in runs {
        for (name, value) in metric_values(&run.summary) {
            values.entry(name.to_string()).or_default().push(value);
        }
    }
    values
        .into_iter()
        .filter_map(|(name, values)| DistributionV1::from_values(values).map(|stats| (name, stats)))
        .collect()
}

fn common_identity(
    summaries: &[RunSummaryV1],
) -> Result<(Option<String>, Option<BenchmarkModeV1>), AggregateError> {
    let identities = summaries
        .iter()
        .map(|summary| summary.benchmark.as_ref())
        .collect::<Vec<_>>();
    if identities.iter().all(|identity| identity.is_none()) {
        return Ok((None, None));
    }
    if identities.iter().any(|identity| identity.is_none()) {
        return Err(AggregateError::Incompatible(
            "benchmark identity is present on only some runs".to_string(),
        ));
    }
    let first = identities[0].expect("checked as present");
    if first.name.trim().is_empty() || first.repetition == 0 {
        return Err(AggregateError::Incompatible(
            "benchmark name and repetition must be non-empty and non-zero".to_string(),
        ));
    }
    for identity in identities.into_iter().flatten().skip(1) {
        if identity.name != first.name || identity.mode != first.mode {
            return Err(AggregateError::Incompatible(
                "benchmark name or mode changed between repetitions".to_string(),
            ));
        }
    }
    Ok((Some(first.name.clone()), Some(first.mode)))
}

fn validate_declared_identity(
    benchmark: Option<&str>,
    mode: Option<BenchmarkModeV1>,
    runs: &[AggregateRunV1],
) -> Result<(), AggregateError> {
    match (benchmark, mode) {
        (None, None) => {
            if runs.iter().any(|run| run.summary.benchmark.is_some()) {
                return Err(AggregateError::Malformed(
                    "aggregate omits identity supplied by embedded runs".to_string(),
                ));
            }
        }
        (Some(name), Some(mode)) if !name.trim().is_empty() => {
            for run in runs {
                let identity = run.summary.benchmark.as_ref().ok_or_else(|| {
                    AggregateError::Malformed(
                        "embedded run is missing aggregate benchmark identity".to_string(),
                    )
                })?;
                if identity.name != name
                    || identity.mode != mode
                    || identity.repetition != run.repetition
                {
                    return Err(AggregateError::Malformed(
                        "embedded run identity does not match aggregate".to_string(),
                    ));
                }
            }
        }
        _ => {
            return Err(AggregateError::Malformed(
                "benchmark name and mode must be supplied together".to_string(),
            ));
        }
    }
    Ok(())
}

fn outcome_counts(runs: &[AggregateRunV1]) -> RunOutcomeCountsV1 {
    let mut counts = RunOutcomeCountsV1 {
        total: runs.len() as u64,
        succeeded: 0,
        failed: 0,
        cancelled: 0,
        incomplete: 0,
    };
    for run in runs {
        match run
            .summary
            .terminal
            .as_ref()
            .map(|terminal| terminal.status)
        {
            Some(RunTerminalStatusV1::Succeeded) => counts.succeeded += 1,
            Some(RunTerminalStatusV1::Failed) => counts.failed += 1,
            Some(RunTerminalStatusV1::Cancelled) => counts.cancelled += 1,
            None => counts.incomplete += 1,
        }
    }
    counts
}
