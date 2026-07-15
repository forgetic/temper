// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use temper_forge::{CreateRepository, Forge};
use temper_forge_memory::MemoryForge;
use temper_workflow::RoleId;

use super::support::{assert_success, temper, workspace_root};

#[test]
fn signed_webhook_proves_engine_intake_and_selected_operator_contract() {
    let root = workspace_root();
    let scenario_root = root.join("scenarios/target-ux-e2e");
    let manifest_source = std::fs::read_to_string(scenario_root.join("scenario.toml"))
        .expect("read target UX manifest");
    let manifest: toml::Value = toml::from_str(&manifest_source).expect("parse target UX manifest");
    let trigger = manifest
        .get("target_ux")
        .and_then(toml::Value::as_table)
        .and_then(|target_ux| target_ux.get("trigger"))
        .and_then(toml::Value::as_table)
        .expect("target_ux.trigger table");
    let selected_surfaces = trigger
        .get("selected_surfaces")
        .and_then(toml::Value::as_array)
        .expect("selected_surfaces array")
        .iter()
        .map(|surface| surface.as_str().expect("selected surface string"))
        .collect::<Vec<_>>();
    assert_eq!(
        selected_surfaces,
        vec!["temper serve engine", "temper serve standalone"]
    );
    assert_eq!(
        trigger.get("endpoint").and_then(toml::Value::as_str),
        Some("POST /forgejo/webhook")
    );
    assert_eq!(
        trigger
            .get("legacy_internal_adapter_command")
            .and_then(toml::Value::as_str),
        Some("temper trigger-forgejo")
    );
    assert_eq!(
        trigger
            .get("rejected_command")
            .and_then(toml::Value::as_str),
        Some("temper serve trigger")
    );
    assert!(trigger.get("selected_command").is_none());

    let readme =
        std::fs::read_to_string(scenario_root.join("README.md")).expect("target UX README");
    for expected in [
        "temper serve engine",
        "temper serve standalone",
        "POST /forgejo/webhook",
        "[engine] webhook_secret",
        "[engine] webhook_secret_file",
        "periodic polling remains the correctness backstop",
        "`temper trigger-forgejo`",
        "legacy/internal adapter command",
        "adapter compatibility test coverage",
    ] {
        assert!(
            readme.contains(expected),
            "README should mention {expected}"
        );
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let serve_help = temper(&["serve", "--help"], dir.path());
    assert_success(&serve_help);
    let serve_help = String::from_utf8(serve_help.stdout).expect("serve help stdout utf8");
    for expected in [
        "temper serve engine",
        "temper serve standalone",
        "POST /forgejo/webhook",
        "[engine] webhook_secret",
        "[engine] webhook_secret_file",
        "polling remains",
        "correctness backstop",
    ] {
        assert!(
            serve_help.contains(expected),
            "serve help lacks {expected}: {serve_help}"
        );
    }
    assert!(!serve_help.contains("trigger-forgejo"), "{serve_help}");

    let rejected = temper(&["serve", "trigger"], dir.path());
    assert!(!rejected.status.success());
    let stderr = String::from_utf8(rejected.stderr).expect("stderr utf8");
    for expected in [
        "`temper serve trigger` is not a supported separate component",
        "temper serve engine",
        "temper serve standalone",
        "POST /forgejo/webhook",
        "polling remains",
        "correctness backstop",
    ] {
        assert!(
            stderr.contains(expected),
            "rejection lacks {expected}: {stderr}"
        );
    }

    let payload = std::fs::read(scenario_root.join("config/trigger/forgejo-issue-webhook.json"))
        .expect("read checked-in Forgejo payload");
    let secret = String::from_utf8(
        std::fs::read(scenario_root.join("config/trigger/webhook-secret"))
            .expect("read checked-in webhook secret"),
    )
    .expect("webhook secret utf8")
    .trim()
    .to_string();

    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repository = forge
            .create_repository(CreateRepository {
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("create repository")
            .id;
        let workflow = Arc::new(temper_reference_delivery::workflow());
        let compiled = Arc::new(workflow.compile());
        let daemon = temper_engine::Daemon::new(Arc::new(handle.clone())).with_webhook(
            forge,
            workflow,
            compiled,
            Arc::new(temper_engine::WebhookConfig {
                secret: secret.clone(),
                targets: vec![temper_engine::RoleFeedTarget {
                    repo: repository,
                    path: temper_forge::RepositoryPath::new("ai", "temper"),
                    role: RoleId::new("engineer"),
                    mode: temper_engine::RoleFeedMode::Wake,
                }],
            }),
            temper_engine::system_clock(),
        );
        let server = temper_engine::serve(
            &handle,
            &daemon,
            "127.0.0.1:0".parse().expect("loopback address"),
        )
        .await
        .expect("bind in-process engine route");
        let signature = format!(
            "sha256={}",
            temper_engine::webhook_signature(&secret, &payload)
        );
        let client = temper_engine_io::http::build_http_client();
        let response = temper_engine_io::http::http_call(
            &client,
            temper_engine_io::http::HttpCall {
                method: "POST".to_string(),
                url: format!("http://{}/forgejo/webhook", server.local_addr()),
                headers: vec![
                    ("x-forgejo-event".to_string(), "issues".to_string()),
                    ("x-forgejo-signature".to_string(), signature),
                ],
                body: payload,
            },
        )
        .await
        .expect("post checked-in webhook payload");
        assert_eq!(response.status, 202, "response: {response:?}");
        server.begin_drain(std::time::Duration::from_secs(1));
    });
}
