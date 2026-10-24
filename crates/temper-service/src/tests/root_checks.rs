// Fixture parsing without a deployment keeps written paths.
//! What the root's construction asserts and the worker's rules against the
//! agent, each refused at start naming its path (protocol/configuration.md, 7).
#![expect(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "ordinary test code edits UTF-8 configuration fixtures and reads their diagnostics"
)]
use super::fixtures;
use crate::startup::{Problems, checks, lines};

const ENGINE_OVERRIDES: &str = r#""tasks.result_bytes":32,"tasks.run_inbox":2,"hub.turns":1,"people.amendment_bytes":64,"core.policy_bytes":512,"tasks.spec_bytes":128,"tasks.message_bytes":512,"tasks.hold_bytes":64,"tasks.authority_bytes":64,"people.words":512,"core.call_answer_bytes":2048,"notes.recalled":1"#;
const WORKER_OVERRIDES: &str = r#""hub.slots":1,"hub.run_calls":1,"tasks.run_inbox":2,"hub.turns":1,"tasks.message_bytes":512,"people.words":512,"core.call_answer_bytes":2048"#;

fn started(document: &[u8]) -> Box<[u8]> {
    let found = match crate::startup(
        document,
        &jig_protocol_config::Directories::working(),
        &fixtures::files(),
        &fixtures::startup_agent(),
        &fixtures::seeds(),
        None,
        fixtures::worker_runtime(),
        fixtures::forge_runtime(),
    ) {
        Ok(service) => {
            drop(service);
            Problems::empty()
        }
        Err(problems) => problems,
    };
    lines(&found)
}
fn overridden(document: &[u8], path: &[u8], shipped: &str, extra: &str) -> Box<[u8]> {
    fixtures::with(document, path, format!("{{{shipped},{extra}}}").as_bytes())
}
fn engine_with(extra: &str) -> Box<[u8]> {
    overridden(&fixtures::document(), b"engine.limits.overrides", ENGINE_OVERRIDES, extra)
}
/// The fixture engine's root as the start makes it, held to `limits`.
fn root(limits: &temper_engine_domain::Limits) -> Box<[u8]> {
    let engine = fixtures::engine();
    let mut found = Problems::empty();
    let seeds = fixtures::seeds();
    let mut core = jig_protocol_config::core(&engine.jig, seeds.deployment, seeds.seed);
    for charter in &mut core.defaults.charters {
        if let Some(contents) = engine.files.contents(&charter.policy.instructions) {
            charter.policy.instructions = Box::from(contents);
        }
    }
    checks::root(core, &engine, &seeds, &[], limits, &mut found);
    lines(&found)
}
fn root_limits() -> temper_engine_domain::Limits {
    let engine = fixtures::engine();
    crate::limits::engine(&engine.jig.limits, &engine.limits).unwrap()
}

