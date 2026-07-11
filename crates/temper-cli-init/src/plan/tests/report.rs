use std::collections::BTreeMap;

use temper_cli_common::LoadOptions;
use temper_forge::WebhookStatus;

use crate::deployment::load_deployment;
use crate::plan::PlanOptions;
use crate::plan::inspection::{ForgeInspection, MetadataInspection, desired_users};
use crate::plan::report::build_report;

use super::support::{RecordingInspector, repository, write_bundle};

#[test]
fn one_repository_keeps_singular_json_projections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bundle(dir.path(), &["acme/service"]);
    let opts = PlanOptions {
        options: LoadOptions {
            config: Some(path.join("config.toml")),
            credentials: Some(path.join("credentials.toml")),
        },
        ..Default::default()
    };
    let bundle = load_deployment(&opts.options, &opts.env, &opts.paths, false).expect("bundle");
    let mut inspector = RecordingInspector {
        inspections: BTreeMap::from([(
            "acme/service".to_string(),
            Ok(ForgeInspection {
                inspected: true,
                repository: Some(repository("acme", "service")),
                labels: vec!["queued".to_string()],
                webhooks: vec![WebhookStatus {
                    url: bundle.webhook.as_ref().expect("webhook").url.clone(),
                    events: temper_forge::WebhookEvents::All,
                }],
                ..ForgeInspection::default()
            }),
        )]),
        calls: Vec::new(),
    };

    let report = build_report(&bundle, &mut inspector).expect("report");
    let value = serde_json::to_value(&report).expect("json");

    assert_eq!(value["report_version"], 1);
    assert_eq!(value["repositories"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["repository"], value["repositories"][0]["repository"]);
    assert_eq!(value["labels"], value["repositories"][0]["labels"]);
    assert_eq!(value["webhook"], value["repositories"][0]["webhook"]);
    assert_eq!(value["metadata"], value["repositories"][0]["metadata"]);
    let json = value.to_string();
    assert!(!json.contains("admin-pass"), "{json}");
    assert!(!json.contains("webhook-secret-value"), "{json}");
    assert_eq!(inspector.calls, ["acme/service"]);
    assert_eq!(desired_users(&bundle).len(), report.identities.users.len());
}

#[test]
fn all_repositories_are_inspected_and_failures_are_aggregated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bundle(dir.path(), &["acme/api", "acme/web", "acme/docs"]);
    let opts = PlanOptions {
        options: LoadOptions {
            config: Some(path.join("config.toml")),
            credentials: Some(path.join("credentials.toml")),
        },
        existing_repo: true,
        ..Default::default()
    };
    let bundle = load_deployment(&opts.options, &opts.env, &opts.paths, true).expect("bundle");
    let mut inspector = RecordingInspector {
        inspections: BTreeMap::from([
            (
                "acme/api".to_string(),
                Ok(ForgeInspection {
                    inspected: true,
                    repository: Some(repository("acme", "api")),
                    metadata: MetadataInspection {
                        checked_artifacts: 1,
                        invalid: vec!["issue #7: malformed workflow metadata".to_string()],
                    },
                    ..ForgeInspection::default()
                }),
            ),
            (
                "acme/web".to_string(),
                Err("web repository unavailable".to_string()),
            ),
            (
                "acme/docs".to_string(),
                Ok(ForgeInspection {
                    inspected: true,
                    ..ForgeInspection::default()
                }),
            ),
        ]),
        calls: Vec::new(),
    };

    let report = build_report(&bundle, &mut inspector).expect("report");
    let value = serde_json::to_value(&report).expect("json");

    assert_eq!(
        inspector.calls,
        ["acme/api", "acme/web", "acme/docs"],
        "an unavailable middle repository must not short-circuit inspection"
    );
    assert_eq!(report.repositories.len(), 3);
    assert!(value.get("repository").is_none(), "{value}");
    assert!(value.get("labels").is_none(), "{value}");
    assert!(value.get("webhook").is_none(), "{value}");
    assert!(value.get("metadata").is_none(), "{value}");
    assert_eq!(report.status, "needs_attention");
    assert_eq!(report.result, "error");
    assert!(!report.forge.inspected);
    assert!(report.findings.iter().any(|finding| {
        finding.category == "metadata"
            && finding.message.contains("acme/api")
            && finding.message.contains("issue #7")
    }));
    assert!(
        report.findings.iter().any(|finding| {
            finding.severity == "error" && finding.message.contains("acme/docs")
        })
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.message.contains("acme/web"))
    );
}
