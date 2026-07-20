// SPDX-License-Identifier: MPL-2.0

//! Privacy-limited metadata collected for direct benchmark runs.
//!
//! The collector has an explicit allowlist. In particular, it never reads or
//! records the hostname, username, home directory, arbitrary environment
//! variables, network addresses, or credentials.

use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::fs;
use std::process::Command;

use temper_protocol_activity::{AgentActivityEventV1, AgentRunEventV1};

use crate::{
    BenchmarkAnnotationsV1, HostMetadataV1, ObservedModelIdentityV1, TemperBuildMetadataV1,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
const MAX_METADATA_VALUE_BYTES: usize = 256;

/// Collects the allowlisted host/build metadata for a direct benchmark run.
/// Region and cache state are copied only from explicit manifest annotations.
pub fn collect_environment_metadata(
    events: &[AgentRunEventV1],
    annotations: &BenchmarkAnnotationsV1,
) -> HostMetadataV1 {
    HostMetadataV1 {
        temper: temper_build_metadata(),
        observed_models: observed_model_identities(events),
        os: std::env::consts::OS.to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        logical_cpu_count: std::thread::available_parallelism()
            .ok()
            .and_then(|count| u64::try_from(count.get()).ok()),
        cpu_model: best_effort_cpu_model(),
        load_average: best_effort_load_average(),
        provider_region: annotations.provider_region.clone(),
        cache_warmth: annotations.cache_warmth.clone(),
    }
}

/// Returns build identity without consulting package-manager configuration or
/// arbitrary process environment.
pub fn temper_build_metadata() -> TemperBuildMetadataV1 {
    TemperBuildMetadataV1 {
        package_version: env!("CARGO_PKG_VERSION").to_string(),
        commit: best_effort_temper_commit(),
    }
}

/// Returns the unique provider/model pairs actually observed in model-call
/// events, in deterministic lexical order.
pub fn observed_model_identities(events: &[AgentRunEventV1]) -> Vec<ObservedModelIdentityV1> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::ModelCallStarted(started) => Some(ObservedModelIdentityV1 {
                provider: started.provider.clone(),
                model: started.model.clone(),
            }),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Resolves the Temper source commit when the binary still has access to its
/// build checkout. Failure is expected for packaged binaries and is represented
/// as unavailable.
pub fn best_effort_temper_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?;
    let commit = commit.trim();
    if (7..=64).contains(&commit.len()) && commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(commit.to_ascii_lowercase())
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn best_effort_cpu_model() -> Option<String> {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").ok()?;
    cpuinfo.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        matches!(name.trim(), "model name" | "Hardware" | "Processor")
            .then(|| bounded(value.trim()))
            .flatten()
    })
}

#[cfg(target_os = "macos")]
fn best_effort_cpu_model() -> Option<String> {
    let output = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
        .and_then(|value| bounded(value.trim()))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn best_effort_cpu_model() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn best_effort_load_average() -> Option<String> {
    let load = fs::read_to_string("/proc/loadavg").ok()?;
    let values = load.split_whitespace().take(3).collect::<Vec<_>>();
    (values.len() == 3 && values.iter().all(|value| value.parse::<f64>().is_ok()))
        .then(|| values.join(" "))
}

#[cfg(target_os = "macos")]
fn best_effort_load_average() -> Option<String> {
    let output = Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let load = String::from_utf8(output.stdout).ok()?;
    let values = load
        .trim()
        .trim_matches(['{', '}'])
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>();
    (values.len() == 3
        && values
            .iter()
            .all(|value| value.parse::<f64>().is_ok_and(f64::is_finite)))
    .then(|| values.join(" "))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn best_effort_load_average() -> Option<String> {
    None
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn bounded(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut end = value.len().min(MAX_METADATA_VALUE_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Some(value[..end].to_string())
}
