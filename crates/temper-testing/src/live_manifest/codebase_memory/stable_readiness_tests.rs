use std::path::Path;

#[test]
fn stable_provider_confirms_only_its_indexed_current_root() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = std::process::Command::new("python3")
        .arg(crate_root.join("tests/support/stable_readiness_protocol.py"))
        .arg(crate_root.join("src/live_manifest/fake_codebase_memory_mcp.py"))
        .output()
        .expect("run the legacy provider readiness protocol");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
