// SPDX-License-Identifier: MPL-2.0

//! Opt-in scenario adapter: seed the real benchmark, then verify its merged report.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

use super::process::{run_git, run_git_with_token, run_logged};
use super::{FinalStateEvidence, RepoFixture};

const MARKER: &str = ".temper-artifact-harness";
const MARKER_VALUE: &str = "temper-graph-artifact-fixture-v1\n";
const HARNESS: &str = "benchmarks/codebase-memory-artifacts";
const IDENTITY: &str = ".artifact-harness-sha256.json";
const FACT_PREFIX: &str = "artifact-benchmark-fact ";

macro_rules! harness_sources {
    ($($name:literal),+ $(,)?) => {
        &[$(($name, include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"), "/../../benchmarks/codebase-memory-artifacts/", $name,
        )).as_slice())),+]
    };
}

const SOURCES: &[(&str, &[u8])] = harness_sources!(
    "benchmark.py",
    "contract.py",
    "source.py",
    "artifact.py",
    "protocol.py",
    "accounting.py",
    "native.py",
    "reporting.py",
    "fixture.py",
);

fn enabled(root: &Path) -> Result<bool, String> {
    let marker = root.join(MARKER);
    if !marker.exists() {
        return Ok(false);
    }
    if fs::read_to_string(marker).map_err(|error| error.to_string())? != MARKER_VALUE {
        return Err("unrecognized artifact harness fixture marker".to_string());
    }
    Ok(true)
}

fn identity_bytes() -> Result<Vec<u8>, String> {
    let checksums = SOURCES
        .iter()
        .map(|(name, bytes)| (*name, format!("{:x}", Sha256::digest(bytes))))
        .collect::<BTreeMap<_, _>>();
    serde_json::to_vec(&checksums).map_err(|error| error.to_string())
}

pub(super) fn install(checkout: &Path) -> Result<(), String> {
    if !enabled(checkout)? {
        return Ok(());
    }
    let target = checkout.join(HARNESS);
    if target.exists() || checkout.join(IDENTITY).exists() {
        return Err("artifact scenario must use the repository-embedded harness".to_string());
    }
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    for (name, bytes) in SOURCES {
        fs::write(target.join(name), bytes).map_err(|error| error.to_string())?;
    }
    fs::write(checkout.join(IDENTITY), identity_bytes()?).map_err(|error| error.to_string())?;
    verify_harness(checkout)
}

fn verify_harness(checkout: &Path) -> Result<(), String> {
    if fs::read(checkout.join(IDENTITY)).map_err(|error| error.to_string())? != identity_bytes()? {
        return Err("merged artifact harness identity was changed".to_string());
    }
    let directory = checkout.join(HARNESS);
    let mut actual = fs::read_dir(&directory)
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    actual.retain(|name| name != "__pycache__");
    actual.sort();
    let mut expected = SOURCES
        .iter()
        .map(|(name, _)| std::ffi::OsString::from(*name))
        .collect::<Vec<_>>();
    expected.sort();
    if actual != expected {
        return Err("merged artifact harness module set was changed".to_string());
    }
    for (name, bytes) in SOURCES {
        if fs::read(directory.join(name)).map_err(|error| error.to_string())? != *bytes {
            return Err(
                "merged artifact harness bytes differ from the scenario binary".to_string(),
            );
        }
    }
    Ok(())
}

pub(super) fn verify_merged(
    repo: &RepoFixture,
    workspace: &Path,
    token: &str,
    final_state: &FinalStateEvidence,
    log: &Path,
) -> Result<(), String> {
    if !enabled(&repo.seed_path)? {
        return Ok(());
    }
    let checkout = workspace.join("repo-seed").join(&repo.name);
    let merged = final_state
        .pull_request
        .merged_sha
        .as_deref()
        .ok_or_else(|| "artifact scenario requires an actual merge SHA".to_string())?;
    checkout_merged(&checkout, &repo.default_branch, merged, token, log)?;
    verify_harness(&checkout)?;
    verify_fixture_sources(&checkout, &repo.seed_path)?;
    verify_report_and_product(&checkout, log)
}