#[test]
fn the_fixture_document_starts() {
    assert_eq!(&*started(&fixtures::document()), b"");
    assert_eq!(&*root(&root_limits()), b"");
}
#[test]
fn engine_slots_above_0_refuse_the_start() {
    assert_eq!(
        &*started(&engine_with(r#""fleet.engine_slots":1"#)),
        b"engine.limits.overrides.fleet.engine_slots: the root has no hub, so the fleet must place every run on a worker\n"
    );
}
#[test]
fn a_charter_the_engine_may_host_refuses_the_start() {
    for hosts in [br#""engine""#.as_slice(), br#""both""#] {
        let document = fixtures::with(&fixtures::document(), b"engine.charters[0].hosts", hosts);
        assert_eq!(
            &*started(&document),
            b"engine.charters[0].hosts: the root has no hub, so the fleet must place every run on a worker\n"
        );
    }
}
#[test]
fn an_identity_past_the_accounts_credentials_refuses_the_start() {
    let document = fixtures::with(&fixtures::document(), b"application.identities[0].number", b"4");
    let document = fixtures::with(&document, b"application.forges[0].identity", b"4");
    assert_eq!(
        &*started(&document),
        b"application.identities[0].number: must be from 1 to accounts.credentials, in engine.limits and in the worker's limits\n"
    );
    let document = overridden(
        &fixtures::document(),
        b"workers[0].limits.overrides",
        WORKER_OVERRIDES,
        r#""accounts.credentials":1"#,
    );
    let document = fixtures::with(&document, b"application.identities[0].number", b"2");
    let document = fixtures::with(&document, b"application.forges[0].identity", b"2");
    assert_eq!(
        &*started(&document),
        b"application.identities[0].number: must be from 1 to accounts.credentials, in engine.limits and in the worker's limits\n"
    );
}
#[test]
fn the_engines_repositories_below_the_checkouts_refuse_the_start() {
    assert_eq!(
        &*started(&fixtures::with(
            &fixtures::document(),
            b"application.limits.overrides",
            br#"{"checkout.name_bytes":600,"checkout.repositories":4,"git_component.stdout_bytes":2048,"forge.name_bytes":256,"forge.resources_per_task":2}"#,
        )),
        b"application.limits.overrides.forge.resources_per_task: must agree with checkout.repositories and checkout.name_bytes\n"
    );
}
#[test]
fn the_hubs_accounts_past_the_agents_name_both_limits() {
    let document =
        overridden(&fixtures::document(), b"workers[0].limits.overrides", WORKER_OVERRIDES, r#""hub.accounts":4096"#);
    assert_eq!(
        &*started(&document),
        b"workers[0].limits.overrides.hub.accounts: must agree with the agent's accounts\n"
    );
}
#[test]
fn the_checkouts_repositories_past_the_agents_directories_name_both_limits() {
    let mut agent = fixtures::startup_agent();
    agent.host_limits.directories = 1;
    let worker = fixtures::worker_settings();
    let own = crate::limits::profile(crate::limits::Profile::Small);
    let mut found = Problems::empty();
    checks::worker(&crate::worker_settings::root_limits(&worker, &agent, &own), &mut found);
    assert_eq!(
        &*lines(&found),
        b"application.limits.overrides.checkout.repositories: must agree with the agent's directories\n"
    );
}
#[test]
fn a_resume_limit_past_a_rows_bytes_refuses_the_start() {
    assert_eq!(
        &*started(&fixtures::with(
            &engine_with(r#""core.resume_bytes":65541,"hub.transcript_bytes":262161"#),
            b"workers[0].limits.overrides",
            format!("{{{WORKER_OVERRIDES},\"hub.transcript_bytes\":262161}}").as_bytes(),
        )),
        b"engine.limits.overrides.core.resume_bytes: must agree with root.journal.transcript_bytes, above 0\n"
    );
}
#[test]
fn judges_tables_smaller_than_the_authoritys_refuse_the_start() {
    let mut limits = root_limits();
    limits.forge.judge_projects = limits.authority.projects.checked_sub(1).unwrap();
    assert_eq!(
        &*root(&limits),
        b"application.limits.overrides.forge.judge_projects: must agree with authority.projects in engine.limits\n"
    );
    let mut limits = root_limits();
    limits.forge.judge_criteria = limits.authority.requirements.checked_sub(1).unwrap();
    assert_eq!(
        &*root(&limits),
        b"application.limits.overrides.forge.judge_criteria: must agree with authority.requirements in engine.limits\n"
    );
}
#[test]
fn an_owners_project_without_an_escalation_role_refuses_the_start() {
    let document = fixtures::with(&fixtures::document(), b"engine.projects[0].policy.escalation_role", b"null");
    assert_eq!(
        &*started(&document),
        b"engine.projects[0].policy.escalation_role: an owner's project must name an escalation role from 0 to 3\n"
    );
}
#[test]
fn a_charters_run_past_its_bytes_refuses_the_start() {
    let mut limits = root_limits();
    limits.journal.run_bytes = 16;
    let found = root(&limits);
    assert!(found.starts_with(b"engine.charters[0]: "), "{}", String::from_utf8_lossy(&found));
    assert!(found.ends_with(b" bytes, past root.journal.run_bytes of 16\n"), "{}", String::from_utf8_lossy(&found));
}
#[test]
fn permission_mappings_past_a_rows_bytes_refuse_the_start() {
    let mut limits = root_limits();
    limits.journal.transcript_bytes = 1;
    limits.forge.judge_projects = limits.authority.projects;
    let found = root(&limits);
    assert!(
        String::from_utf8_lossy(&found).lines().any(|line| line.starts_with("engine.projects[0].roles: ")
            && line.ends_with(" bytes, past root.journal.transcript_bytes of 1")),
        "{}",
        String::from_utf8_lossy(&found)
    );
}
#[test]
fn landing_rules_past_the_authoritys_requirements_refuse_the_start() {
    let rule = r#"{"connector":1,"kind":4,"pattern":{"segments":["forge","127.0.0.1:9","acme","app"],"last":"open","value":"main"},"enforce":true,"ci":true,"up_to_date":true,"gates":[{"number":3,"blocking":true,"freshness":"exact"}],"approvals":[]}"#;
    let document = engine_with(r#""authority.requirements":2"#);
    let deployment = fixtures::with(&document, b"application.landing.deployment", format!("[{rule}]").as_bytes());
    let deployment = fixtures::with(&deployment, b"application.landing.projects", b"[]");
    assert_eq!(
        &*started(&deployment),
        b"application.landing.deployment: must agree with root.journal.transcript_bytes, authority.requirements, forge.judge_criteria and forge.change_policy.gates\n"
    );
    let project = fixtures::with(&document, b"application.landing.projects[0].rules", format!("[{rule}]").as_bytes());
    assert_eq!(
        &*started(&project),
        b"application.landing.projects[0].rules: must agree with root.journal.transcript_bytes, authority.requirements, forge.judge_criteria and forge.change_policy.gates\n"
    );
}
#[test]
fn forges_past_the_clients_op_bytes_refuse_the_start() {
    let mut limits = root_limits();
    limits.forge.client.op_bytes = 1;
    assert_eq!(&*root(&limits), b"application.forges: must agree with forge.repositories and forge.client.op_bytes\n");
}
#[test]
fn a_deployment_provider_0_refuses_the_start() {
    let document = fixtures::with(&fixtures::document(), b"engine.providers[1].number", b"0");
    assert_eq!(
        &*started(&document),
        b"engine.providers[1].number: the deployment's provider must not be 0, the forge's sign-in provider\n"
    );
}

#[test]
fn configured_worker_count_past_membership_refuses_startup() {
    let mut limits = root_limits();
    limits.fleet.configured_workers = 1;
    assert_eq!(&*root(&limits), b"engine.hosts.workers: past its bound\n");
}
#[test]
fn configured_worker_name_past_membership_names_its_startup_path() {
    let mut limits = root_limits();
    limits.fleet.worker_name_bytes = 1;
    assert_eq!(&*root(&limits), b"engine.hosts.workers[0].name: past its bound\n");
}
