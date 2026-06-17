use crate::interaction_args::{ParseOutcome, USAGE, parse};
use serde_json::json;
use temper_interaction::RawInteractionSpec;

use crate::interaction_bindings::{
    ForgeBinding, InteractionDeploymentBindings, ProcessResponderBinding, ProfileBinding,
    ServiceBinding, build_service, service_options,
};

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

#[test]
fn generic_interaction_args_parse_repl_from_flags() {
    let parsed = parse(argv(&[
        "repl",
        "--spec",
        "/etc/temper/interactions.json",
        "--bindings",
        "/etc/temper/interaction-bindings.json",
        "--profile",
        "support-agent",
    ]))
    .unwrap();
    let ParseOutcome::Repl(args) = parsed else {
        panic!("expected repl args")
    };

    assert_eq!(
        args.spec_path.to_string_lossy(),
        "/etc/temper/interactions.json"
    );
    assert_eq!(
        args.bindings_path.to_string_lossy(),
        "/etc/temper/interaction-bindings.json"
    );
    assert_eq!(args.profile_id.as_str(), "support-agent");
    assert!(!format!("{args:?}").contains("TEMPER_PRODUCT_CHAT"));
    assert!(!USAGE.contains("TEMPER_PRODUCT_CHAT"));
}

#[test]
fn generic_interaction_args_repl_requires_flags() {
    // No `--spec`/`--bindings`/`--profile` and no env fallback ⇒ a clear error
    // naming the missing required flag.
    let error = parse(argv(&["repl"])).unwrap_err().to_string();
    assert!(error.contains("--spec"), "names the missing flag: {error}");
}

#[test]
fn generic_interaction_args_parse_serve_bind_override() {
    let parsed = parse(argv(&[
        "serve",
        "--spec",
        "spec.json",
        "--bindings",
        "bindings.json",
        "--bind",
        "127.0.0.1:39000",
    ]))
    .unwrap();
    let ParseOutcome::Serve(args) = parsed else {
        panic!("expected serve args")
    };
    assert_eq!(args.bind_override.unwrap().to_string(), "127.0.0.1:39000");
    assert!(!args.allow_non_loopback);
}

#[test]
fn generic_bindings_build_service_from_profile_and_responder_env_names() {
    let spec = one_profile_spec();
    let mut bindings = bindings_with_service(ServiceBinding::default());
    bindings.responders.insert(
        "support-responder".into(),
        ProcessResponderBinding {
            command: "/bin/echo".into(),
            args: Vec::new(),
            cwd: None,
            env_allowlist: vec!["LLM_API_KEY".into()],
            timeout_secs: Some(5),
        },
    );

    let cx = temper_engine_io::Cx::for_testing();
    let service = build_service(&cx, &spec, &bindings, |key| match key {
        "TEMPER_INTERACTION_HUMAN_TOKEN" => Some("human-token".into()),
        "TEMPER_INTERACTION_AGENT_TOKEN" => Some("agent-token".into()),
        _ => None,
    })
    .unwrap();

    assert_eq!(
        service.default_profile_id().unwrap().as_str(),
        "support-agent"
    );
}

#[test]
fn generic_service_options_enforce_non_loopback_auth() {
    let mut bindings = bindings_with_service(ServiceBinding {
        bind: Some("0.0.0.0:39000".into()),
        token_env: Some("TEMPER_INTERACTION_SERVICE_TOKEN".into()),
        allow_non_loopback: false,
    });

    let error = service_options(&bindings, None, false, |_| None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("non-loopback bind requires"));

    let error = service_options(&bindings, None, true, |_| None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("TEMPER_INTERACTION_SERVICE_TOKEN"));

    let options = service_options(&bindings, None, true, |key| {
        (key == "TEMPER_INTERACTION_SERVICE_TOKEN").then(|| "api-secret".into())
    })
    .unwrap();
    assert_eq!(options.bind.to_string(), "0.0.0.0:39000");
    assert_eq!(options.service_token.as_deref(), Some("api-secret"));
    assert!(!format!("{options:?}").contains("api-secret"));

    bindings.service = ServiceBinding {
        bind: Some("127.0.0.1:39000".into()),
        token_env: None,
        allow_non_loopback: false,
    };
    assert!(service_options(&bindings, None, false, |_| None).is_ok());
}

fn bindings_with_service(service: ServiceBinding) -> InteractionDeploymentBindings {
    InteractionDeploymentBindings {
        forge: ForgeBinding {
            base_url: "https://git.example.test".into(),
        },
        repository: "ai/temper".into(),
        default_profile: Some("support-agent".into()),
        service,
        profiles: [("support-agent".into(), profile_binding())]
            .into_iter()
            .collect(),
        responders: Default::default(),
    }
}

fn profile_binding() -> ProfileBinding {
    ProfileBinding {
        human_token_env: "TEMPER_INTERACTION_HUMAN_TOKEN".into(),
        agent_token_env: "TEMPER_INTERACTION_AGENT_TOKEN".into(),
    }
}

fn one_profile_spec() -> temper_interaction::CompiledInteractionSpec {
    let raw: RawInteractionSpec = serde_json::from_value(json!({
        "id": "support-interactions",
        "responders": [{
            "id": "support-responder",
            "protocol": "process-v1",
            "required": true
        }],
        "profiles": [{
            "id": "support-agent",
            "transcript": {
                "target": "issue",
                "title_prefix": "Support conversation",
                "labels": ["support-chat"],
                "label_policy": "exact",
                "marker_namespace": "support-chat",
                "recent_turn_limit": 30
            },
            "participants": {
                "human": { "display_name": "human" },
                "agent": { "display_name": "support-agent" }
            },
            "responder": "support-responder",
            "proposal_kinds": [{ "id": "issue", "payload": "issue_draft" }],
            "commands": [{
                "id": "file-draft",
                "aliases": ["/file"],
                "action": {
                    "accept_proposal": {
                        "kind": "issue",
                        "acceptance_action": "file-draft"
                    }
                }
            }],
            "acceptance_actions": [{
                "id": "file-draft",
                "proposal_kind": "issue",
                "acceptance": { "policy": "explicit", "commands": ["file-draft"] },
                "idempotency_key": "${conversation.id}:${proposal.id}",
                "effects": [{
                    "kind": "create_issue",
                    "title": "${proposal.payload.title}",
                    "body_template": "${proposal.payload.body}\n\n${effect.marker}",
                    "labels": ["untriaged"],
                    "marker_namespace": "support-chat",
                    "marker_key": "file"
                }]
            }]
        }]
    }))
    .unwrap();
    raw.validate().unwrap().compile()
}
