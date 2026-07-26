//! Linux public-process capstone for bounded standalone shutdown and restart.
//!
//! Run with:
//! `cargo test --test standalone_shutdown_forgejo_e2e standalone_sigterm_hands_off_and_recovers_once -- --ignored --exact --nocapture`

#![cfg(target_os = "linux")]

#[path = "support/e2e_lock.rs"]
mod e2e_lock;

use std::path::PathBuf;
use temper_testing::live_manifest::{
    ScenarioBundle, StandaloneShutdownRequest, TemperCommand, run_standalone_shutdown_acceptance,
};

#[test]
#[ignore = "boots real Forgejo and public standalone processes; run with --ignored"]
fn standalone_sigterm_hands_off_and_recovers_once() {
    let _e2e_lock = e2e_lock::acquire();
    let scenario = ScenarioBundle::load(repo_root().join("scenarios/basic-delivery"))
        .expect("basic-delivery scenario bundle loads");
    let evidence = run_standalone_shutdown_acceptance(StandaloneShutdownRequest {
        scenario,
        temper: TemperCommand::new(env!("CARGO_BIN_EXE_temper")),
        descendant_fixture: PathBuf::from(env!(
            "CARGO_BIN_EXE_temper-standalone-shutdown-e2e-fixture"
        )),
    })
    .unwrap_or_else(|error| panic!("{error}"));
    eprintln!("{}", evidence.to_report());
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
