//! Shared landing configuration and scripted turns (domain/testing.md, 2.4).
use serde_json::{Value, json};
use std::net::SocketAddr;

pub mod kept;

pub const EDIT: &[u8] = b"Changed by temper.\n";
pub const README: &[u8] = b"# app\n";
// A created member owns all ten grants, their segment boxes and executor kinds.
const MEMBER_BYTES: u32 = 2048;

#[must_use]
pub fn fixture() -> crate::forge::Fixture {
    crate::forge::Fixture {
        setup: || {
            use skein_fake_forge_domain::api;
            let mut setup = (crate::forge::fixture().setup)();
            setup.tree.iter_mut().find(|file| file.path.as_ref() == b"README.md").expect("README").content =
                README.into();
            setup.checks.cue = Some(api::Cue::File { path: b"README.md".as_slice().into(), green: EDIT.into() });
            setup.protection = Some(api::Protection {
                branch: b"main".as_slice().into(),
                contexts: setup.checks.contexts.clone(),
                approvals: 0,
                dismiss_stale: false,
                queue: false,
                linear: false,
                code_owners: false,
                bypassing_users: Box::new([]),
            });
            setup
        },
        configure: |value, agent, forge| {
            let limits = &mut value["application"]["limits"]["overrides"];
            // Shipped: poll 5 s, poll_max 30 s, slow 40 s, hinted 1 s.
            for (name, millis) in [("poll_ms", 10), ("poll_max_ms", 50), ("slow_ms", 100), ("hinted_ms", 10)] {
                limits[format!("forge.client.{name}")] = millis.into();
            }
            // Chat, repository polls and one fresh change chain share the client.
            limits["forge.client.pending"] = 8.into();
            limits["forge.client.calls"] = 4.into();
            limits["forge_component.connections"] = 4.into();
            // The tools request their shipped 32-row tree window; the base world allows two.
            value["engine"]["limits"]["overrides"]["documents.rows"] = temper_client_tools::limits::ROWS.into();
            // The sample profile allows 100 requests per 10 s; short polls need a shorter window.
            value["application"]["limits"]["overrides"]["forge.client.window_ms"] = 100.into();
            value["application"]["landing"]["projects"] = json!([{"project":1,"rules":[{
                "connector":1,"kind":4,"pattern":{"segments":["forge",forge.to_string(),"acme","app","branch"],"last":"exact","value":"main"},
                "enforce":true,"ci":true,"up_to_date":true,
                "gates":[{"number":1,"blocking":true,"freshness":"exact"}],"approvals":[]
            }]}]);
            configure(value, agent, forge);
        },
    }
}

