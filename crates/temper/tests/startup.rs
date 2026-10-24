#[path = "support/listeners.rs"]
pub mod listeners;
pub mod support;
use std::{
    fmt::Write,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn a_configuration_file_past_its_bound_is_refused() {
    let scratch = support::Scratch::startup("configuration-bound");
    let directory = scratch.path();
    fs::File::create(directory.join("temper.json"))
        .expect("configuration file")
        .set_len(1_048_577)
        .expect("one byte past bound");
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(directory)).err().expect("refused"),
        format!("temper serve: configuration {}: past 1048576 bytes\n", directory.join("temper.json").display())
    );
    scratch.remove();
}
#[test]
fn a_secret_file_readable_by_others_refuses_the_start_naming_it() {
    let scratch = support::Scratch::startup("public-token");
    let directory = scratch.path();
    support::configuration(directory, &support::Forge::Unreached);
    fs::set_permissions(directory.join("secrets/forge-1.git-token"), fs::Permissions::from_mode(0o644))
        .expect("public permissions");
    let refused = temper::prepare_in(&support::configuration::directories(directory)).err().expect("start refused");
    assert_eq!(
        refused,
        format!(
            "temper serve: application.identities[0].token: {}: readable by others or reached through a link\n",
            scratch.path().join("secrets/forge-1.git-token").display()
        )
    );
    assert!(!refused.contains("forge-1-git-token-value"), "no secret in a diagnostic");
    scratch.remove();
}
#[test]
fn a_worker_root_reached_through_a_link_is_refused() {
    let scratch = support::Scratch::startup("linked-root");
    let directory = scratch.path();
    support::configuration(directory, &support::Forge::Unreached);
    fs::create_dir(directory.join("real-root")).expect("root directory");
    symlink(directory.join("real-root"), directory.join("worker-root")).expect("linked root");
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(directory)).err().expect("refused"),
        "temper serve: application.workers[0].root: must be a private directory of this user's, not reached through a link\n"
    );
    scratch.remove();
}
#[test]
fn problems_reach_stderr_one_line_each() {
    let scratch = support::Scratch::startup("problem-lines");
    let directory = scratch.path();
    fs::write(directory.join("temper.json"), b"{}").expect("configuration");
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(directory)).err().expect("refused"),
        "temper serve: engine: missing\ntemper serve: workers: missing\ntemper serve: application: missing\n"
    );
    scratch.remove();
}

#[test]
fn an_address_with_no_token_in_the_directory_is_not_signed_in() {
    for (row, token, expected) in [
        ("absent", None, "temper open: not signed in: give the link temper serve printed\n"),
        ("empty", Some(b"".as_slice()), "temper open: not signed in: give the link temper serve printed\n"),
    ] {
        let scratch = support::Scratch::startup(&format!("open-{row}"));
        if let Some(token) = token {
            fs::write(scratch.path().join("token"), token).expect("token file");
            fs::set_permissions(scratch.path().join("token"), fs::Permissions::from_mode(0o600))
                .expect("private token");
        }
        assert_eq!(
            temper::open::check_token(scratch.path()).expect_err("not signed in").line(scratch.path()),
            expected,
            "{row}"
        );
        scratch.remove();
    }
}

#[test]
fn a_user_file_readable_by_others_refuses_the_start_naming_it() {
    let scratch = support::Scratch::startup("public-user");
    support::configuration(scratch.path(), &support::Forge::Unreached);
    fs::set_permissions(scratch.path().join("secrets/forge-1.user"), fs::Permissions::from_mode(0o644))
        .expect("fixture operation");
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("configuration refused"),
        format!(
            "temper serve: application.forges[0].user.file: {}: readable by others or reached through a link\n",
            scratch.path().join("secrets/forge-1.user").display()
        )
    );
    scratch.remove();
}
fn managed(directory: &std::path::Path, move_files: bool) -> serde_json::Value {
    support::configuration(directory, &support::Forge::Unreached);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("temper.json")).expect("configuration bytes"))
            .expect("fixture operation");
    value["application"]["setup"] = serde_json::json!([{
        "forge":1,"directory":"forge-1","bot":{"login":"temper","email":"temper@example.test"},
        "orgs":["acme"],"mode":{"managed":{"admin_token":"secrets/forgejo-admin"}}
    }]);
    if move_files {
        fs::create_dir(directory.join("forge-1")).expect("fixture operation");
        for (from, to) in [
            ("secrets/forge-1.api-token", "api_token"),
            ("secrets/forge-1.git-token", "git_token"),
            ("secrets/forge-1.user", "user"),
        ] {
            fs::rename(directory.join(from), directory.join("forge-1").join(to)).expect("fixture operation");
        }
        value["application"]["forges"][0]["api_token"] = "forge-1/api_token".into();
        value["application"]["forges"][0]["user"] = serde_json::json!({"file":"forge-1/user"});
        value["application"]["identities"][0]["token"] = "forge-1/git_token".into();
    }
    support::write_private(&directory.join("temper.json"), &serde_json::to_vec(&value).expect("configuration JSON"));
    value
}
#[test]
fn a_setup_naming_another_api_token_file_refuses_temper_serve_naming_its_path() {
    let scratch = support::Scratch::startup("setup-token");
    let mut value = managed(scratch.path(), true);
    value["application"]["forges"][0]["api_token"] = "another/api_token".into();
    support::write_private(
        &scratch.path().join("temper.json"),
        &serde_json::to_vec(&value).expect("configuration JSON"),
    );
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("configuration refused"),
        "temper serve: application.forges[0].api_token: invalid form\n"
    );
    scratch.remove();
}
#[test]
fn a_setup_limit_past_its_check_refuses_temper_serve_naming_both_limits() {
    let scratch = support::Scratch::startup("setup-limits");
    let mut value = managed(scratch.path(), true);
    value["application"]["limits"]["overrides"]["setup.backoff_ms"] = 6000.into();
    support::write_private(
        &scratch.path().join("temper.json"),
        &serde_json::to_vec(&value).expect("configuration JSON"),
    );
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("configuration refused"),
        "temper serve: application.limits.overrides.setup.backoff_ms: must agree with application.limits.overrides.setup.backoff_max_ms\n"
    );
    scratch.remove();
}
#[test]
fn a_managed_entrys_absent_admin_token_file_does_not_stop_temper_serve() {
    let scratch = support::Scratch::startup("setup-no-admin");
    managed(scratch.path(), true);
    assert!(!scratch.path().join("secrets/forgejo-admin").exists());
    assert!(temper::prepare_in(&support::configuration::directories(scratch.path())).is_ok());
    scratch.remove();
}
fn box_document(scratch: &support::Scratch) -> serde_json::Value {
    support::configuration(scratch.path(), &support::Forge::Unreached);
    serde_json::from_slice(&fs::read(scratch.path().join("temper.json")).expect("configuration")).expect("JSON")
}
fn refused_box(scratch: &support::Scratch, value: &serde_json::Value) -> String {
    support::write_private(&scratch.path().join("temper.json"), &serde_json::to_vec(value).expect("JSON"));
    temper::run(temper::Command::Box { directories: support::configuration::directories(scratch.path()) })
        .expect_err("box refused")
}
#[test]
fn a_box_on_a_fresh_init_reaches_serves_link_with_forge_3_dormant() {
    let scratch = support::Scratch::startup("box-fresh-init");
    let init = std::process::Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("init")
        .args(support::configuration::flags(scratch.path()))
        .output()
        .expect("init command");
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    // init's ports are fixed when free; tests run side by side, so each listener moves to a drawn one.
    move_listeners(scratch.path());
    let ended =
        support::boxed::run(scratch.path(), None, support::boxed::Plan::Stop, skein_lib::Duration::from_secs(6));
    let diagnostic = String::from_utf8_lossy(&ended.stderr);
    let link = support::printed_link(&ended.stdout).unwrap_or_else(|| panic!("serve's link: {diagnostic}"));
    assert!(skein_lib::bytes::find(link, b"/jig/v1/sign-in/link?secret=").is_some(), "{diagnostic}");
    assert!(!diagnostic.contains("application.setup"), "{diagnostic}");
    assert_eq!(ended.exit, Some(skein_io::kernel::Exit::Code(0)), "{diagnostic}");
    assert_eq!(ended.left, Some(Vec::new()), "no process outlived the box: {diagnostic}");
    // Forge 3 is asked: its group never ran, so it has no directory and nothing was fetched.
    for absent in ["forge-3", "forgejo", "forgejo-runner"] {
        assert!(!scratch.path().join(absent).exists(), "{absent}: {diagnostic}");
    }
    scratch.remove();
}
/// Move each listener a fresh init names to a drawn loopback port, in both of its documents.
fn move_listeners(directory: &std::path::Path) {
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(directory.join(name)).expect("init's document")).expect("JSON")
    };
    let mut deployment = read("temper.json");
    let application = &deployment["application"];
    let named = [
        &application["clients"]["listener"]["socket"],
        &application["forges"][0]["api"]["socket"],
        &application["forges"][2]["api"]["socket"],
        &application["demo"]["llm"]["listen"]["socket"],
        &application["webhooks"]["listener"]["socket"],
    ]
    .map(|socket| socket.as_str().expect("init's socket").to_owned());
    let drawn = support::free_addresses::<5>(&[]);
    let moves: Vec<_> = named.into_iter().zip(drawn.map(|address| address.to_string())).collect();
    let mut agent = read("agent.json");
    for (name, value) in [("temper.json", &mut deployment), ("agent.json", &mut agent)] {
        replace_sockets(value, &moves);
        support::write_private(&directory.join(name), &serde_json::to_vec(value).expect("JSON"));
    }
}

