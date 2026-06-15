//! Backend-selection (`--backend`) and Forgejo-secret parsing tests, split out
//! of [`super`] to keep each test file within the line budget. Helpers and the
//! glob `use` come from the parent `tests` module.

use super::*;

#[test]
fn parses_forgejo_role_with_env_secrets() {
    let args = run_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000/",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[
            (FORGEJO_TOKEN_ENV, "tok-engineer"),
            (FORGEJO_USERNAME_ENV, "engineer"),
            (FORGEJO_PASSWORD_ENV, "pw-engineer"),
        ],
    );
    assert_eq!(
        args.backend,
        Backend::Forgejo(ForgejoArgs {
            base_url: "http://127.0.0.1:3000/".to_string(),
            token: "tok-engineer".to_string(),
            username: Some("engineer".to_string()),
            password: Some("pw-engineer".to_string()),
            ci_diagnostics: false,
        })
    );
    assert_eq!(args.backend.kind(), BackendKind::Forgejo);
}

#[test]
fn forgejo_ci_diagnostics_comes_from_env() {
    let args = run_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[
            (FORGEJO_TOKEN_ENV, "tok-engineer"),
            (FORGEJO_CI_DIAGNOSTICS_ENV, "1"),
        ],
    );
    let Backend::Forgejo(forgejo) = &args.backend else {
        panic!("expected forgejo backend");
    };
    assert!(forgejo.ci_diagnostics);
}

#[test]
fn forgejo_token_comes_from_env_not_argv() {
    let error = parse_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[],
    )
    .unwrap_err();
    assert!(error.to_string().contains(FORGEJO_TOKEN_ENV));
}

#[test]
fn forgejo_requires_base_url() {
    let error = parse_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[(FORGEJO_TOKEN_ENV, "tok")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("--base-url"));
}

#[test]
fn forgejo_rejects_ci_kind() {
    let error = parse_env(
        &[
            "--kind",
            "ci",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[(FORGEJO_TOKEN_ENV, "tok")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("--kind ci"));
    assert!(error.to_string().contains("forgejo"));
}

#[test]
fn forgejo_requires_wall_clock() {
    let error = parse_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
        ],
        &[(FORGEJO_TOKEN_ENV, "tok")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("--clock wall"));
}

#[test]
fn filesystem_ci_kind_still_accepted() {
    let args = run(&[
        "--kind",
        "ci",
        "--ci",
        "fail-then-pass",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    assert_eq!(args.backend, Backend::Filesystem);
    assert_eq!(
        args.kind,
        WorkerKind::Ci {
            policy: CiPolicyKind::FailThenPass
        }
    );
}

#[test]
fn rejects_bad_backend() {
    let error = parse(argv(&[
        "--kind",
        "provision",
        "--backend",
        "bogus",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("--backend"));
}

#[test]
fn base_url_rejected_for_filesystem() {
    let error = parse(argv(&[
        "--kind",
        "provision",
        "--base-url",
        "http://127.0.0.1:3000",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("--base-url"));
}

#[test]
fn forgejo_debug_redacts_secrets() {
    let args = ForgejoArgs {
        base_url: "http://127.0.0.1:3000".to_string(),
        token: "super-secret-token".to_string(),
        username: Some("engineer".to_string()),
        password: Some("super-secret-password".to_string()),
        ci_diagnostics: false,
    };
    let rendered = format!("{args:?}");
    assert!(!rendered.contains("super-secret-token"));
    assert!(!rendered.contains("super-secret-password"));
    assert!(rendered.contains("<redacted>"));
    assert!(rendered.contains("http://127.0.0.1:3000"));
}
