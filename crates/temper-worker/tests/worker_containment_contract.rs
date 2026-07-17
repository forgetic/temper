#[test]
#[cfg(target_os = "linux")]
fn worker_descendant_containment_contract() {
    let status =
        std::process::Command::new(env!("CARGO_BIN_EXE_temper-worker-containment-fixture"))
            .status()
            .expect("run worker containment fixture");
    assert!(
        status.success(),
        "worker containment fixture failed: {status}"
    );
}