/// Each string naming a moved socket, alone or as an `http://` URL, names its new one.
fn replace_sockets(value: &mut serde_json::Value, moves: &[(String, String)]) {
    match value {
        serde_json::Value::String(text) => {
            for (before, after) in moves {
                if text == before {
                    text.clone_from(after);
                } else if text.strip_prefix("http://") == Some(before.as_str()) {
                    *text = format!("http://{after}");
                } else {
                    continue;
                }
                break;
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|item| replace_sockets(item, moves)),
        serde_json::Value::Object(items) => items.values_mut().for_each(|item| replace_sockets(item, moves)),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}
#[test]
fn a_fake_entrys_handover_is_read_privately_and_checked_before_serve_starts() {
    for case in ["missing", "public", "short", "uppercase", "valid"] {
        let scratch = support::Scratch::startup(&format!("fake-handover-{case}"));
        let mut value = managed(scratch.path(), true);
        support::configuration::fake_entry(&mut value["application"]["setup"][0], "127.0.0.1:9");
        let handover = scratch.path().join("forge-1/admin_token");
        if case != "missing" {
            let bytes = match case {
                "short" => b"0123456789abcdef".as_slice(),
                "uppercase" => b"0123456789abcdef0123456789abcdef0123456A",
                _ => b"0123456789abcdef0123456789abcdef01234567",
            };
            support::write_private(&handover, bytes);
            if case == "public" {
                fs::set_permissions(&handover, fs::Permissions::from_mode(0o644)).expect("public handover");
            }
        }
        support::write_private(&scratch.path().join("temper.json"), &serde_json::to_vec(&value).expect("JSON"));
        let result = temper::envelope_in(
            &support::configuration::directories(scratch.path()),
            &serde_json::to_vec(&value).expect("JSON"),
        );
        match case {
            "valid" => {
                assert!(result.is_ok());
            }
            "missing" | "public" => {
                let why = if case == "missing" { "absent" } else { "readable by others or reached through a link" };
                assert_eq!(
                    result.expect_err("handover refused"),
                    format!("temper serve: application.setup[0].admin_token: {}: {why}\n", handover.display())
                );
            }
            _ => assert_eq!(
                result.expect_err("handover refused"),
                format!(
                    "temper serve: application.setup[0].admin_token: {}: admin_token must be exactly 40 lowercase hexadecimal digits\n",
                    handover.display()
                )
            ),
        }
        scratch.remove();
    }
}

#[test]
fn a_nonempty_box_configuration_without_a_document_is_refused_naming_it() {
    let scratch = support::Scratch::startup("box-absent-document");
    fs::write(scratch.path().join("other"), b"kept").expect("nonempty configuration");
    assert_eq!(
        temper::run(temper::Command::Box { directories: support::configuration::directories(scratch.path()) }),
        Err(format!(
            "temper box: configuration {}: absent; temper init writes one\n",
            scratch.path().join("temper.json").display()
        ))
    );
    scratch.remove();
}
#[test]
fn a_box_configuration_missing_members_is_refused_one_line_each() {
    let scratch = support::Scratch::startup("box-members");
    assert_eq!(
        refused_box(&scratch, &serde_json::json!({})),
        "temper box: engine: missing\ntemper box: workers: missing\ntemper box: application: missing\n"
    );
    scratch.remove();
}
#[test]
fn a_setup_naming_no_forge_refuses_the_box_before_it_starts() {
    let scratch = support::Scratch::startup("box-forge");
    let mut value = managed(scratch.path(), false);
    value["application"]["setup"][0]["forge"] = 7.into();
    assert_eq!(refused_box(&scratch, &value), "temper box: application.setup[0].forge: unknown reference\n");
    assert!(!scratch.path().join("forge-7").exists());
    scratch.remove();
}
#[test]
fn a_box_maximum_backoff_below_its_base_is_refused() {
    let scratch = support::Scratch::startup("box-backoff");
    let mut value = box_document(&scratch);
    value["application"]["limits"]["overrides"]["box.backoff_max_ms"] = 100.into();
    assert_eq!(
        refused_box(&scratch, &value),
        "temper box: application.limits.overrides.box.backoff_ms: must agree with box.backoff_max_ms\n"
    );
    scratch.remove();
}
#[test]
fn a_box_maximum_backoff_reaching_its_window_is_refused() {
    let scratch = support::Scratch::startup("box-window");
    let mut value = box_document(&scratch);
    value["application"]["limits"]["overrides"]["box.backoff_max_ms"] = 300_000.into();
    assert_eq!(
        refused_box(&scratch, &value),
        "temper box: application.limits.overrides.box.backoff_max_ms: must agree with box.window_ms\n"
    );
    scratch.remove();
}
#[test]
fn a_box_restart_count_past_its_bound_is_refused() {
    let scratch = support::Scratch::startup("box-restarts");
    let mut value = box_document(&scratch);
    value["application"]["limits"]["overrides"]["box.restarts"] = 65.into();
    assert_eq!(
        refused_box(&scratch, &value),
        "temper box: application.limits.overrides.box.restarts: past its bound\n"
    );
    scratch.remove();
}
#[test]
fn a_box_restart_count_outside_its_type_is_refused() {
    let scratch = support::Scratch::startup("box-restarts-type");
    let mut value = box_document(&scratch);
    value["application"]["limits"]["overrides"]["box.restarts"] = 4_294_967_296_u64.into();
    assert_eq!(
        refused_box(&scratch, &value),
        "temper box: application.limits.overrides.box.restarts: outside its type\n"
    );
    scratch.remove();
}
#[test]
fn an_unknown_box_limit_is_refused() {
    let scratch = support::Scratch::startup("box-unknown");
    let mut value = box_document(&scratch);
    value["application"]["limits"]["overrides"]["box.nope"] = 1.into();
    assert_eq!(
        refused_box(&scratch, &value),
        "temper box: application.limits.overrides.box.nope: no limit of temper's has this name\n"
    );
    scratch.remove();
}
#[test]
fn a_box_limit_past_its_check_refuses_temper_serve_too() {
    let scratch = support::Scratch::startup("box-serve-refused");
    let mut value = box_document(&scratch);
    value["application"]["limits"]["overrides"]["box.backoff_max_ms"] = 100.into();
    support::write_private(&scratch.path().join("temper.json"), &serde_json::to_vec(&value).expect("JSON"));
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("serve refused"),
        "temper serve: application.limits.overrides.box.backoff_ms: must agree with box.backoff_max_ms\n"
    );
    scratch.remove();
}
#[test]
fn a_box_limit_within_its_check_is_accepted_by_temper_serve() {
    let scratch = support::Scratch::startup("box-serve-accepted");
    let mut value = box_document(&scratch);
    value["application"]["limits"]["overrides"]["box.restarts"] = 3.into();
    support::write_private(&scratch.path().join("temper.json"), &serde_json::to_vec(&value).expect("JSON"));
    assert!(temper::prepare_in(&support::configuration::directories(scratch.path())).is_ok());
    scratch.remove();
}

#[test]
fn a_webhook_secret_readable_by_others_refuses_startup() {
    let scratch = support::Scratch::startup("public-hook-secret");
    let _ = support::webhooks::configuration(scratch.path());
    fs::set_permissions(scratch.path().join("secrets/forge-1.webhook-secret"), fs::Permissions::from_mode(0o644))
        .expect("public permissions");
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("configuration refused"),
        format!(
            "temper serve: application.forges[0].webhook_secret: {}: readable by others or reached through a link\n",
            scratch.path().join("secrets/forge-1.webhook-secret").display()
        )
    );
    scratch.remove();
}

fn owned_example(case: &str) -> support::Scratch {
    let scratch = support::Scratch::startup(case);
    let owned = support::owned::Owned::example();
    support::owned::lay(scratch.path(), &owned);
    fs::write(scratch.path().join("temper.json"), support::owned::example_document(scratch.path(), &owned))
        .expect("example configuration");
    scratch
}
#[test]
fn the_owned_example_starts_without_credential_files() {
    let scratch = owned_example("owned-example-records");
    let prepared = temper::prepare_in(&support::configuration::directories(scratch.path()));
    assert!(prepared.is_ok(), "{}", prepared.err().unwrap_or_default());
    assert!(!scratch.path().join("forge-1").exists(), "startup writes no forge files");
    scratch.remove();
}

#[test]
fn serve_prepares_once_on_the_owned_example() {
    let scratch = owned_example("owned-example-prepared");
    let prepared = temper::prepare_in(&support::configuration::directories(scratch.path()));
    assert!(prepared.is_ok(), "{}", prepared.err().unwrap_or_default());
    scratch.remove();
}
#[test]
fn serve_checks_the_owned_agents_admission_at_each_bound() {
    let scratch = owned_example("owned-admission-bounds");
    let path = scratch.path().join("owned/agent.json");
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("agent fixture")).expect("agent JSON");
    for (member, value, expected) in [
        (
            "max_turns",
            7,
            Some("engine.charters[0].turns: needs 8 turns; the agent's profile.policy.max_turns allows 7\n"),
        ),
        ("max_turns", 8, None),
        (
            "check_timeout",
            60001,
            Some(
                "engine.charters[0].reserve_time_ms: needs checks within 60000 ms; the agent's profile.policy.check_timeout is 60001 ms\n",
            ),
        ),
        ("check_timeout", 60000, None),
        (
            "check_timeout",
            300_000,
            Some(
                "engine.charters[0].reserve_time_ms: needs checks within 60000 ms; the agent's profile.policy.check_timeout is 300000 ms\n",
            ),
        ),
        (
            "tool_payload",
            4097,
            Some(
                "engine.limits: needs 4096-byte messages (tasks.message_bytes); the agent offers 4095 (tool_payload less 2)\n",
            ),
        ),
        ("tool_payload", 4098, None),
        (
            "tool_payload",
            600,
            Some(
                "engine.limits: needs 4096-byte messages (tasks.message_bytes); the agent offers 598 (tool_payload less 2)\n",
            ),
        ),
    ] {
        let mut agent = original.clone();
        let section = if member == "tool_payload" { "declared" } else { "policy" };
        agent["profile"][section][member] = value.into();
        support::write_private(&path, &serde_json::to_vec(&agent).expect("agent JSON"));
        let result = temper::prepare_in(&support::configuration::directories(scratch.path()));
        match expected {
            Some(line) => assert_eq!(
                result.err().expect("admission refused"),
                format!("temper serve: {line}"),
                "{member}={value}"
            ),
            None => assert!(result.is_ok(), "{member}={value}: {}", result.err().unwrap_or_default()),
        }
    }
    let mut agent = original.clone();
    agent["profile"]["policy"]["max_turns"] = 5.into();
    agent["profile"]["policy"]["check_timeout"] = 300_000.into();
    agent["profile"]["declared"]["tool_payload"] = 600.into();
    support::write_private(&path, &serde_json::to_vec(&agent).expect("agent JSON"));
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("old tuple refused"),
        "temper serve: engine.charters[0].turns: needs 8 turns; the agent's profile.policy.max_turns allows 5\n"
    );
    // The selected charter's number identifies its path, even when it is not its index.
    let config_path = scratch.path().join("temper.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).expect("configuration")).expect("JSON");
    assert_eq!(document["engine"]["charters"][1]["number"].as_u64(), Some(2));
    document["engine"]["charters"][0]["turns"] = 4.into();
    support::write_private(&config_path, &serde_json::to_vec(&document).expect("configuration JSON"));
    agent = original;
    agent["profile"]["policy"]["max_turns"] = 7.into();
    support::write_private(&path, &serde_json::to_vec(&agent).expect("agent JSON"));
    assert_eq!(
        temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("second charter refused"),
        "temper serve: engine.charters[1].turns: needs 8 turns; the agent's profile.policy.max_turns allows 7\n"
    );
    scratch.remove();
}

