// SPDX-License-Identifier: MPL-2.0

use crate::{SERVE_STANDALONE_USAGE, SERVE_USAGE, parse_serve_invocation};

#[test]
fn serve_usage_documents_supported_components() {
    assert!(
        SERVE_USAGE.contains("standalone"),
        "serve help should advertise standalone mode"
    );
    assert!(
        SERVE_USAGE.contains("engine      Run the engine service"),
        "serve help should advertise engine as supported: {SERVE_USAGE}"
    );
    assert!(
        SERVE_USAGE.contains("worker      Run the worker service"),
        "serve help should advertise worker as supported: {SERVE_USAGE}"
    );
    assert!(
        !SERVE_USAGE.contains("trigger     "),
        "serve help must not advertise trigger as a runnable component: {SERVE_USAGE}"
    );
    assert!(
        SERVE_USAGE.contains("There is no separate `temper serve trigger` process"),
        "serve help should explicitly reject trigger as a separate runtime: {SERVE_USAGE}"
    );
    for expected in [
        "POST /forgejo/webhook",
        "`temper serve engine`",
        "`temper serve standalone`",
        "[engine] webhook_secret",
        "[engine] webhook_secret_file",
        "polling remains",
    ] {
        assert!(
            SERVE_USAGE.contains(expected),
            "serve help should mention {expected}: {SERVE_USAGE}"
        );
    }
    assert!(
        !SERVE_USAGE.contains("Not implemented yet"),
        "serve help should not describe trigger as future work: {SERVE_USAGE}"
    );
    assert!(
        SERVE_USAGE.contains("temper --config") && SERVE_USAGE.contains("--secrets"),
        "serve help should show deployment file flags before `serve`"
    );
    assert!(SERVE_STANDALONE_USAGE.contains("serve standalone"));
    assert!(SERVE_STANDALONE_USAGE.contains("--id <ID>"));
    assert!(!SERVE_STANDALONE_USAGE.contains("--secrets"));
    assert!(!SERVE_STANDALONE_USAGE.contains("--config"));
    assert!(
        SERVE_STANDALONE_USAGE.contains("temper daemon"),
        "standalone help should identify the compatibility wrapper"
    );
    for flag in ["--id", "--pool", "--capacity", "--engine-url"] {
        assert!(
            SERVE_USAGE.contains(flag),
            "serve help should mention {flag}: {SERVE_USAGE}"
        );
    }
}

#[test]
fn serve_trigger_is_rejected_as_unsupported_runtime_with_actionable_guidance() {
    let error = parse_serve_invocation(vec!["trigger".to_string()])
        .expect_err("trigger serve component should remain rejected");

    assert!(error.contains("temper serve trigger"), "{error}");
    assert!(
        error.contains("not a supported separate component"),
        "{error}"
    );
    assert!(error.contains("temper serve engine"), "{error}");
    assert!(error.contains("temper serve standalone"), "{error}");
    assert!(error.contains("POST /forgejo/webhook"), "{error}");
    assert!(error.contains("[engine] webhook_secret"), "{error}");
    assert!(error.contains("[engine] webhook_secret_file"), "{error}");
    assert!(error.contains("polling remains"), "{error}");
    for forbidden in ["not implemented yet", "later", "workitem"] {
        assert!(!error.contains(forbidden), "{error}");
    }
}
