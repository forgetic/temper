//! Basic-delivery live e2e: thin root-package wrapper over the reusable
//! `temper_testing::live_manifest` harness.
//!
//! Run with:
//!   cargo test --test basic_delivery_forgejo_e2e -- --ignored --nocapture

#![cfg(unix)]

#[path = "support/e2e_lock.rs"]
mod e2e_lock;

use std::path::PathBuf;

use temper_testing::live_manifest::{ScenarioBundle, TemperCommand, run_live_manifest};

#[test]
#[ignore = "boots real Forgejo + forgejo-runner and spawns `temper` binaries; run with --ignored"]
fn basic_delivery_run_sh_equivalent_converges() {
    let _e2e_lock = e2e_lock::acquire();
    let scenario = ScenarioBundle::load(repo_root().join("scenarios/basic-delivery"))
        .expect("basic-delivery scenario bundle loads");
    let temper = TemperCommand::new(env!("CARGO_BIN_EXE_temper"));
    let evidence = run_live_manifest(scenario, temper).unwrap_or_else(|error| {
        panic!("{error}");
    });
    eprintln!("{}", evidence.to_report());
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
