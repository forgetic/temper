//! Executable acceptance for descendant-complete process ownership.
//!
//! The custom driver runs the compiled nested-session fixture through the real
//! process owners, worker machine/shell watchdog, active-job registry, and
//! signal-shutdown ordering. It never invokes Cargo recursively.

#[cfg(target_os = "linux")]
#[test]
fn compiled_fixture_crosses_every_production_completion_boundary() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_temper-containment-acceptance"))
        .arg(env!("CARGO_BIN_EXE_temper-descendant-fixture"))
        .output()
        .expect("run descendant-containment acceptance driver");
    assert!(
        output.status.success(),
        "acceptance driver failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_backend_cases(&stdout, "forced-supervisor");
    assert!(
        stdout.contains("BACKEND forced-supervisor PASS"),
        "{stdout}"
    );

    if stdout.contains("BACKEND auto-cgroup-v2 PASS") {
        assert_backend_cases(&stdout, "auto-cgroup-v2");
    } else {
        assert!(
            stdout.contains("CGROUP SKIP:"),
            "cgroup capability must produce a pass or explicit skip: {stdout}"
        );
    }
}

#[cfg(target_os = "linux")]
fn assert_backend_cases(stdout: &str, backend: &str) {
    for case in [
        "capacity-one-watchdog",
        "inspection-recovery",
        "split-signal-shutdown",
        "standalone-signal-shutdown",
        "worker-managed-command",
        "pre-push",
    ] {
        let evidence = format!("CASE {backend} {case} PASS");
        assert!(
            stdout.contains(&evidence),
            "missing `{evidence}`:\n{stdout}"
        );
    }
}