pub fn configure(value: &mut Value, agent: &mut Value, forge: SocketAddr) {
    let grant = |kind| json!({"connector":1,"kind":kind,"pattern":{"segments":["forge",forge.to_string()],"last":"open","value":"acme"}});
    let grants =
        json!([grant(1), grant(2), grant(3), grant(4), grant(7), grant(8), grant(10), grant(11), grant(12), grant(13)]);
    let authority = json!({"tools":1,"grants":grants,"delegation":{"kinds":[{"kind":"charter","number":1},{"kind":"procedure","number":2}],"tasks":8,"depth":4},"budget":{"spend":100,"deadline_unix_seconds":null},"notes":0,"note_resources":[]});
    value["engine"]["rules"]["ceiling"] = authority.clone();
    value["engine"]["rules"]["maximum_run_spend"] = 10.into();
    value["engine"]["projects"][0]["policy"]["ceiling"] = authority.clone();
    value["engine"]["projects"][0]["policy"]["roles"][0]["authority"] = authority;
    value["engine"]["projects"][0]["policy"]["roles"][0]["authority"]["budget"]["spend"] = 40.into();
    value["engine"]["projects"][0]["policy"]["projections"] = grants;
    // A deployment project zero, which the binary's fixture clones from project one, keeps that
    // mirror: its policy too must sit under the rules ceiling set above.
    let policy = value["engine"]["projects"][0]["policy"].clone();
    for project in value["engine"]["projects"].as_array_mut().expect("projects").iter_mut().skip(1) {
        project["policy"] = policy.clone();
    }
    // forge-setup's new project is made from policy 1, which must sit under the ceiling too.
    if let Some(policies) = value["engine"].get_mut("policies").and_then(Value::as_array_mut) {
        for configured in policies {
            configured["policy"] = policy.clone();
        }
    }
    let limits = value["engine"]["limits"]["overrides"].as_object_mut().expect("engine overrides");
    for (name, size) in [
        ("people.operations", 5),
        ("tasks.tasks", 8),
        ("documents.lines", 2),
        ("documents.chips", 2),
        ("tasks.project_tasks", 8),
        ("tasks.tree_tasks", 8),
        ("tasks.charters", 1),
        ("tasks.message_bytes", 4094),
        ("people.words", 512),
        ("tasks.authority_bytes", 512),
        ("core.call_answer_bytes", 2048),
        ("tasks.spec_bytes", 512),
        ("tasks.result_bytes", 128),
        ("tasks.delegates", 4),
        ("tasks.parameters", 5),
        ("tasks.contract_choices", 2),
        ("tasks.authority_grants", 12),
        ("tasks.authority_segments", 9),
        ("tasks.hold_segments", 8),
        ("tasks.saved_resources", 4),
        ("authority.grants", 12),
        ("authority.segments", 8),
        ("authority.roles", 1),
        // CI, up-to-date and the review gate each produce one requirement.
        ("authority.requirements", 3),
        ("notes.scopes", 15),
        ("brief.briefs", 2),
        ("brief.sections", 4),
        ("fleet.attempts", 2),
        ("people.inbox_tasks", 16),
        ("people.pending", 4),
        ("people.waiters", 4),
        ("people.amendment_bytes", MEMBER_BYTES),
        ("core.policy_bytes", 4096),
    ] {
        limits.insert(name.into(), size.into());
    }
    value["workers"][0]["slots"] = 1.into();
    let limits = &mut value["workers"][0]["limits"]["overrides"];
    limits["hub.slots"] = 1.into();
    limits["hub.event_bytes"] = 4094.into();
    limits["tasks.message_bytes"] = 4096.into();
    let charter = &mut value["engine"]["charters"][0];
    for flag in ["inspect", "modify", "agents"] {
        charter[flag] = true.into();
    }
    charter["turns"] = 8.into();
    charter["time_ms"] = 120_000.into();
    charter["reserve_time_ms"] = 60000.into();
    agent["profile"]["declared"]["tool_payload"] = 4096.into();
    agent["profile"]["policy"]["max_turns"] = 8.into();
    agent["profile"]["policy"]["check_timeout"] = 1000.into();
    agent["endpoints"][0]["models"][0]["output"] = 8192.into();
    admit(value);
}

pub(crate) fn admit(value: &Value) {
    let bytes = serde_json::to_vec(&value["engine"]).expect("engine JSON");
    let document = jig_protocol_config::document(
        &bytes,
        &jig_protocol_config::DocumentLimits { bytes: 1_048_576, tokens: 65_536, depth: 32 },
    )
    .expect("engine document");
    // Parsing a fixture without a deployment; only its limits are used.
    let jig = jig_protocol_config::engine(&document, 0, &jig_protocol_config::Directories::working())
        .expect("engine configuration");
    let mut own = temper_service::limits::profile(temper_service::limits::Profile::Small);
    for (name, number) in value["application"]["limits"]["overrides"].as_object().expect("own overrides") {
        temper_service::limits::apply(&mut own, name.as_bytes(), number.as_u64().expect("limit number"))
            .expect("application override");
    }
    let root = temper_service::limits::engine(&jig.limits, &own).expect("engine root admits the fixture");
    jig_protocol_store::sizes::check(
        &jig.limits.core,
        &temper_protocol_store::application(&root.forge).expect("application sizes"),
        &temper_engine_domain::journal_limits(&root.journal),
        &jig.limits.store.store,
        jig.limits.store.orphan_erases,
    )
    .expect("store admits the fixture");
}