fn configuration_document(value: &serde_json::Value) -> skein_json::Document {
    jig_protocol_config::document(
        &serde_json::to_vec(value).expect("configuration JSON"),
        &jig_protocol_config::DocumentLimits { bytes: 1_048_576, tokens: 65_536, depth: 32 },
    )
    .expect("configuration document")
}

#[test]
fn the_shipped_chat_is_ended_by_its_requester() {
    let scratch = support::Scratch::startup("owned-ending-authority");
    let bounded = support::owned::bounded(scratch.path());
    let value: serde_json::Value = serde_json::from_slice(&bounded.document).expect("deployment JSON");
    let engine = jig_protocol_config::engine(
        &configuration_document(&value["engine"]),
        0,
        &jig_protocol_config::Directories::working(),
    )
    .expect("engine configuration");
    assert_eq!(engine.templates[0].number, 1);
    assert_eq!(engine.templates[0].flags.ended_by, jig_domain_tasks::EndedBy::Requester);
    assert_eq!(engine.templates[1].number, 2);
    assert_eq!(engine.templates[1].flags.ended_by, jig_domain_tasks::EndedBy::Result);
    scratch.remove();
}

#[test]
fn shipped_bounds_fit_the_configured_charters_and_worker() {
    use jig_protocol_config as config;
    use temper_service as service;

    let scratch = support::Scratch::startup("owned-admission");
    let bounded = support::owned::bounded(scratch.path());
    let value: serde_json::Value = serde_json::from_slice(&bounded.document).expect("deployment JSON");
    let engine = config::engine(&configuration_document(&value["engine"]), 0, &config::Directories::working())
        .expect("engine configuration");
    let agent = &bounded.agent;
    let core = config::core(&engine, [0; 16], 1);
    let needs = jig_domain_core::fleet_needs::configured(&core, &engine.limits.core);
    assert_eq!(
        jig_domain_fleet::shortfall(
            &needs,
            &temper_engine_domain::hello_bounds(agent.bounds.clone()),
            jig_domain_fleet::HostKind::Worker,
        ),
        None,
        "the shipped worker is admitted by the canonical fleet check"
    );
    // The minimum Smith payload is the engine's message requirement plus
    // Smith's rendered separator; its advertised message excludes that separator.
    let payload = u64::from(engine.limits.core.tasks.message_bytes)
        .checked_add(temper_worker_domain::SEPARATOR)
        .expect("rendered message");
    assert!(agent.host_limits.message_bytes >= payload);
    let named = service::named(&bounded.document, &config::Directories::working()).expect("owned configuration");
    let worker = &service::settings::Worker {
        jig: config::worker(&configuration_document(&value["workers"][0]), 0, &config::Directories::working())
            .expect("worker configuration"),
        root: named.worker.expect("standalone worker").root,
        git: Some(b"/usr/bin/git".as_slice().into()),
    };
    let mut own = service::limits::profile(service::limits::Profile::Small);
    for (name, number) in value["application"]["limits"]["overrides"].as_object().expect("overrides") {
        service::limits::apply(&mut own, name.as_bytes(), number.as_u64().expect("limit number"))
            .expect("application override");
    }
    let own = &own;
    assert!(worker.jig.limits.hub.event_bytes >= u64::from(agent.bounds.message_bytes));
    assert!(u64::from(own.checkout.message_bytes) >= agent.host_limits.call_bytes);
    let prepared = service::worker_settings::prepare(worker, agent, own).expect("complete worker bounds");
    // Each receiver one byte below its actual sender's bound fails its own rule.
    let mut short = prepared.root.clone();
    short.host.event_bytes = u64::from(agent.bounds.message_bytes).checked_sub(1).expect("nonempty message");
    assert_eq!(temper_worker_domain::unfit(&short).as_slice(), &[temper_worker_domain::Unfit::BoundsMessage]);
    let mut short = prepared.root.clone();
    short.checkout.message_bytes = u32::try_from(agent.host_limits.call_bytes)
        .expect("call fits checkout bound")
        .checked_sub(1)
        .expect("nonempty call");
    assert_eq!(temper_worker_domain::unfit(&short).as_slice(), &[temper_worker_domain::Unfit::CallBytes]);
    let required = service::worker_worst_case(worker, agent, own).expect("worker memory expression");
    assert!(required <= worker.jig.memory, "worker needs {required}, declares {}", worker.jig.memory);
    scratch.remove();
}

#[test]
fn the_owned_example_is_named_without_a_problem() {
    // Only naming a fixture document; no deployment or files are opened.
    let named = temper_service::named(support::owned::EXAMPLE, &jig_protocol_config::Directories::working())
        .expect("owned example named");
    assert_eq!(named.worker.as_ref().expect("standalone worker").agent.as_ref(), b"/home/me/temper/owned/agent.json");
    assert_eq!(named.worker.as_ref().expect("standalone worker").root.as_ref(), b"workspaces");
    assert_eq!(named.deployment.as_ref(), b"state");
    for path in [b"forge-1/api_token".as_slice(), b"forge-1/user", b"forge-1/webhook_secret", b"forge-1/git_token"] {
        assert_eq!(
            named.files.iter().find(|file| file.path.as_ref() == path).map(|file| file.private),
            None,
            "records-only owned input names no file: {path:?}"
        );
    }
}

