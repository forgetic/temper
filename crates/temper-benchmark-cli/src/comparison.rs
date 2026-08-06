// SPDX-License-Identifier: MPL-2.0

//! Report-only comparison of run summaries and aggregate artifacts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::aggregate::metric_values;
use crate::{
    ADVISORY_METRICS, AggregateError, BenchmarkAggregateV1, BenchmarkConditionV1, BenchmarkModeV1,
    DistributionV1, PRIMARY_METRICS, RunSummaryV1, RunTerminalStatusV1,
};

mod input;
mod output;
mod render;

pub use input::load_comparison_input;
pub use output::write_comparison_artifacts;
pub use render::render_comparison_markdown;

pub const BENCHMARK_COMPARISON_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonInputKindV1 {
    RunSummary,
    Aggregate,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonSubjectV1 {
    pub kind: ComparisonInputKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<BenchmarkModeV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<BenchmarkConditionV1>,
    pub run_count: u64,
    pub success_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricComparisonV1 {
    pub metric: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<DistributionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<DistributionV1>,
    /// Head median minus base median. Missing observations have no delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median_delta: Option<i128>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "BenchmarkComparisonWireV1")]
pub struct BenchmarkComparisonV1 {
    pub version: u32,
    pub base: ComparisonSubjectV1,
    pub head: ComparisonSubjectV1,
    pub primary: Vec<MetricComparisonV1>,
    pub advisory: Vec<MetricComparisonV1>,
    /// Metrics unknown to this producer are preserved rather than promoted into
    /// either the structural or timing table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other: Vec<MetricComparisonV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkComparisonWireV1 {
    version: u32,
    base: ComparisonSubjectV1,
    head: ComparisonSubjectV1,
    primary: Vec<MetricComparisonV1>,
    advisory: Vec<MetricComparisonV1>,
    #[serde(default)]
    other: Vec<MetricComparisonV1>,
}

impl TryFrom<BenchmarkComparisonWireV1> for BenchmarkComparisonV1 {
    type Error = ComparisonError;

    fn try_from(value: BenchmarkComparisonWireV1) -> Result<Self, Self::Error> {
        if value.version != BENCHMARK_COMPARISON_VERSION {
            return Err(ComparisonError::UnsupportedVersion(value.version));
        }
        let comparison = Self {
            version: value.version,
            base: value.base,
            head: value.head,
            primary: value.primary,
            advisory: value.advisory,
            other: value.other,
        };
        comparison.validate()?;
        Ok(comparison)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ComparisonInput {
    Run(RunSummaryV1),
    Aggregate(BenchmarkAggregateV1),
}

#[derive(Debug, thiserror::Error)]
pub enum ComparisonError {
    #[error("cannot inspect comparison input `{path}`: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("comparison input directory `{0}` contains neither aggregate.json nor run.json")]
    MissingArtifact(PathBuf),
    #[error(
        "comparison input `{0}` must be a regular file or artifact directory, not a symlink or special file"
    )]
    UnsafeInput(PathBuf),
    #[error("cannot read comparison input `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("comparison input `{path}` is malformed JSON: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("comparison input `{0}` is not a run summary or aggregate artifact")]
    Unrecognized(PathBuf),
    #[error(
        "unsupported benchmark comparison version {0}; expected {BENCHMARK_COMPARISON_VERSION}"
    )]
    UnsupportedVersion(u32),
    #[error("incompatible comparison artifacts: {0}")]
    Incompatible(String),
    #[error("malformed benchmark comparison: {0}")]
    Malformed(String),
    #[error("cannot create comparison output directory `{path}`: {source}")]
    CreateOutput {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unsafe comparison output path `{0}`")]
    UnsafeOutput(PathBuf),
    #[error("cannot serialize comparison: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("cannot write comparison output `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl BenchmarkComparisonV1 {
    fn validate(&self) -> Result<(), ComparisonError> {
        validate_subject("base", &self.base)?;
        validate_subject("head", &self.head)?;
        validate_compatible(&self.base, &self.head)?;
        let mut names = BTreeSet::new();
        for metric in self.primary.iter().chain(&self.advisory).chain(&self.other) {
            if metric.metric.trim().is_empty() || !names.insert(&metric.metric) {
                return Err(ComparisonError::Malformed(
                    "metric names must be non-empty and unique".to_string(),
                ));
            }
            if let Some(statistics) = &metric.base {
                validate_statistics(statistics, &metric.metric, self.base.run_count)?;
            }
            if let Some(statistics) = &metric.head {
                validate_statistics(statistics, &metric.metric, self.head.run_count)?;
            }
            let expected_delta = metric
                .base
                .as_ref()
                .zip(metric.head.as_ref())
                .map(|(base, head)| i128::from(head.median) - i128::from(base.median));
            if metric.median_delta != expected_delta {
                return Err(ComparisonError::Malformed(format!(
                    "metric `{}` has an inconsistent median delta",
                    metric.metric
                )));
            }
        }
        Ok(())
    }
}

fn validate_subject(label: &str, subject: &ComparisonSubjectV1) -> Result<(), ComparisonError> {
    if subject.run_count == 0 {
        return Err(ComparisonError::Malformed(format!(
            "{label} subject requires at least one run"
        )));
    }
    if subject.success_count > subject.run_count {
        return Err(ComparisonError::Malformed(format!(
            "{label} success count exceeds run count"
        )));
    }
    match (&subject.benchmark, subject.mode) {
        (None, None) if subject.condition.is_none() => Ok(()),
        (Some(benchmark), Some(_)) if !benchmark.trim().is_empty() => Ok(()),
        _ => Err(ComparisonError::Malformed(format!(
            "{label} benchmark name and mode must be supplied together and are required by a condition"
        ))),
    }
}

fn validate_statistics(
    statistics: &DistributionV1,
    metric: &str,
    run_count: u64,
) -> Result<(), ComparisonError> {
    statistics
        .validate(metric)
        .map_err(|error| ComparisonError::Malformed(error.to_string()))?;
    if statistics.count > run_count {
        return Err(ComparisonError::Malformed(format!(
            "metric `{metric}` has more samples than subject runs"
        )));
    }
    Ok(())
}

/// Produces a report-only comparison. No delta threshold can turn a valid
/// comparison into an error.
pub fn compare_benchmarks(
    base: &ComparisonInput,
    head: &ComparisonInput,
) -> Result<BenchmarkComparisonV1, ComparisonError> {
    validate_input(base)?;
    validate_input(head)?;
    let base_subject = subject(base);
    let head_subject = subject(head);
    validate_compatible(&base_subject, &head_subject)?;
    let base_metrics = distributions(base);
    let head_metrics = distributions(head);

    let primary = compare_named(PRIMARY_METRICS, &base_metrics, &head_metrics);
    let advisory = compare_named(ADVISORY_METRICS, &base_metrics, &head_metrics);
    let known = PRIMARY_METRICS
        .iter()
        .chain(ADVISORY_METRICS)
        .copied()
        .collect::<BTreeSet<_>>();
    let other_names = base_metrics
        .keys()
        .chain(head_metrics.keys())
        .filter(|name| !known.contains(name.as_str()))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let other = other_names
        .into_iter()
        .map(|name| compare_metric(name, &base_metrics, &head_metrics))
        .collect();

    let comparison = BenchmarkComparisonV1 {
        version: BENCHMARK_COMPARISON_VERSION,
        base: base_subject,
        head: head_subject,
        primary,
        advisory,
        other,
    };
    comparison.validate()?;
    Ok(comparison)
}

fn validate_input(input: &ComparisonInput) -> Result<(), ComparisonError> {
    if let ComparisonInput::Aggregate(aggregate) = input {
        aggregate.validate()?;
    }
    Ok(())
}

fn subject(input: &ComparisonInput) -> ComparisonSubjectV1 {
    match input {
        ComparisonInput::Run(summary) => ComparisonSubjectV1 {
            kind: ComparisonInputKindV1::RunSummary,
            benchmark: summary
                .benchmark
                .as_ref()
                .map(|benchmark| benchmark.name.clone()),
            mode: summary.benchmark.as_ref().map(|benchmark| benchmark.mode),
            condition: summary
                .benchmark
                .as_ref()
                .and_then(|benchmark| benchmark.condition),
            run_count: 1,
            success_count: u64::from(
                summary.terminal.as_ref().map(|terminal| terminal.status)
                    == Some(RunTerminalStatusV1::Succeeded),
            ),
        },
        ComparisonInput::Aggregate(aggregate) => ComparisonSubjectV1 {
            kind: ComparisonInputKindV1::Aggregate,
            benchmark: aggregate.benchmark.clone(),
            mode: aggregate.mode,
            condition: aggregate.condition,
            run_count: aggregate.outcomes.total,
            success_count: aggregate.outcomes.succeeded,
        },
    }
}

fn validate_compatible(
    base: &ComparisonSubjectV1,
    head: &ComparisonSubjectV1,
) -> Result<(), ComparisonError> {
    if let (Some(base), Some(head)) = (&base.benchmark, &head.benchmark) {
        if base != head {
            return Err(ComparisonError::Incompatible(format!(
                "benchmark names differ (`{base}` versus `{head}`)"
            )));
        }
    }
    if let (Some(base), Some(head)) = (base.mode, head.mode) {
        if base != head {
            return Err(ComparisonError::Incompatible(format!(
                "benchmark modes differ (`{base:?}` versus `{head:?}`)"
            )));
        }
    }
    Ok(())
}

fn distributions(input: &ComparisonInput) -> BTreeMap<String, DistributionV1> {
    match input {
        ComparisonInput::Run(summary) => metric_values(summary)
            .into_iter()
            .filter_map(|(name, value)| {
                DistributionV1::from_values(vec![value]).map(|stats| (name.to_string(), stats))
            })
            .collect(),
        ComparisonInput::Aggregate(aggregate) => aggregate.metrics.clone(),
    }
}

fn compare_named(
    names: &[&str],
    base: &BTreeMap<String, DistributionV1>,
    head: &BTreeMap<String, DistributionV1>,
) -> Vec<MetricComparisonV1> {
    names
        .iter()
        .map(|name| compare_metric(name, base, head))
        .collect()
}

fn compare_metric(
    name: &str,
    base: &BTreeMap<String, DistributionV1>,
    head: &BTreeMap<String, DistributionV1>,
) -> MetricComparisonV1 {
    let base = base.get(name).cloned();
    let head = head.get(name).cloned();
    let median_delta = base
        .as_ref()
        .zip(head.as_ref())
        .map(|(base, head)| i128::from(head.median) - i128::from(base.median));
    MetricComparisonV1 {
        metric: name.to_string(),
        base,
        head,
        median_delta,
    }
}

impl From<AggregateError> for ComparisonError {
    fn from(error: AggregateError) -> Self {
        Self::Malformed(error.to_string())
    }
}