fn checkout_merged(
    checkout: &Path,
    branch: &str,
    merged: &str,
    token: &str,
    log: &Path,
) -> Result<(), String> {
    run_git_with_token(
        checkout,
        token,
        &["fetch", "--quiet", "origin", branch],
        log,
        "fetch merged artifact scenario default branch",
    )?;
    let head = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["rev-parse", "FETCH_HEAD"])
        .output()
        .map_err(|error| error.to_string())?;
    if !head.status.success() || String::from_utf8_lossy(&head.stdout).trim() != merged {
        return Err(
            "artifact report verification requires the actual merged default branch".to_string(),
        );
    }
    run_git(
        checkout,
        &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
        log,
        "inspect merged artifact scenario checkout",
    )
}

fn verify_fixture_sources(checkout: &Path, seed: &Path) -> Result<(), String> {
    for path in [
        MARKER,
        "verify_report.py",
        "tests/test_report_summary.py",
        ".forgejo/workflows/ci.yml",
    ] {
        if fs::read(checkout.join(path)).map_err(|error| error.to_string())?
            != fs::read(seed.join(path)).map_err(|error| error.to_string())?
        {
            return Err(
                "artifact scenario validation fixtures were changed by the engineer".to_string(),
            );
        }
    }
    Ok(())
}

fn verify_report_and_product(checkout: &Path, log: &Path) -> Result<(), String> {
    python(
        checkout,
        &[
            "benchmarks/codebase-memory-artifacts/benchmark.py",
            "fixture",
            "--output",
            "ci-artifact-report.json",
        ],
        log,
        "repeat the exact merged fixture harness",
    )?;
    python(
        checkout,
        &[
            "verify_report.py",
            "graph-artifact-report.json",
            "ci-artifact-report.json",
        ],
        log,
        "verify delivered report equals repeated exact harness output",
    )?;
    python(
        checkout,
        &["-m", "unittest", "discover", "-s", "tests", "-v"],
        log,
        "verify report consumer product behavior at merged main",
    )?;
    let mut output = fs::OpenOptions::new()
        .append(true)
        .open(log)
        .map_err(|error| error.to_string())?;
    for checkpoint in [
        "delivered-report-matches-repeated-harness",
        "report-consumer-product-tests-passed",
        "report-verified-at-actual-merged-main",
    ] {
        writeln!(
            output,
            "{FACT_PREFIX}{}",
            serde_json::json!({"checkpoint": checkpoint, "passed": true})
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn python(checkout: &Path, args: &[&str], log: &Path, label: &str) -> Result<(), String> {
    run_logged(
        Command::new("timeout")
            .args(["20s", "python3", "-B"])
            .args(args)
            .current_dir(checkout)
            .env("PYTHONPATH", "")
            .env("PYTHONSTARTUP", ""),
        log,
        label,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_harness_requires_opt_in_and_detects_changed_source_or_identity() {
        let root = tempfile::tempdir().unwrap();
        install(root.path()).unwrap();
        assert!(!root.path().join(HARNESS).exists());
        fs::write(root.path().join(MARKER), MARKER_VALUE).unwrap();
        install(root.path()).unwrap();
        assert!(install(root.path()).is_err());
        let source = root.path().join(HARNESS).join("contract.py");
        fs::write(&source, "changed validation").unwrap();
        assert!(verify_harness(root.path()).is_err());
        fs::write(
            source,
            SOURCES
                .iter()
                .find(|(name, _)| *name == "contract.py")
                .unwrap()
                .1,
        )
        .unwrap();
        fs::write(root.path().join(IDENTITY), "{}").unwrap();
        assert!(verify_harness(root.path()).is_err());
    }

    #[test]
    fn artifact_harness_refuses_preseeded_shadow_modules() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(MARKER), MARKER_VALUE).unwrap();
        fs::create_dir_all(root.path().join(HARNESS)).unwrap();
        assert!(install(root.path()).is_err());
    }
}