#[test]
fn every_command_checks_setup_before_reading_files() {
    let scratch = owned_example("every-command-setup");
    let path = scratch.path().join("temper.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).expect("configuration")).expect("JSON");
    value["application"]["limits"]["overrides"]["setup.backoff_ms"] = 6000.into();
    let bytes = serde_json::to_vec(&value).expect("configuration JSON");
    support::write_private(&path, &bytes);
    let line = "application.limits.overrides.setup.backoff_ms: must agree with application.limits.overrides.setup.backoff_max_ms\n";
    for (command, refused) in [
        (
            "serve",
            temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("serve refused"),
        ),
        (
            "box",
            temper::run(temper::Command::Box { directories: support::configuration::directories(scratch.path()) })
                .expect_err("box refused"),
        ),
    ] {
        assert_eq!(refused, format!("temper {command}: {line}"), "{command}");
    }
    for verb in ["prepare", "reconcile", "run"] {
        support::owned::refused(scratch.path(), verb, format!("temper forge {verb}: forge 1: {line}").as_bytes());
    }
    let runner = support::owned::command(scratch.path(), "runner").output().expect("runner refuses");
    assert_eq!(runner.status.code(), Some(1));
    assert_eq!(runner.stderr, format!("temper forge run: forge 1: {line}").as_bytes());
    scratch.remove();
}

const PAGE: &[u8] = b"<!doctype html><title>t</title>";
const DIGEST: &str = "bundle/0123456789abcdef";

/// A refusal's row: its name, what spoils the bundle, and the start's refusal.
type Spoiled<'a> = (&'a str, fn(&std::path::Path), &'a str);
/// A limit's row: its name, the configuration's change, and the start's refusal.
type Limited<'a> = (&'a str, fn(&mut serde_json::Value), &'a str);

const BUNDLE_FILES: &[(&str, &[u8])] = &[
    ("index.html", PAGE),
    ("0123456789abcdef/boot.js", b"boot();"),
    ("0123456789abcdef/files/robots.txt", b"User-agent: *\nDisallow:\n"),
];
// The page, boot.js and robots.txt: three files, 31 + 7 + 24 bytes.
const BUNDLE_COUNT: u32 = 3;
const BUNDLE_BYTES: u32 = 62;
// A .js path at the service's 1024-byte bound.
const PATH_STEM_BOUND: usize = temper_service::clients::bundle::NAME_BYTES as usize - b".js".len();
const BUNDLE_LIMITS: temper_service::limits::Clients =
    temper_service::limits::Clients { bundle_files: 128, bundle_bytes: 16 << 20 };
fn fixture_bundle(directory: &std::path::Path) {
    for (name, bytes) in BUNDLE_FILES {
        let file = directory.join("bundle").join(name);
        fs::create_dir_all(file.parent().expect("bundle parent")).expect("bundle directory");
        fs::write(file, bytes).expect("bundle file");
    }
}
fn bundle_configuration(directory: &std::path::Path) {
    support::configuration(directory, &support::Forge::Unreached);
    let document = directory.join("temper.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&document).expect("configuration")).expect("JSON");
    value["application"]["clients"]["bundle"] = directory.join("bundle").to_str().expect("bundle path").into();
    support::write_private(&document, &serde_json::to_vec(&value).expect("JSON"));
}
#[test]
fn a_bundle_directory_is_read_with_its_one_digest_directory() {
    let scratch = support::Scratch::startup("bundle-read");
    let directory = scratch.path();
    fixture_bundle(directory);
    let limits = temper_service::limits::Clients { bundle_files: 128, bundle_bytes: 16 << 20 };
    let bundle =
        temper::bundle::read(directory.join("bundle").as_os_str().as_encoded_bytes(), limits).expect("bundle read");
    assert_eq!(bundle.page.as_ref(), PAGE);
    assert_eq!(bundle.etag.as_ref(), b"\"edaa28e315690734\"");
    assert_eq!(bundle.digest.as_ref(), b"0123456789abcdef");
    assert_eq!(bundle.files.len(), 2);
    let script = bundle.files.get(b"boot.js".as_slice()).expect("script");
    assert_eq!((script.kind, script.bytes.as_ref()), (b"text/javascript".as_slice(), b"boot();".as_slice()));
    let robots = bundle.files.get(b"files/robots.txt".as_slice()).expect("root file");
    assert_eq!(
        (robots.kind, robots.bytes.as_ref()),
        (b"text/plain; charset=utf-8".as_slice(), b"User-agent: *\nDisallow:\n".as_slice())
    );
    scratch.remove();
}
#[test]
fn embedded_and_directory_files_make_the_same_bundle() {
    let scratch = support::Scratch::startup("bundle-embedded");
    fixture_bundle(scratch.path());
    // Root files beside index.html are ignored in both reads.
    fs::write(scratch.path().join("bundle/ignored.bin"), b"ignored").expect("root file");
    let mut files = BUNDLE_FILES.to_vec();
    files.push(("ignored.bin", b"ignored"));
    let directory = temper::bundle::read(scratch.path().join("bundle").as_os_str().as_encoded_bytes(), BUNDLE_LIMITS)
        .expect("directory");
    let embedded = temper::bundle::embedded(&files, BUNDLE_LIMITS).expect("embedded");
    assert_eq!(embedded.page, directory.page);
    assert_eq!(embedded.etag, directory.etag);
    assert_eq!(embedded.digest, directory.digest);
    assert_eq!(embedded.files.len(), directory.files.len());
    for (name, _) in BUNDLE_FILES.iter().skip(1) {
        let (_, relative) = name.split_once('/').expect("digest path");
        let from_directory = directory.files.get(relative.as_bytes()).expect("directory file");
        let from_embedded = embedded.files.get(relative.as_bytes()).expect("embedded file");
        assert_eq!(from_embedded.kind, from_directory.kind);
        assert_eq!(from_embedded.bytes, from_directory.bytes);
    }
    scratch.remove();
}

#[test]
fn embedded_files_refuse_each_spoiled_bundle_and_limit() {
    use temper_service::limits::Clients;
    let long = format!("0123456789abcdef/{}.js", "a".repeat(PATH_STEM_BOUND + 1)); // 1025 bytes below the digest.
    let rows = [
        ("no page", BUNDLE_FILES[1..].to_vec(), BUNDLE_LIMITS, "index.html: absent".to_owned()),
        ("no digest", BUNDLE_FILES[..1].to_vec(), BUNDLE_LIMITS, "no digest directory".to_owned()),
        (
            "two digests",
            vec![("index.html", PAGE), ("0123456789abcdef/a.js", b""), ("fedcba9876543210/a.js", b"")],
            BUNDLE_LIMITS,
            "fedcba9876543210: more than one digest directory".to_owned(),
        ),
        (
            "bad digest",
            vec![("index.html", PAGE), ("0123456789abcdeF/a.js", b"")],
            BUNDLE_LIMITS,
            "0123456789abcdeF: digest must be 16 lowercase hex digits".to_owned(),
        ),
        (
            "short digest",
            vec![("index.html", PAGE), ("abc/a.js", b"")],
            BUNDLE_LIMITS,
            "abc: digest must be 16 lowercase hex digits".to_owned(),
        ),
        (
            "parent path",
            vec![("index.html", PAGE), ("0123456789abcdef/../a.js", b"")],
            BUNDLE_LIMITS,
            "0123456789abcdef/../a.js: path must be plain and at most 1024 bytes".to_owned(),
        ),
        (
            "dot path",
            vec![("index.html", PAGE), ("0123456789abcdef/./a.js", b"")],
            BUNDLE_LIMITS,
            "0123456789abcdef/./a.js: path must be plain and at most 1024 bytes".to_owned(),
        ),
        (
            "empty component",
            vec![("index.html", PAGE), ("0123456789abcdef//a.js", b"")],
            BUNDLE_LIMITS,
            "0123456789abcdef//a.js: path must be plain and at most 1024 bytes".to_owned(),
        ),
        (
            "long path",
            vec![("index.html", PAGE), (long.as_str(), b"")],
            BUNDLE_LIMITS,
            format!("{long}: path must be plain and at most 1024 bytes"),
        ),
        (
            "closed list",
            vec![("index.html", PAGE), ("0123456789abcdef/data.bin", b"")],
            BUNDLE_LIMITS,
            "0123456789abcdef/data.bin: file type is not on the closed list".to_owned(),
        ),
        (
            "files",
            BUNDLE_FILES.to_vec(),
            Clients { bundle_files: BUNDLE_COUNT - 1, ..BUNDLE_LIMITS },
            "0123456789abcdef/files/robots.txt: past clients.bundle_files".to_owned(),
        ),
        (
            "bytes",
            BUNDLE_FILES.to_vec(),
            Clients { bundle_bytes: BUNDLE_BYTES - 1, ..BUNDLE_LIMITS },
            "0123456789abcdef/files/robots.txt: past clients.bundle_bytes".to_owned(),
        ),
    ];
    for (name, files, limits, why) in rows {
        assert_eq!(
            temper::bundle::embedded(&files, limits).expect_err("refused"),
            format!("application.clients.bundle: the binary's bundle: {why}"),
            "{name}"
        );
    }
}

#[test]
fn embedded_files_at_each_bound_are_admitted() {
    let bound = format!("0123456789abcdef/{}.js", "a".repeat(PATH_STEM_BOUND));
    for (name, files, limits) in [
        (
            "files and bytes",
            BUNDLE_FILES.to_vec(),
            temper_service::limits::Clients { bundle_files: BUNDLE_COUNT, bundle_bytes: BUNDLE_BYTES },
        ),
        ("path", vec![("index.html", PAGE), (bound.as_str(), b"")], BUNDLE_LIMITS),
    ] {
        temper::bundle::embedded(&files, limits).unwrap_or_else(|why| panic!("{name}: {why}"));
    }
}

#[test]
fn a_directory_that_is_no_bundle_refuses_the_start() {
    let rows: [Spoiled<'_>; 4] = [
        (
            "no-page",
            |directory| fs::remove_file(directory.join("bundle/index.html")).expect("page removed"),
            "temper serve: application.clients.bundle: bundle/index.html: absent\n",
        ),
        (
            "no-digest",
            |directory| fs::remove_dir_all(directory.join(DIGEST)).expect("digest removed"),
            "temper serve: application.clients.bundle: bundle: no digest directory\n",
        ),
        (
            "two-digests",
            |directory| fs::create_dir(directory.join("bundle/fedcba9876543210")).expect("second digest"),
            "temper serve: application.clients.bundle: bundle/fedcba9876543210: more than one digest directory\n",
        ),
        (
            "off-the-list",
            |directory| fs::write(directory.join(DIGEST).join("files/data.bin"), b"x").expect("odd file"),
            "temper serve: application.clients.bundle: bundle/0123456789abcdef/files/data.bin: file type is not on the closed list\n",
        ),
    ];
    for (name, spoil, expected) in rows {
        let scratch = support::Scratch::startup(&format!("bundle-{name}"));
        let directory = scratch.path();
        bundle_configuration(directory);
        fixture_bundle(directory);
        spoil(directory);
        assert_eq!(
            temper::prepare_in(&support::configuration::directories(directory)).err().expect("refused"),
            expected.replace(": bundle", &format!(": {}/bundle", directory.display())),
            "row {name}"
        );
        scratch.remove();
    }
}
#[test]
fn a_bundle_past_a_limit_refuses_the_start() {
    let rows: [Limited<'_>; 2] = [
        (
            "files",
            |value| value["application"]["limits"]["overrides"]["clients.bundle_files"] = 2.into(),
            "temper serve: application.clients.bundle: bundle/0123456789abcdef/files/robots.txt: past clients.bundle_files\n",
        ),
        (
            "bytes",
            |value| value["application"]["limits"]["overrides"]["clients.bundle_bytes"] = 61.into(),
            "temper serve: application.clients.bundle: bundle/0123456789abcdef/files/robots.txt: past clients.bundle_bytes\n",
        ),
    ];
    for (name, limit, expected) in rows {
        let scratch = support::Scratch::startup(&format!("bundle-limit-{name}"));
        let directory = scratch.path();
        bundle_configuration(directory);
        fixture_bundle(directory);
        support::owned::configuration_value(directory, limit);
        assert_eq!(
            temper::prepare_in(&support::configuration::directories(directory)).err().expect("refused"),
            expected.replace(": bundle", &format!(": {}/bundle", directory.display())),
            "row {name}"
        );
        scratch.remove();
    }
}

#[test]
fn programs_directories_are_checked_at_the_name_before_startup_effects() {
    for row in ["missing", "private", "link", "linked-parent", "mode", "file"] {
        let scratch = support::Scratch::startup(&format!("programs-{row}"));
        let directory = scratch.path();
        support::configuration(directory, &support::Forge::Unreached);
        let target = directory.join("programs");
        if row != "missing" {
            if row == "file" {
                fs::write(&target, b"not a directory").expect("file");
            } else {
                fs::create_dir(&target).expect("programs directory");
                fs::set_permissions(&target, fs::Permissions::from_mode(if row == "mode" { 0o750 } else { 0o700 }))
                    .expect("directory mode");
            }
        }
        let path = match row {
            "link" => {
                let link = directory.join("linked");
                symlink(&target, &link).expect("leaf link");
                link
            }
            "linked-parent" => {
                let parent = directory.join("parent");
                symlink(directory, &parent).expect("parent link");
                parent.join("programs")
            }
            _ => target.clone(),
        };
        let agent_path = directory.join("agent.json");
        let mut agent: serde_json::Value =
            serde_json::from_slice(&fs::read(&agent_path).expect("agent")).expect("JSON");
        agent["programs"] = serde_json::json!({"claude_code":{"path":"/nonexistent/claude", "directory":path}});
        fs::write(agent_path, serde_json::to_vec(&agent).expect("agent JSON")).expect("agent");
        // A later missing secret proves that the preflight neither creates nor inspects a login.
        fs::remove_file(directory.join("secrets/forge-1.api-token")).expect("absent peer");
        let refused = temper::prepare_in(&support::configuration::directories(directory)).err().expect("refused");
        if ["missing", "private", "linked-parent"].contains(&row) {
            assert!(refused.contains("application.forges[0].api_token"), "{row}: {refused}");
            assert!(!refused.contains("programs.claude_code.directory"), "{row}: {refused}");
        } else {
            let reason = match row {
                "link" => "symbolic link",
                "mode" => "nonprivate mode bits 0o50",
                "file" => "not a directory",
                _ => unreachable!(),
            };
            assert_eq!(
                refused,
                format!(
                    "temper serve: workers[0].agent.configuration: programs.claude_code.directory: {}: {reason}\n",
                    path.display()
                )
            );
        }
        assert!(!directory.join("worker-root").exists(), "{row}: no worker root");
        assert!(!target.exists() || row != "missing", "missing programs stay missing");
        scratch.remove();
    }
}

#[path = "../../../testing/memory_figures.rs"]
mod memory_figures;
// engine counts the root; system the process, routing queues, payloads and diagnostics.
// io counts kernel IO; store its machines; values the account credential table.
// workers and clients count their protocol machines and routing storage.
// clients.public/bundle/google/demo count origins, assets, sign-in and demo state.
// forge/held/settings/tokens/records count connections, retained calls, configuration,
// credentials and record handling; identities counts encoded identities and their keys.
// accounts counts its driver, configured providers and trust; setup/checked its host and inputs.
// worker counts the root, worker.remote/link its transports; host counts spawned processes.
// program_sources and worker_settings.programs count program state and retained paths;
// worker_settings counts routing queues, payloads, process tables and spawn commands.
// A worker's values, identities, io and worker_git count credentials, grants, IO and git.
fn memory_figure(
    name: &str,
    terms: &temper_service::startup::MemoryTerms,
    required: temper_service::startup::Requirements,
) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/figures/memory").join(name);
    memory_figures::check(&path.join("engine"), &terms.engine, required.engine_bytes);
    memory_figures::check(&path.join("worker"), &terms.worker, required.worker_bytes);
}

#[test]
fn example_memory_headroom_rounds_up_to_two_significant_digits() {
    for (required, declared) in [(0, 0), (90, 99), (91, 110), (100, 110), (101, 120), (999, 1100), (1001, 1200)] {
        assert_eq!(support::owned::memory_with_headroom(required), declared, "worst case {required}");
    }
}

#[test]
fn example_memory_band_includes_both_bounds_and_rejects_each_side() {
    for (required, declared, admitted) in [
        (100, 99, false),
        (100, 100, true),
        (100, 110, true),
        (100, 125, true),
        (100, 126, false),
        (101, 126, true),
        (101, 127, false),
        (u64::MAX, u64::MAX, true),
    ] {
        assert_eq!(
            support::owned::memory_in_band(declared, required),
            admitted,
            "{declared} for worst case {required}"
        );
    }
}

#[test]
fn owned_memory_figures_match_startup_requirements_at_bounded_strings() {
    let scratch = support::Scratch::startup("owned-memory-figures");
    let bounded = support::owned::bounded(scratch.path());
    let required = bounded.requirements();
    memory_figure("owned", &bounded.memory_terms(), required);
    support::owned::check_memory("../../docs/examples/owned.json", required);
    eprintln!("owned requirements: engine {}, worker {} bytes", required.engine_bytes, required.worker_bytes);
    check_bounded_startup(&bounded, required);
    scratch.remove();
}

fn check_bounded_startup(bounded: &support::owned::Bounded, required: temper_service::startup::Requirements) {
    let mut value: serde_json::Value = serde_json::from_slice(&bounded.document).expect("bounded JSON");
    value["engine"]["memory_bytes"] = required.engine_bytes.into();
    value["workers"][0]["memory_bytes"] = required.worker_bytes.into();
    let document = serde_json::to_vec(&value).expect("exact envelopes JSON");
    let started = bounded.startup(&document);
    assert!(
        started.is_ok(),
        "{}",
        String::from_utf8_lossy(&temper_service::startup::lines(&started.err().expect("startup refusal")))
    );
    for (engine, path) in [(true, "engine.memory_bytes"), (false, "workers[0].memory_bytes")] {
        let mut short = value.clone();
        if engine {
            short["engine"]["memory_bytes"] = (required.engine_bytes - 1).into();
        } else {
            short["workers"][0]["memory_bytes"] = (required.worker_bytes - 1).into();
        }
        let refused = bounded
            .startup(&serde_json::to_vec(&short).expect("one-byte-short JSON"))
            .err()
            .expect("refused before construction");
        assert_eq!(
            String::from_utf8_lossy(&temper_service::startup::lines(&refused)),
            format!("{path}: the worst case passes memory_bytes by 1 bytes\n")
        );
    }
}

#[test]
fn records_only_owned_memory_admits_checked_input_at_the_exact_bound() {
    let scratch = support::Scratch::startup("records-owned-memory");
    let bounded = support::owned::bounded_records(scratch.path());
    let required = bounded.requirements();
    let mut handovers = 0_u32;
    for (path, value) in &bounded.files.entries {
        if path.ends_with(b"/admin_token") {
            assert_eq!(path.len(), 4096 + b"/admin_token".len());
            assert_eq!(value.expose().len(), 4096);
            handovers += 1;
        }
    }
    assert_eq!(handovers, 1);
    // Checked paths, secrets and handover diagnostics are admitted at both bounds.
    let ordinary = support::owned::bounded(scratch.path());
    assert_eq!(required.worker_bytes, ordinary.requirements().worker_bytes);
    eprintln!(
        "records-only handover requirements: engine {}, worker {} bytes",
        required.engine_bytes, required.worker_bytes
    );
    check_bounded_startup(&bounded, required);
    scratch.remove();
}

#[test]
fn owned_memory_figures_for_init_match_its_own_document() {
    let scratch = support::Scratch::startup("init-memory-figures");
    let bounded = support::owned::bounded_template(scratch.path());
    let required = bounded.requirements();
    memory_figure("init", &bounded.memory_terms(), required);
    support::owned::check_memory("../temper-init/template/temper.json", required);
    eprintln!("init requirements: engine {}, worker {} bytes", required.engine_bytes, required.worker_bytes);
    check_bounded_startup(&bounded, required);
    scratch.remove();
}

#[test]
fn program_login_refuses_invalid_input_before_creation() {
    use support::program_login::{configuration, no_deployment_resources, refused};
    for row in ["no-program", "malformed", "missing-members", "oversized", "bad-agent", "bad-name"] {
        let scratch = support::Scratch::startup(&format!("login-{row}"));
        let directory = scratch.path();
        let programs = directory.join("programs");
        configuration(
            directory,
            (row != "no-program").then_some(programs.as_path()),
            std::path::Path::new("/nonexistent/claude"),
        );
        match row {
            "malformed" => fs::write(directory.join("temper.json"), b"{").expect("malformed JSON"),
            "missing-members" => fs::write(directory.join("temper.json"), b"{}").expect("missing members"),
            "oversized" => fs::File::create(directory.join("temper.json"))
                .expect("configuration")
                .set_len(1_048_577)
                .expect("past bound"),
            "bad-agent" => fs::write(directory.join("agent.json"), b"{}").expect("bad agent"),
            _ => {}
        }
        let why = refused(directory, if row == "bad-name" { "../escape" } else { "source" }.as_ref());
        assert!(why.starts_with("temper program-login: "), "{row}: {why}");
        assert_eq!(why.lines().count(), 1, "{row}: {why}");
        assert!(!why.contains("exec:"), "{row}: {why}");
        if row == "no-program" {
            assert!(why.contains("programs.claude_code: not configured"), "{why}");
        }
        if row == "oversized" {
            assert!(why.contains("past 1048576 bytes"), "{why}");
        }
        if row == "bad-name" {
            assert!(why.contains("source name must be"), "{why}");
        }
        assert!(!programs.exists());
        no_deployment_resources(directory);
        scratch.remove();
    }
}

#[test]
fn program_login_checks_both_directory_names_before_creating_anything() {
    use support::program_login::{configuration, no_deployment_resources, refused};
    for row in
        ["program-link", "program-mode", "program-file", "source-link", "source-mode", "source-file", "source-dangling"]
    {
        let scratch = support::Scratch::startup(&format!("login-{row}"));
        let directory = scratch.path();
        let programs = directory.join("programs");
        let source = programs.join("source");
        if row.starts_with("source-") {
            fs::create_dir(&programs).expect("programs");
            fs::set_permissions(&programs, fs::Permissions::from_mode(0o700)).expect("private");
        }
        let target = if row.starts_with("source-") { &source } else { &programs };
        let expected = if row.ends_with("link") || row.ends_with("dangling") {
            symlink(directory.join("missing"), target).expect("linked leaf");
            "symbolic link"
        } else if row.ends_with("mode") {
            fs::create_dir(target).expect("directory");
            fs::set_permissions(target, fs::Permissions::from_mode(0o750)).expect("unsafe mode");
            "nonprivate mode bits 0o50"
        } else {
            fs::write(target, b"file").expect("wrong kind");
            "not a directory"
        };
        configuration(directory, Some(&programs), std::path::Path::new("/nonexistent/claude"));
        let why = refused(directory, "source".as_ref());
        assert!(why.contains(expected), "{row}: {why}");
        assert!(why.contains(&target.display().to_string()), "{row}: {why}");
        assert!(!directory.join("missing").exists(), "linked peer stays absent");
        if row.starts_with("program-") {
            assert!(fs::symlink_metadata(&source).is_err(), "missing source stays absent");
        }
        no_deployment_resources(directory);
        scratch.remove();
    }
}

#[test]
fn program_login_prepares_private_directories_through_each_supported_parent() {
    use support::program_login::{configuration, no_deployment_resources, refused};
    for row in ["missing", "private", "linked-parent"] {
        let scratch = support::Scratch::startup(&format!("login-create-{row}"));
        let directory = scratch.path();
        let programs = directory.join("programs");
        if row == "private" {
            fs::create_dir(&programs).expect("programs");
            fs::set_permissions(&programs, fs::Permissions::from_mode(0o700)).expect("private");
        }
        let configured = if row == "linked-parent" {
            symlink(directory, directory.join("parent")).expect("linked parent");
            directory.join("parent/programs")
        } else {
            programs.clone()
        };
        configuration(directory, Some(&configured), std::path::Path::new("/nonexistent/claude"));
        let why = refused(directory, "source".as_ref());
        assert!(why.starts_with("temper program-login: exec: "), "{row}: {why}");
        assert_eq!(why.lines().count(), 1);
        for path in [&programs, &programs.join("source")] {
            assert_eq!(fs::metadata(path).expect("created directory").permissions().mode() & 0o777, 0o700);
        }
        no_deployment_resources(directory);
        scratch.remove();
    }
}

#[test]
fn init_prints_written_files_account_guidance_and_explicit_start_flags_on_stderr() {
    const NO_GIT: &str =
        "init: no git on PATH: a real forge's repositories need git 2.41 or later, set as workers[].git";
    for has_git in [true, false] {
        let scratch = support::Scratch::startup(&format!("init-command-{has_git}"));
        let bin = scratch.path().join("bin");
        fs::create_dir(&bin).expect("bin");
        let git = bin.join("git");
        if has_git {
            fs::write(&git, b"#!/bin/sh\nexit 0\n").expect("git");
            fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).expect("executable");
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_temper"))
            .arg("init")
            .args(support::configuration::flags(scratch.path()))
            .env_clear()
            .env("PATH", &bin)
            .output()
            .expect("init command");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stdout.is_empty());
        let lines = String::from_utf8(output.stderr).expect("init lines");
        let mut expected = String::new();
        for name in [
            "worker.secret",
            "worker.digest",
            "demo/llm.key",
            "demo/bin/git",
            "charters/chat.txt",
            "charters/coding.txt",
            "agent.json",
            "temper.json",
        ] {
            writeln!(expected, "init: wrote {}", scratch.path().join(name).display()).expect("string");
        }
        assert!(lines.starts_with(&expected), "files are reported in write order");
        assert!(
            lines.contains(
                "init: after starting, continue with ChatGPT or add an OpenAI API key from the accounts page\n"
            )
        );
        let path = scratch.path().display();
        assert!(
            lines.ends_with(&format!("init: start it with temper box --config {path} --data {path} --cache {path}\n"))
        );

        let diagnostics: Vec<_> = lines.lines().filter(|line| line.starts_with("init: no git")).collect();
        assert_eq!(diagnostics, if has_git { vec![] } else { vec![NO_GIT] });
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(scratch.path().join("temper.json")).expect("configuration"))
                .expect("JSON");
        assert_eq!(document["application"]["workers"][0]["git"].is_null(), !has_git);
        scratch.remove();
    }
}