#[must_use]
pub fn scripts(forge: SocketAddr, repository: u64, readme: &[u8]) -> Box<[skein_fake_llm_domain::api::Script]> {
    let repository = u32::try_from(repository).expect("forge-local repository id fits u32");
    change_scripts(
        ChangeAuthority { coding_kind: 1, scope: Some((forge, "acme")) },
        temper_engine_domain_forge::tag(1, repository),
        readme,
        EDIT,
        b"@forge_chat",
        "@coding Fix",
    )
}

/// The coding charter and grant scope used by a landing exchange.
#[derive(Clone, Copy)]
pub struct ChangeAuthority<'a> {
    pub coding_kind: u64,
    /// None uses the first repository's deployment-wide forge grants.
    pub scope: Option<(SocketAddr, &'a str)>,
}

/// The landing exchange with a caller's cues, grant scope and README addition;
/// `repository` is the resource's packed tag (forge and repository).
#[must_use]
pub fn change_scripts(
    authority: ChangeAuthority<'_>,
    repository: u64,
    readme: &[u8],
    edit: &[u8],
    chat_cue: &[u8],
    coding_words: &str,
) -> Box<[skein_fake_llm_domain::api::Script]> {
    use skein_fake_llm_domain::api::{Finish, Line, Script, Turn};
    let call = |name: &[u8], value: Value| Turn {
        lines: Box::new([Line::Call {
            name: name.into(),
            arguments: serde_json::to_vec(&value).expect("tool JSON").into_boxed_slice(),
        }]),
        finish: Finish::ToolCalls,
        tokens: 20,
    };
    let grant = |kind| match authority.scope {
        Some((forge, org)) => {
            json!({"connector":1,"kind":kind,"segments":["forge",forge.to_string()],"terminal":"open","last":org})
        }
        None => json!({"connector":1,"kind":kind,"segments":["forge"],"terminal":"open","last":""}),
    };
    let delegate = json!({"batch":[{"executor":{"kind":"procedure","connector":1,"code":2},"spec":{"words":coding_words,"parameters":[{"name":1,"kind":"resource","connector":1,"resource":repository},{"name":2,"kind":"bytes","value":"main"}]},"contract":{"kind":"change","connector":1,"change_kind":1,"words":128},"authority":{"tools":1,"grants":[grant(2),grant(3),grant(4),grant(10),grant(11),grant(12)],"delegation":{"kinds":[{"kind":"agent","number":authority.coding_kind}],"tasks":4,"depth":2},"budget":{"spend":10},"notes":0}}]});
    let original = std::str::from_utf8(readme).expect("README UTF-8");
    let changed = format!("{original}{}", std::str::from_utf8(edit).expect("edit UTF-8"));
    Box::new([
        Script {
            cue: chat_cue.into(),
            turns: Box::new([
                call(b"delegate", delegate),
                Turn {
                    lines: Box::new([
                        Line::Text { text: b"Change requested".as_slice().into() },
                        Line::Call { name: b"wait".as_slice().into(), arguments: b"{}".as_slice().into() },
                    ]),
                    finish: Finish::ToolCalls,
                    tokens: 20,
                },
                call(b"finish", json!({"form":"report","text":"The change landed.","fields":{}})),
            ]),
        },
        Script {
            cue: coding_words.as_bytes().into(),
            turns: Box::new([
                call(b"read", json!({"path":"README.md"})),
                call(b"edit", json!({"path":"README.md","edits":[{"old":original,"new":changed}]})),
                call(
                    b"finish",
                    json!({"form":"change","fields":{"title":"Update README","body":"Changed by temper."}}),
                ),
            ]),
        },
        Script {
            cue: b"Review this change at the named head".as_slice().into(),
            turns: Box::new([call(
                b"finish",
                json!({"form":"verdict","label":"1","text":"Checked","fields":{},"items":[]}),
            )]),
        },
    ])
}
