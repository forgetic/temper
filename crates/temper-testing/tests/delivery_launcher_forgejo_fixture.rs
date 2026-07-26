//! Static contracts tying both delivery launchers to the Bench-owned fixture pins.

const SHARED_FIXTURE: &str = include_str!("../../../examples/forgejo-fixture.sh");
const BASIC_LAUNCHER: &str = include_str!("../../../examples/basic-delivery/run.sh");
const REFERENCE_LAUNCHER: &str = include_str!("../../../examples/reference-delivery/run.sh");

#[test]
fn bench_fixture_pins_are_the_verified_releases() {
    assert_eq!(bench_forgejo::download::FORGEJO_VERSION, "16.0.1");
    assert_eq!(bench_forgejo::download::FORGEJO_RUNNER_VERSION, "12.12.0");
}

#[test]
fn delivery_launchers_share_bench_resolution_and_version_verification() {
    assert!(SHARED_FIXTURE.contains("cargo build -p temper-testing --bin temper-forgejo-fixture"));
    assert!(!SHARED_FIXTURE.contains("16.0.1"));
    assert!(!SHARED_FIXTURE.contains("12.12.0"));
    assert!(SHARED_FIXTURE.contains("/api/v1/version"));
    assert!(SHARED_FIXTURE.contains("Forgejo fixture version mismatch"));

    for launcher in [BASIC_LAUNCHER, REFERENCE_LAUNCHER] {
        assert!(launcher.contains(". \"$SCRIPT_DIR/../forgejo-fixture.sh\""));
        assert!(launcher.contains("resolve_forgejo_fixture \"$WORKSPACE_ROOT\""));
        assert!(launcher.contains("verify_forgejo_fixture_version"));
        assert!(!launcher.contains("\nFORGEJO_VERSION="));
        assert!(!launcher.contains("\nFORGEJO_RUNNER_VERSION="));
    }
}