#[test]
fn box_initializes_only_absent_or_empty_configuration_directories() {
    for row in ["absent", "empty", "nonempty"] {
        let scratch = support::Scratch::startup(&format!("box-directory-{row}"));
        let config = scratch.path().join("config");
        let data = scratch.path().join("data");
        let cache = scratch.path().join("cache");
        let arguments: Vec<Box<[u8]>> = [
            b"--config".as_slice(),
            config.as_os_str().as_encoded_bytes(),
            b"--data",
            data.as_os_str().as_encoded_bytes(),
            b"--cache",
            cache.as_os_str().as_encoded_bytes(),
        ]
        .iter()
        .map(|v| Box::from(*v))
        .collect();
        let (dirs, _) = temper::directories::resolve(&arguments, &[], b"/").expect("scratch directories");
        if row != "absent" {
            fs::create_dir(&config).expect("configuration");
        }
        if row == "nonempty" {
            fs::write(config.join("other"), b"kept").expect("entry");
        }
        let result = temper::box_command::configuration(&dirs);
        if row == "nonempty" {
            assert_eq!(
                result.expect_err("missing document"),
                format!("configuration {}: absent; temper init writes one", dirs.document().display())
            );
            assert_eq!(fs::read(config.join("other")).expect("kept"), b"kept");
            assert!(!data.exists());
            assert!(!cache.exists());
        } else {
            let (first, held) = result.expect("initialization");
            assert_eq!(held.len(), 5, "init retains its five listeners");
            assert!(data.join("worker.secret").exists());
            assert!(cache.is_dir());
            let (second, reopened) = temper::box_command::configuration(&dirs).expect("second read");
            assert_eq!(second, first, "existing configuration is kept");
            assert!(reopened.is_empty(), "reading an existing configuration reserves no listeners");
            for (fd, _) in held {
                skein_shell::close_keeper_fd(fd);
            }
        }
        scratch.remove();
    }
}

#[test]
fn owned_memory_figures_for_a_worker_alone_include_its_process_and_no_engine() {
    let scratch = support::Scratch::startup("worker-alone-memory-figures");
    let bounded = support::owned::bounded(scratch.path());
    let mut document: serde_json::Value = serde_json::from_slice(&bounded.document).expect("bounded document");
    document.as_object_mut().expect("worker-alone fixture").remove("engine");
    document["workers"][0]["engine"] = serde_json::json!({"socket":"127.0.0.1:7100"});
    document["application"] =
        serde_json::json!({"workers":document["application"]["workers"], "limits":document["application"]["limits"]});
    let input = serde_json::to_vec(&document).expect("worker-alone fixture");
    let required = temper_service::startup::memory_requirements(
        &input,
        &jig_protocol_config::Directories::working(),
        &bounded.files,
        &bounded.agent,
        &bounded.seeds,
        None,
        skein_lib::List::with_capacity(0),
        None,
    )
    .expect("worker-alone fixture");
    assert_eq!(required.engine_bytes, 0);
    // A standalone worker also counts its process queues, kernel tables and IO.
    let terms = temper_service::startup::memory_terms(
        &input,
        &jig_protocol_config::Directories::working(),
        &bounded.files,
        &bounded.agent,
        &bounded.seeds,
        None,
        skein_lib::List::with_capacity(0),
        None,
    )
    .expect("worker-alone terms");
    assert!(terms.engine.is_empty());
    memory_figures::check(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/figures/memory/worker-alone/worker"),
        &terms.worker,
        required.worker_bytes,
    );
    document["workers"][0]["memory_bytes"] = required.worker_bytes.into();
    let part = temper_service::startup::worker_part(
        &serde_json::to_vec(&document).expect("worker-alone fixture"),
        &jig_protocol_config::Directories::working(),
    )
    .expect("worker-alone fixture");
    let service = temper_service::startup::worker_alone(
        part,
        &bounded.files,
        &bounded.agent,
        temper_service::WorkerRuntime { root: skein_io::kernel::Fd::new(9), path: b"/worker".as_slice().into() },
        7,
    )
    .expect("worker-alone fixture");
    assert_eq!(service.worst_case(), required.worker_bytes);
    assert!(service.setup_page(1, b"state", skein_lib::Time::ZERO).is_none());
    scratch.remove();
}

#[test]
fn serve_box_refuses_closed_or_non_pipe_descriptors_before_starting() {
    for inherited in [false, true] {
        for (redirections, offset) in [("{r}>&- {a}>&-", 0), ("{r}>/dev/null {a}>&-", 0), ("{r}>&1 {a}>&-", 1)] {
            let requests = if inherited { 4 } else { 3 };
            let answers = requests + 1;
            let redirections = redirections.replace("{r}", &requests.to_string()).replace("{a}", &answers.to_string());
            let script = format!(
                "exec \"$1\" serve --config /unused --data /unused --cache /unused --box {} {redirections}",
                if inherited { "3<&0" } else { "" }
            );
            let mut command = if inherited {
                listeners::command(std::net::TcpListener::bind("127.0.0.1:0").expect("listener"), &script)
            } else {
                let mut command = std::process::Command::new("/bin/sh");
                command
                    .args(["-c", &script, "serve-box-test", env!("CARGO_BIN_EXE_temper")])
                    .env_clear()
                    .stdin(std::process::Stdio::piped());
                command
            };
            let output = command.output().expect("serve child");
            let fd = requests + offset;
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert_eq!(
                String::from_utf8(output.stderr).expect("UTF-8 diagnostic"),
                format!("temper serve: --box: fd {fd} is not an open pipe\n")
            );
        }
    }
}

#[test]
fn serve_claims_its_clients_listener_and_box_pipes_follow_it() {
    for boxed in [false, true] {
        let scratch = support::Scratch::startup(if boxed { "inherited-boxed" } else { "inherited-serve" });
        support::configuration(scratch.path(), &support::Forge::Unreached);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("held listener");
        let address = listener.local_addr().expect("address");
        let path = scratch.path().join("temper.json");
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).expect("config")).expect("JSON");
        value["application"]["clients"]["listener"] = serde_json::json!({"socket":address.to_string()});
        support::write_private(&path, &serde_json::to_vec(&value).expect("JSON"));
        let script = if boxed { "exec \"$@\" 3<&0 4>&1 5<&2 0</dev/null 2>&1" } else { "exec \"$@\" 3<&0 0</dev/null" };
        let mut command = listeners::command(listener, script);
        command.arg("serve").args(support::configuration::flags(scratch.path()));
        let (answers, keep_answers_open) = std::io::pipe().expect("box answers");
        if boxed {
            command.arg("--box").stderr(std::process::Stdio::from(answers));
        }
        let ended = listeners::probe(command, address, "secret=");
        drop(keep_answers_open);
        assert!(ended.ready.is_some(), "serve ready: {:?} {:?}", ended.stdout, ended.stderr);
        assert!(ended.reply.expect("clients answer").starts_with(b"HTTP/1.1 "));
        assert!(ended.status.success(), "serve drains: {:?} {:?}", ended.stdout, ended.stderr);
        scratch.remove();
    }
}

#[test]
fn a_legacy_asked_owned_forges_absent_files_are_left_out_but_existing_files_stay_private() {
    for public in [false, true] {
        let scratch = owned_example(if public { "asked-public-file" } else { "asked-absent-files" });
        let config = scratch.path().join("temper.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&config).expect("configuration file")).expect("configuration JSON");
        // temper-15 removes the old-form file checks; the shipped form names none.
        support::owned::old_form(&mut value["application"]);
        value["application"]["setup"][0]["start"] = "asked".into();
        support::write_private(&config, &serde_json::to_vec(&value).expect("configuration JSON"));
        if public {
            fs::create_dir(scratch.path().join("forge-1")).expect("forge directory");
            support::write_private(&scratch.path().join("forge-1/api_token"), b"api-token");
            fs::set_permissions(scratch.path().join("forge-1/api_token"), fs::Permissions::from_mode(0o644))
                .expect("public token permissions");
        }
        let prepared = temper::prepare_in(&support::configuration::directories(scratch.path()));
        if public {
            assert_eq!(
                prepared.err().expect("unsafe asked file refused"),
                format!(
                    "temper serve: application.forges[0].api_token: {}/forge-1/api_token: readable by others or reached through a link\n",
                    scratch.path().display()
                )
            );
        } else {
            assert!(prepared.is_ok(), "absent asked files are allowed: {:?}", prepared.err());
        }
        scratch.remove();
    }
}

#[test]
fn records_only_handover_reads_are_private_optional_bounded_and_only_read_admin_token() {
    for row in ["absent", "exact", "over", "newline", "public", "link", "parent-link", "directory", "unreadable"] {
        let scratch = owned_example(&format!("handover-{row}"));
        let directory = scratch.path();
        let forge = directory.join("forge-1");
        fs::create_dir(&forge).expect("forge directory");
        fs::set_permissions(&forge, fs::Permissions::from_mode(0o700)).expect("handover fixture");
        let token = forge.join("admin_token");
        // Every other old file is deliberately unsafe and must stay unread.
        for name in ["api_token", "user", "git_token", "webhook_secret"] {
            fs::write(forge.join(name), b"unused").expect("handover fixture");
            fs::set_permissions(forge.join(name), fs::Permissions::from_mode(0o644)).expect("handover fixture");
        }
        match row {
            "absent" => {}
            "directory" => fs::create_dir(&token).expect("handover fixture"),
            "link" => symlink(forge.join("api_token"), &token).expect("handover fixture"),
            "parent-link" => {
                fs::rename(&forge, directory.join("actual-forge")).expect("handover fixture");
                symlink(directory.join("actual-forge"), &forge).expect("handover fixture");
                support::write_private(&token, b"test-admin");
            }
            "exact" | "over" => support::write_private(&token, &vec![b'x'; if row == "exact" { 4096 } else { 4097 }]),
            "newline" => support::write_private(&token, b"test-admin\n"),
            "public" | "unreadable" => {
                support::write_private(&token, b"test-admin");
                fs::set_permissions(&token, fs::Permissions::from_mode(if row == "public" { 0o644 } else { 0o000 }))
                    .expect("handover fixture");
            }
            _ => unreachable!(),
        }
        let answer = temper::prepare_in(&support::configuration::directories(directory));
        if matches!(row, "absent" | "exact" | "newline") || (row == "unreadable" && skein_shell::effective_user() == 0)
        {
            assert!(answer.is_ok(), "{row}: bounded private handover accepted");
        } else {
            let why = answer.err().expect("unsafe handover refuses startup");
            assert!(why.contains(&token.display().to_string()), "names the file");
            assert!(why.contains("the handover is unreadable"));
            assert!(!why.contains("test-admin"), "no handover in diagnostic");
        }
        if row == "unreadable" {
            fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).expect("handover fixture");
        }
        scratch.remove();
    }
}
#[test]
fn old_form_owned_does_not_read_a_handover() {
    let scratch = owned_example("old-form-no-handover");
    let forge = scratch.path().join("forge-1");
    fs::create_dir(&forge).expect("handover fixture");
    fs::create_dir(forge.join("admin_token")).expect("handover fixture");
    // Asked old-form entries permit absent legacy files.
    let config = scratch.path().join("temper.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&config).expect("handover fixture")).expect("handover fixture");
    support::owned::old_form(&mut value["application"]);
    value["application"]["setup"][0]["start"] = "asked".into();
    support::write_private(&config, &serde_json::to_vec(&value).expect("handover fixture"));
    assert!(temper::prepare_in(&support::configuration::directories(scratch.path())).is_ok());
    scratch.remove();
}

#[test]
fn tls_is_refused_before_reading_the_handover() {
    let scratch = owned_example("tls-before-handover");
    fs::create_dir(scratch.path().join("forge-1")).expect("handover fixture");
    fs::create_dir(scratch.path().join("forge-1/admin_token")).expect("handover fixture");
    let config = scratch.path().join("temper.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&config).expect("handover fixture")).expect("handover fixture");
    value["application"]["forges"][0]["transport"] =
        serde_json::json!({"tls":{"server_name":"localhost", "trust":null}});
    support::write_private(&config, &serde_json::to_vec(&value).expect("handover fixture"));
    let why = temper::prepare_in(&support::configuration::directories(scratch.path())).err().expect("TLS refused");
    assert!(!why.contains("the handover is unreadable"));
    assert!(scratch.path().join("forge-1/admin_token").is_dir());
    scratch.remove();
}

#[test]
fn the_store_page_holds_the_templates_largest_turn_frame() {
    let scratch = support::Scratch::startup("turn-store-page");
    let bounded = support::owned::bounded_template(scratch.path());
    let store = temper_service::startup::store_limits(&bounded.document, &jig_protocol_config::Directories::working())
        .expect("template store limits");
    let turn = u64::from(bounded.agent.limits.bodies.turn_body);
    eprintln!("template T={turn}, kv.page.bytes={}, T+64={}", store.store.store.page.bytes, turn + 64);
    assert!(u64::from(store.store.store.page.bytes) >= turn + 64);
    scratch.remove();
}

/// Hold only the limit maps this derivation owns; keep the document's other text.
fn turn_bound_document(path: &std::path::Path, turn: u32, escaped: bool) {
    use support::configuration::turn_bounds;
    let text = fs::read_to_string(path).expect("turn-bound document");
    let plain = if escaped { text.replace("{{", "{").replace("}}", "}") } else { text.clone() };
    let before: serde_json::Value = serde_json::from_str(&plain).expect("document JSON");
    let mut after = before.clone();
    turn_bounds::apply(&mut after, turn);
    let mut maps = Vec::new();
    for pointer in ["/engine/limits/overrides", "/application/limits/overrides"] {
        if let Some(previous) = before.pointer(pointer) {
            maps.push((previous, after.pointer(pointer).expect("same map")));
        }
    }
    if let Some(workers) = before["workers"].as_array() {
        for (index, worker) in workers.iter().enumerate() {
            maps.push((&worker["limits"]["overrides"], &after["workers"][index]["limits"]["overrides"]));
        }
    }
    let write = std::env::var_os("SKEIN_CODEGEN_WRITE").is_some_and(|v| v == "1");
    if !write {
        assert_eq!(
            before,
            after,
            "{}: run SKEIN_CODEGEN_WRITE=1 heavy cargo nextest run -p temper --test startup -E 'test(turn_bounds)'",
            path.display()
        );
        return;
    }
    let mut result = String::new();
    let mut rest = plain.as_str();
    while let Some(at) = rest.find("\"overrides\"") {
        let start = at + rest[at..].find('{').expect("override map");
        let end = start + rest[start..].find('}').expect("flat numeric overrides") + 1;
        let previous: serde_json::Value = serde_json::from_str(&rest[start..end]).expect("override JSON");
        let (_, next) = maps.iter().find(|(old, _)| **old == previous).expect("known override map");
        result.push_str(&rest[..start]);
        let indent = rest[..at].rsplit_once('\n').map_or("", |(_, line)| line);
        let indent = &indent[..indent.len() - indent.trim_start().len()];
        let map = serde_json::to_string_pretty(next).expect("limit map JSON");
        result.push_str(&map.replace('\n', &format!("\n{indent}")));
        rest = &rest[end..];
    }
    result.push_str(rest);
    if escaped {
        // Only JSON object braces are doubled in the lab's format; interpolation
        // braces inside quoted strings remain the lab reader's expressions.
        let mut in_string = false;
        let mut slash = false;
        result = result.chars().fold(String::new(), |mut out, c| {
            if !in_string && matches!(c, '{' | '}') {
                out.push(c);
            }
            out.push(c);
            if c == '"' && !slash {
                in_string = !in_string;
            }
            slash = c == '\\' && !slash;
            out
        });
    }
    fs::write(path, result).expect("write derived turn bounds");
}

#[test]
fn turn_bounds() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (deployment, agent) in [
        ("crates/temper-init/template/temper.json", "crates/temper-init/template/agent.json"),
        ("docs/examples/owned.json", "docs/examples/owned/agent.json"),
    ] {
        let agent: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(agent)).expect("agent document")).expect("agent JSON");
        let turn = support::configuration::turn_bounds::turn_body(&agent);
        turn_bound_document(&root.join(deployment), turn, false);
        eprintln!("{deployment}: T={turn}");
    }
    for lab in ["cluster", "cluster-web", "look-live", "chat-change-smoke", "chat-change", "chat-change-killed"] {
        let directory = root.join("lab").join(lab);
        let agent = fs::read_to_string(directory.join("agent.json")).expect("lab agent");
        let agent: serde_json::Value =
            serde_json::from_str(&agent.replace("{{", "{").replace("}}", "}")).expect("lab agent JSON");
        let turn = support::configuration::turn_bounds::turn_body(&agent);
        for name in ["engine.json", "w1.json", "w2.json"] {
            turn_bound_document(&directory.join(name), turn, true);
        }
        eprintln!("lab/{lab}: T={turn}");
    }
    let github = root.join("crates/temper/tests/support/github");
    let agent = fs::read_to_string(github.join("agent.json")).expect("GitHub agent");
    let agent: serde_json::Value =
        serde_json::from_str(&agent.replace("{{", "{").replace("}}", "}")).expect("GitHub agent JSON");
    let turn = support::configuration::turn_bounds::turn_body(&agent);
    for name in ["engine.json", "w1.json"] {
        turn_bound_document(&github.join(name), turn, true);
    }
}

#[test]
fn the_template_agent_refuses_jigs_small_hub_turn_bound_before_serving() {
    let scratch = support::Scratch::startup("small-hub-turn");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("init")
        .args(support::configuration::flags(scratch.path()))
        .output()
        .expect("init");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let path = scratch.path().join("temper.json");
    let mut document: serde_json::Value = serde_json::from_slice(&fs::read(&path).expect("deployment")).expect("JSON");
    let hub = jig_protocol_config::profile(jig_protocol_config::Profile::Small).hub;
    let limits = &mut document["engine"]["limits"]["overrides"];
    limits["hub.turn_bytes"] = hub.turn_bytes.into();
    limits["hub.transcript_bytes"] = hub.transcript_bytes.into();
    limits["hub.turn_queue_bytes"] = hub.turn_queue_bytes.into();
    fs::write(&path, serde_json::to_vec(&document).expect("JSON")).expect("deployment");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("serve")
        .args(support::configuration::flags(scratch.path()))
        .output()
        .expect("serve");
    let agent: serde_json::Value =
        serde_json::from_slice(&fs::read(scratch.path().join("agent.json")).expect("agent")).expect("agent JSON");
    let turn = support::configuration::turn_bounds::turn_body(&agent);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).lines().find(|line| line.starts_with("temper serve:")).expect("startup problem").starts_with(&format!(
        "temper serve: engine.limits.overrides.hub.turn_bytes: 65536 bytes, short of the agent's largest turn, {turn} bytes"
    )), "{}", String::from_utf8_lossy(&output.stderr));
    scratch.remove();
}

#[test]
fn forge_run_refuses_a_listener_at_the_wrong_address_before_taking_the_lock() {
    let scratch = support::Scratch::startup("forge-wrong-listener");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let found = listener.local_addr().expect("address");
    support::owned::configuration(
        scratch.path(),
        &support::owned::Names {
            forgejo: std::path::Path::new("/not-run/forgejo"),
            runner: None,
            port: 9,
            bot: "temper",
            person: "ada",
        },
    );
    let mut command = listeners::command(listener, "exec \"$@\" 3<&0 0</dev/null");
    command.args(["forge", "run"]).args(support::configuration::flags(scratch.path())).args(["1", "forgejo"]);
    let ended = command.output().expect("run");
    assert_eq!(ended.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(ended.stderr).expect("diagnostic"),
        format!("temper forge run: forge 1: fd 3 listens at {found}, expected owned.listen 127.0.0.1:9\n")
    );
    assert!(!scratch.path().join("forge-1/forgejo.lock").exists());
    scratch.remove();
}

#[test]
fn forge_run_keeps_fd_three_through_the_version_spawn_and_exec() {
    use skein_forge::forgejo::pin::VERSION;
    let scratch = support::Scratch::startup("forge-listener-exec");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let binary = scratch.path().join("forgejo");
    fs::write(
        &binary,
        format!(
            r#"#!/bin/sh
if [ "$1" = --version ]; then
    test -z "${{LISTEN_FDS+x}}" || exit 20
    test ! -S /proc/self/fd/3 || exit 21
    printf 'forgejo version {VERSION}\n'
    exit 0
fi
test "$LISTEN_FDS" = 1 || exit 22
test -S /proc/self/fd/3 || exit 23
printf 'inherited fd 3 after exec\n'
"#
        ),
    )
    .expect("fake Forgejo");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("executable");
    support::owned::configuration(
        scratch.path(),
        &support::owned::Names { forgejo: &binary, runner: None, port: address.port(), bot: "temper", person: "ada" },
    );
    support::owned::finished(scratch.path());
    let state_path = scratch.path().join("forge-1/state");
    let mut state = temper_forge_setup::state::decode(&fs::read(&state_path).expect("state")).expect("prepared");
    state.runner = None;
    support::write_private(&state_path, temper_forge_setup::state::encode(&state).expose());
    let mut command = listeners::command(listener, "exec \"$@\" 3<&0 0</dev/null");
    command.args(["forge", "run"]).args(support::configuration::flags(scratch.path())).args(["1", "forgejo"]);
    let ended = command.output().expect("run");
    assert!(ended.status.success(), "{} {:?}", ended.status, String::from_utf8_lossy(&ended.stderr));
    assert_eq!(ended.stdout, b"inherited fd 3 after exec\n");
    scratch.remove();
}

#[cfg(target_os = "linux")]
#[test]
fn forge_parent_listener_helper() {
    if std::env::var_os("TEMPER_FORGE_PARENT_HELPER").is_some() {
        support::forge_parent::serve();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn forge_run_ignores_its_childs_term_relays_the_listener_and_dies_with_its_status() {
    forge_parent_shutdown(false);
}

#[cfg(target_os = "linux")]
#[test]
fn killing_forge_run_alone_kills_its_forgejo_child() {
    forge_parent_shutdown(true);
}

#[cfg(target_os = "linux")]
fn forge_parent_shutdown(kill: bool) {
    use support::parent::{Guard, live, signal, wait};
    let scratch = support::Scratch::startup(&format!("forge-parent-{kill}"));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let binary = scratch.path().join("forgejo");
    let pid_file = scratch.path().join("fake.pid");
    let test = std::env::current_exe().expect("startup binary");
    fs::write(
        &binary,
        format!(
            r#"#!/bin/sh
if [ "$1" = --version ]; then
    printf 'forgejo version {}\n'
    exit 0
fi
test "$LISTEN_FDS" = 1 || exit 22
test -S /proc/self/fd/3 || exit 23
printf '%s' "$$" > '{}'
export TEMPER_FORGE_PARENT_HELPER=1
'{}' --exact forge_parent_listener_helper --nocapture &
helper=$!
trap 'kill -TERM "$helper"; wait "$helper"; exit 37' TERM
kill -TERM "$PPID"
wait "$helper"
exit 38
"#,
            skein_forge::forgejo::pin::VERSION,
            pid_file.display(),
            test.display()
        ),
    )
    .expect("fake Forgejo");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("executable");
    support::owned::configuration(
        scratch.path(),
        &support::owned::Names { forgejo: &binary, runner: None, port: address.port(), bot: "temper", person: "ada" },
    );
    support::owned::finished(scratch.path());
    let state_path = scratch.path().join("forge-1/state");
    let mut state = temper_forge_setup::state::decode(&fs::read(&state_path).expect("state")).expect("prepared");
    state.runner = None;
    support::write_private(&state_path, temper_forge_setup::state::encode(&state).expose());
    let mut command = listeners::command(listener, "exec \"$@\" 3<&0 0</dev/null");
    command.args(["forge", "run"]).args(support::configuration::flags(scratch.path())).args(["1", "forgejo"]);
    let mut guard = Guard::new(command.spawn().expect("forge run"));
    wait(|| pid_file.exists());
    let fake = fs::read_to_string(&pid_file).expect("fake pid").parse().expect("pid");
    guard.pids.push(fake);
    wait(|| support::boxed::status(address, "/") == Some(200));
    support::parent::descendants(fake, &mut guard.pids);
    let parent = guard.child.as_mut().expect("parent");
    assert!(parent.try_wait().expect("status").is_none(), "Forgejo's SIGTERM leaves forge run running");
    signal(parent.id(), if kill { "KILL" } else { "TERM" });
    wait(|| guard.child.as_mut().expect("parent").try_wait().expect("exit").is_some());
    if kill {
        // Kill reaches Forgejo's own process at once; the fake's helper is the
        // script's child, which real Forgejo has none of, so the guard ends it.
        wait(|| !live(fake));
    } else {
        wait(|| guard.pids.iter().all(|pid| !live(*pid)));
    }
    let ended = guard.child.take().expect("parent").wait_with_output().expect("reaped");
    if kill {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(ended.status.signal(), Some(9));
    } else {
        assert_eq!(ended.status.code(), Some(37), "{}", String::from_utf8_lossy(&ended.stderr));
        assert!(String::from_utf8_lossy(&ended.stdout).contains("fake connected"));
        assert!(String::from_utf8_lossy(&ended.stderr).contains("fake ready on fd 3"));
        guard.pids.clear();
    }
    scratch.remove();
}

#[test]
fn forge_run_refuses_other_listener_counts_and_any_for_the_runner_before_opening_files() {
    for (which, count) in [("forgejo", "0"), ("forgejo", "2"), ("forgejo", "many"), ("runner", "1"), ("runner", "0")] {
        let ended = std::process::Command::new(env!("CARGO_BIN_EXE_temper"))
            .args(["forge", "run", "--config", "/no-document", "--data", "/unused", "--cache", "/unused", "1", which])
            .env_clear()
            .env("LISTEN_FDS", count)
            .output()
            .expect("run");
        assert_eq!(ended.status.code(), Some(1));
        let diagnostic = String::from_utf8(ended.stderr).expect("diagnostic");
        assert!(diagnostic.contains("LISTEN_FDS") && diagnostic.contains("fd 3"), "{diagnostic}");
        assert!(!diagnostic.contains("no-document"), "refuses before document read");
    }
}

#[test]
fn listener_refusals_name_the_fd_before_serve_or_a_fake_opens_files() {
    for args in [
        &["serve", "--config", "/no-document", "--data", "/unused", "--cache", "/unused"][..],
        &["fake", "llm", "--document", "/no-document"],
        &["fake", "forge", "--document", "/no-document"],
    ] {
        let ended = std::process::Command::new("/bin/sh")
            .args(["-c", "exec \"$@\" 3>/dev/null", "bad-listener", env!("CARGO_BIN_EXE_temper")])
            .args(args)
            .env_clear()
            .env("LISTEN_FDS", "1")
            .output()
            .expect("run");
        assert_eq!(ended.status.code(), Some(1));
        let diagnostic = String::from_utf8(ended.stderr).expect("diagnostic");
        assert!(diagnostic.contains("fd 3 is not a socket"), "{diagnostic}");
    }
}

#[test]
fn forge_child_refuses_a_wrong_parent_or_missing_program_without_reading_a_document() {
    for (parent, program) in [(0, "/bin/true"), (std::process::id(), "/no-such-forge-child-program")] {
        let ended = std::process::Command::new(env!("CARGO_BIN_EXE_temper"))
            .args(["forge", "child", "--parent", &parent.to_string(), "--signal", "kill", "--", program])
            .env_clear()
            .output()
            .expect("child refusal");
        assert_eq!(ended.status.code(), Some(1));
        assert!(ended.stdout.is_empty());
        let why = String::from_utf8(ended.stderr).expect("diagnostic");
        assert!(why.starts_with("temper forge child: "), "{why}");
        assert_eq!(why.lines().count(), 1, "{why}");
        assert!(!why.contains("document"), "{why}");
    }
}
