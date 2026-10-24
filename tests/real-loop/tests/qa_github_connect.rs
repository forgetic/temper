//! Adversarial public-client stories; client/README.md 8 and protocol/setup.md 17.3–4.
use std::net::SocketAddr;

use serde_json::Value;
use temper_real_loop_world::{
    Exit, Ran,
    calls::{self, Arg, Argv, D1, Step},
    github, run_github,
};

// Every story installs the App on one account; replays and races must not add a second.
const ONE_INSTALLATION: usize = 1;

fn fresh() -> github::Fixture {
    github::Fixture { connected: false, ..github::fixture() }
}

fn no_agents(_: SocketAddr) -> Box<[skein_fake_llm_domain::api::Script]> {
    Box::new([])
}

fn call(words: &[&'static str]) -> Step {
    calls::call(D1, Argv(words.iter().map(|word| Arg::Lit(word)).collect()))
}

fn connect(app: Arg, key: &'static str) -> Step {
    calls::call(
        D1,
        Argv(vec![
            Arg::Lit("forge-connect"),
            Arg::Lit("1"),
            Arg::Lit("github-app"),
            app,
            calls::file("github-app.pem"),
            Arg::Lit("--key"),
            Arg::Lit(key),
        ]),
    )
}

fn begin() -> Vec<Step> {
    vec![
        calls::sign_in(D1),
        connect(calls::github_app(), "qa-connect"),
        call(&["wait", "forges", "--until", "connected:1", "--json"]),
    ]
}

fn adopt(owner: &'static str, name: &'static str, key: &'static str) -> Step {
    call(&[
        "adopt",
        "--project",
        "1",
        "--forge",
        "1",
        "--owner",
        owner,
        "--name",
        name,
        "--landing",
        "main",
        "--key",
        key,
        "--json",
    ])
}

fn json(ran: &Ran, at: usize) -> Value {
    let called = &ran.calls()[at];
    let bytes = if called.stdout.is_empty() { &called.stderr } else { &called.stdout };
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "call {at} JSON: {error}; stdout {}; stderr {}",
            String::from_utf8_lossy(&called.stdout),
            String::from_utf8_lossy(&called.stderr)
        )
    })
}

fn row(document: &Value) -> &Value {
    // `wait` wraps its current document with `met`; a direct read does not.
    let document = if document.get("met").is_some() { &document["forges"] } else { document };
    document["forges"]
        .as_array()
        .unwrap_or_else(|| panic!("forge rows: {document}"))
        .iter()
        .find(|row| row["number"] == 1)
        .expect("configured GitHub forge")
}

fn succeeded(ran: &Ran, at: usize) {
    let called = &ran.calls()[at];
    assert_eq!(
        called.exit,
        Some(Exit::Code(0)),
        "call {at}: {} {}",
        String::from_utf8_lossy(&called.stdout),
        String::from_utf8_lossy(&called.stderr)
    );
}

fn no_git_writes_or_key_leaks(ran: &Ran) {
    assert!(ran.github().git_calls.iter().all(|call| !call.writes), "no git mutation from connection/adoption");
    let pem = ran.github().app.as_ref().expect("fake-made App").pem.expose();
    let body_line = pem.split(|byte| *byte == b'\n').find(|line| line.len() == 64).expect("PEM body line");
    let check = |bytes: &[u8], context: &str| {
        assert!(
            !bytes.windows(body_line.len()).any(|part| part == body_line),
            "no PEM body outside secrets: {context}"
        );
        for der in skein_fake_forge_github::keys::KEYS {
            assert!(!bytes.windows(der.len()).any(|part| part == der), "no DER outside secrets: {context}");
        }
    };
    for called in ran.calls() {
        check(&called.stdout, "ctl stdout");
        check(&called.stderr, "ctl stderr");
    }
    for page in ran.browsed() {
        check(&page.body, "browser body");
    }
    // Only this test's nonsecret state and client trees; never open the secrets store.
    let mut pending = vec![ran.path().join("state"), ran.path().join("clients")];
    let mut files = 0;
    while let Some(path) = pending.pop() {
        if path == ran.path().join("state/generations/1/secrets") {
            continue;
        }
        if path.is_dir() {
            for entry in std::fs::read_dir(path).expect("test state directory") {
                pending.push(entry.expect("test state entry").path());
            }
        } else {
            check(&std::fs::read(&path).expect("test state file"), path.to_str().expect("test path"));
            files += 1;
        }
    }
    assert!(files > 0, "persisted state was checked");
}

#[test]
#[ignore = "backlog: Q1, Adopt with no accepted installation is refused no_access, not protocol/forge.md's not installed; the remedy's words only, off the happy path"]
fn an_unvisited_install_link_does_not_authorize_adoption() {
    // domain/forge.md 4.3: no accepted installation means NotInstalled.
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            call(&["forge-install", "1", "--key", "qa-abandoned"]),
            adopt("acme", "app", "qa-no-install"),
            call(&["forges", "--json"]),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    for at in 0..4 {
        succeeded(&ran, at);
    }
    assert!(!json(&ran, 4)["adopted"].is_object(), "unvisited link grants no adoption");
    assert_eq!(row(&json(&ran, 5))["installations"], serde_json::json!([]));
    assert!(ran.github().installations.is_empty());
    assert_eq!(ran.github().minted, 0, "no token before acceptance");
    assert_eq!(
        json(&ran, 4)["forge_refused"]["reason"],
        "not_installed",
        "adoption names the missing installation: {}",
        json(&ran, 4)
    );
}

#[test]
fn a_read_only_app_keeps_its_hook_and_old_stray_through_paste_and_reconcile() {
    // protocol/setup.md 17.7: another owner's App, hook and installations are read only.
    const STATE_LIFE_MS: u64 = 1000;
    // begin has sign-in, connect and wait; the reconcile wait precedes install.
    const INSTALL: usize = 4;
    const OTHER_HOOK: &[u8] = b"https://another-owner.invalid/hook";
    // The old stray, kept, and the installation this run accepts.
    const KEPT: usize = 2;
    let fixture = github::Fixture {
        old_stray: true,
        pasted_hook: OTHER_HOOK,
        configure: |value, _, _| {
            value["application"]["forges"][0]["app_read_only"] = true.into();
            value["application"]["limits"]["overrides"]["setup.state_life_ms"] = STATE_LIFE_MS.into();
        },
        ..fresh()
    };
    let ran = run_github(fixture, no_agents, || {
        let mut steps = begin();
        // Connected is published before reconcile completes; await its end before
        // asking for an install link, since setup refuses overlapping plans as busy.
        steps.extend([
            call(&["wait", "forges", "--until", "settled", "--json"]),
            call(&["forge-install", "1", "--key", "qa-read-only-install"]),
            calls::browse(INSTALL),
            call(&["wait", "forges", "--until", "settled", "--json"]),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    for at in 0..ran.calls().len() {
        succeeded(&ran, at);
    }
    let seen = ran.github();
    let reads = |target: &[u8]| {
        seen.app_calls.iter().filter(|(method, path)| *method == skein_http::Method::Get && path == target).count()
    };
    assert!(reads(b"/app/hook/config") >= 2, "paste and reconcile both read the differing hook");
    assert!(reads(b"/app/installations?per_page=100&page=2") > 0, "reconcile reached the terminal sweep page");
    let mutations: Vec<_> = seen
        .app_calls
        .iter()
        .filter(|(method, path)| {
            *method == skein_http::Method::Patch && path == b"/app/hook/config"
                || *method == skein_http::Method::Delete && path.starts_with(b"/app/installations/")
        })
        .map(|(method, path)| (method, String::from_utf8_lossy(path)))
        .collect();
    assert!(mutations.is_empty(), "neither paste nor reconcile mutates the hook or installations: {mutations:?}");
    assert!(seen.installations.iter().any(|(id, _, login)| *id == github::INSTALLATION && login == b"umbra"));
    let created = seen.installation_created.iter().find(|(id, _)| *id == github::INSTALLATION).expect("old stray").1;
    let wall = skein_shell::Clock::new().now().wall;
    assert!(wall.as_nanos() - created.as_nanos() > skein_lib::Duration::from_millis(STATE_LIFE_MS).as_nanos());
    assert_eq!(seen.installations.len(), KEPT, "old stray and newly accepted installation both remain");
    assert_eq!(ran.browsed()[0].hops, [302, 200], "read only preserves installation acceptance");
}

#[test]
fn replaying_an_install_request_keeps_one_link_and_one_acceptance() {
    // client/README.md 8: the same live request key returns the same link.
    // client/forge.md 9.1: an installation state is usable once.
    const LINK: usize = 3;
    const REPLAY: usize = 4;
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            call(&["forge-install", "1", "--key", "qa-repeat"]),
            call(&["forge-install", "1", "--key", "qa-repeat"]),
            calls::browse(3),
            calls::browse(4),
            adopt("acme", "app", "qa-after-replay"),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    for at in 0..ran.calls().len() {
        succeeded(&ran, at);
    }
    assert_eq!(ran.calls()[LINK].stdout, ran.calls()[REPLAY].stdout, "same request, same link");
    assert_eq!(ran.browsed()[0].hops, [302, 200]);
    assert_eq!(ran.browsed()[1].hops, [302, 410], "used state ends");
    assert_eq!(ran.github().installations.len(), ONE_INSTALLATION, "no duplicate installation");
    assert!(json(&ran, 5)["adopted"].is_object(), "first acceptance survives replay");
}

#[test]
fn an_accepted_installation_fits_the_ctls_forges_status() {
    // client/README.md 4.6 and client/forge.md 5.4: status carries the installation.
    const WAIT: usize = 4;
    const STATUS: usize = 5;
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            call(&["forge-install", "1", "--key", "qa-status"]),
            calls::browse(3),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            call(&["forges", "--json"]),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    assert_eq!(ran.browsed()[0].hops, [302, 200]);
    assert!(
        [WAIT, STATUS].iter().all(|at| ran.calls()[*at].exit == Some(Exit::Code(0))),
        "wait exit {:?}: {}; forges exit {:?}: {}",
        ran.calls()[WAIT].exit,
        String::from_utf8_lossy(&ran.calls()[WAIT].stderr),
        ran.calls()[STATUS].exit,
        String::from_utf8_lossy(&ran.calls()[STATUS].stderr)
    );
    assert_eq!(row(&json(&ran, STATUS))["installations"].as_array().expect("installations").len(), ONE_INSTALLATION);
}

#[test]
#[ignore = "backlog: Q2, Adopt on an account without an accepted installation is refused no_access, not not installed; access stays scoped, the remedy's words only"]
fn accepting_acme_does_not_grant_umbras_repository() {
    // domain/forge.md 4.3: installation access is scoped to its account.
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            call(&["forge-install", "1", "--key", "qa-acme"]),
            calls::browse(3),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            adopt("umbra", "tool", "qa-wrong-owner"),
            adopt("acme", "app", "qa-right-owner"),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    assert!(!json(&ran, 5)["adopted"].is_object(), "another account is not granted");
    succeeded(&ran, 6);
    assert!(json(&ran, 6)["adopted"].is_object(), "right account still adopts: {}", json(&ran, 6));
    assert_eq!(
        json(&ran, 5)["forge_refused"]["reason"],
        "not_installed",
        "wrong account gets the actionable installation refusal: {}",
        json(&ran, 5)
    );
    assert_eq!(ran.github().installations.len(), ONE_INSTALLATION);
}

#[test]
fn an_app_id_mismatch_leaves_a_public_failure_without_the_key() {
    // protocol/setup.md 17.3: OtherApp; credentials.md 2.2: no key in outcomes.
    let ran = run_github(fresh(), no_agents, || {
        vec![
            calls::sign_in(D1),
            connect(calls::number(u64::MAX), "qa-other-app"),
            call(&["wait", "forges", "--until", "settled", "--json"]),
        ]
    });
    succeeded(&ran, 0);
    succeeded(&ran, 2);
    let document = json(&ran, 2);
    assert!(!row(&document)["ended"].is_null(), "row explains the failed paste");
    assert!(ran.github().installations.is_empty());
    assert_eq!(ran.github().minted, 0);
    let pem = ran.github().app.as_ref().expect("fake-made App").pem.expose();
    for called in ran.calls() {
        for bytes in [&called.stdout, &called.stderr] {
            assert!(!bytes.windows(pem.len()).any(|part| part == pem), "PEM kept out of output");
            assert!(!bytes.windows(b"BEGIN RSA PRIVATE KEY".len()).any(|part| part == b"BEGIN RSA PRIVATE KEY"));
        }
    }
    no_git_writes_or_key_leaks(&ran);
}

#[test]
#[ignore = "gap: real-loop steps expose no GitHub suspension or revocation mutation"]
fn a_suspended_installation_and_a_revoked_key_stop_repository_access() {
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([call(&["forge-install", "1", "--key", "qa-suspend-revoke"]), calls::browse(3)]);
        steps
    });
    assert_eq!(ran.browsed()[0].hops, [302, 200]);
    // Connect and accept, suspend through Service::installations_mut, assert refusal;
    // revoke through Service::revoke_key, assert Stopped with no token or key output.
    panic!("gap: no step can mutate the running GitHub service");
}

#[test]
fn ctl_human_and_json_status_follow_paste_and_install() {
    // client/README.md 4.6; client/forge.md 5.4; client/tools.md 5.5.
    const HUMAN_BEFORE: usize = 2;
    const HUMAN_PASTED: usize = 5;
    const HUMAN_INSTALLED: usize = 9;
    let ran = run_github(fresh(), no_agents, || {
        vec![
            calls::sign_in(D1),
            call(&["forges", "--json"]),
            call(&["forges"]),
            connect(calls::github_app(), "qa2-status-connect"),
            call(&["wait", "forges", "--until", "connected:1", "--json"]),
            call(&["forges"]),
            call(&["forge-install", "1", "--key", "qa2-status-install"]),
            call(&["forges", "--json"]),
            calls::browse(6),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            call(&["forges"]),
        ]
    });
    no_git_writes_or_key_leaks(&ran);
    for at in 0..ran.calls().len() {
        succeeded(&ran, at);
    }
    let initial = json(&ran, 1);
    assert_eq!(row(&initial)["state"], "not_connected");
    assert_eq!(row(&initial)["installations"], serde_json::json!([]));
    assert!(row(&initial)["ended"].is_null());
    assert!(String::from_utf8_lossy(&ran.calls()[HUMAN_BEFORE].stdout).contains("Not connected"));
    let pasted = json(&ran, 4);
    let before_visit = json(&ran, 7);
    for packet in [&pasted, &before_visit] {
        assert_eq!(row(packet)["state"], "connected");
        assert_eq!(row(packet)["login"], "temper-person[bot]");
        assert_eq!(row(packet)["installations"], serde_json::json!([]));
        assert!(row(packet)["ended"].is_null());
    }
    let installed = json(&ran, 8);
    let installations = row(&installed)["installations"].as_array().expect("installation rows");
    assert_eq!(installations.len(), ONE_INSTALLATION);
    assert_eq!(installations[0]["id"], github::INSTALLATION);
    if !installations[0]["account"].is_null() {
        assert_eq!(installations[0]["account"]["login"], "acme");
    }
    assert_eq!(installations[0]["suspended"], false);
    assert_eq!(installations[0]["stray"], false);
    let uninstalled = String::from_utf8_lossy(&ran.calls()[HUMAN_PASTED].stdout);
    assert!(uninstalled.contains("Connecting: installed nowhere yet"), "human uninstalled status: {uninstalled}");
    let text = String::from_utf8_lossy(&ran.calls()[HUMAN_INSTALLED].stdout);
    assert!(text.contains("Connected as temper-person[bot]"), "human installed status: {text}");
    assert!(
        text.contains("acme") || text.to_lowercase().contains("installation 1"),
        "human installation account or its id while the account is unknown: {text}"
    );
    assert!(
        skein_lib::bytes::find(&ran.browsed()[0].body, b"temper has the installation; you may close this tab.")
            .is_some(),
        "the setup page says so"
    );
}

#[test]
fn a_failed_paste_is_named_in_human_status_and_cleared_by_retry() {
    // client/forge.md 5.4: a stopped run stays on the row until the next connect.
    const HUMAN_FAILED: usize = 3;
    const INSTALL: usize = 4;
    const HUMAN_RETRIED: usize = 8;
    let ran = run_github(fresh(), no_agents, || {
        vec![
            calls::sign_in(D1),
            connect(calls::number(u64::MAX), "qa2-failed-paste"),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            call(&["forges"]),
            call(&["forge-install", "1", "--key", "qa2-install-too-soon", "--json"]),
            call(&["forges", "--json"]),
            connect(calls::github_app(), "qa2-retry-paste"),
            call(&["wait", "forges", "--until", "connected:1", "--json"]),
            call(&["forges"]),
        ]
    });
    no_git_writes_or_key_leaks(&ran);
    for at in [0, 1, 2, HUMAN_FAILED, 5, 6, 7, HUMAN_RETRIED] {
        succeeded(&ran, at);
    }
    let failed = json(&ran, 2);
    let why = row(&failed)["ended"].as_str().unwrap_or_else(|| panic!("last connect reason: {failed}"));
    assert!(!why.is_empty() && why.len() <= 256);
    let text = String::from_utf8_lossy(&ran.calls()[HUMAN_FAILED].stdout);
    assert_eq!(ran.calls()[INSTALL].exit, Some(Exit::Code(2)));
    let refusal = json(&ran, INSTALL);
    assert!(refusal.to_string().contains("\"not_connected\""), "install refused as not connected: {refusal}");
    assert_eq!(row(&json(&ran, 5))["ended"], row(&failed)["ended"], "admission refusal keeps prior run's reason");
    let retried = json(&ran, 7);
    assert_eq!(row(&retried)["state"], "connected");
    assert!(row(&retried)["ended"].is_null());
    assert!(!String::from_utf8_lossy(&ran.calls()[HUMAN_RETRIED].stdout).contains("The last connect stopped:"));
    assert!(ran.github().installations.is_empty());
    assert_eq!(ran.github().minted, 0);
    assert!(
        text.contains("The last connect stopped:") && text.contains(why),
        "human last connect omits stored reason {why:?}: {text}"
    );
}

#[test]
fn replaying_a_paste_and_refusing_a_new_connect_preserves_the_app() {
    // client/README.md 8: keyed replay; 4.6: connected refusal; credentials.md 2.2.
    const CONNECT: usize = 1;
    const REPLAY: usize = 3;
    const REFUSED: usize = 4;
    const REPLAY_AGAIN: usize = 6;
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            connect(calls::github_app(), "qa-connect"),
            connect(calls::number(u64::MAX), "qa2-already-connected"),
            call(&["forges", "--json"]),
            connect(calls::github_app(), "qa-connect"),
            call(&["forge-install", "1", "--key", "qa2-after-repeated-paste"]),
            calls::browse(7),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            adopt("acme", "app", "qa2-repeated-paste-adopt"),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    for at in [0, CONNECT, 2, REPLAY, 5, REPLAY_AGAIN, 7, 8, 9] {
        succeeded(&ran, at);
    }
    assert_eq!(ran.calls()[CONNECT].stdout, ran.calls()[REPLAY].stdout, "replay retains original connecting outcome");
    assert_eq!(ran.calls()[CONNECT].stdout, ran.calls()[REPLAY_AGAIN].stdout);
    assert_eq!(ran.calls()[REFUSED].exit, Some(Exit::Code(2)), "new connect cannot replace a connected App");
    let refused = String::from_utf8_lossy(&ran.calls()[REFUSED].stdout);
    let refused_error = String::from_utf8_lossy(&ran.calls()[REFUSED].stderr);
    assert!(format!("{refused}{refused_error}").to_lowercase().contains("connected"));
    let packet = json(&ran, 5);
    assert_eq!(row(&packet)["login"], "temper-person[bot]");
    assert!(row(&packet)["ended"].is_null(), "refusal did not start a failing Paste");
    assert_eq!(ran.github().installations.len(), ONE_INSTALLATION);
    assert_eq!(row(&json(&ran, 8))["installations"].as_array().expect("installations").len(), ONE_INSTALLATION);
    assert!(json(&ran, 9)["adopted"].is_object(), "kept key still authorizes adoption");
}

#[test]
fn install_json_and_human_replays_name_the_same_live_link() {
    // client/README.md 8 and client/tools.md 5.5: one keyed outcome, either rendering.
    const JSON_LINK: usize = 3;
    const HUMAN_LINK: usize = 4;
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            call(&["forge-install", "1", "--key", "qa2-link-formats", "--json"]),
            call(&["forge-install", "1", "--key", "qa2-link-formats"]),
            calls::browse(4),
            call(&["forge-install", "1", "--key", "qa2-link-formats", "--json"]),
            call(&["wait", "forges", "--until", "settled", "--json"]),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    for at in 0..ran.calls().len() {
        succeeded(&ran, at);
    }
    let packet = json(&ran, JSON_LINK);
    let link = packet["link"]["address"].as_str().unwrap_or_else(|| panic!("JSON installation link: {packet}"));
    let address: std::net::SocketAddr = link
        .strip_prefix("http://")
        .expect("fake HTTP")
        .split('/')
        .next()
        .expect("authority")
        .parse()
        .expect("local socket");
    assert!(address.ip().is_loopback(), "link points only at the fake");
    let human = String::from_utf8_lossy(&ran.calls()[HUMAN_LINK].stdout);
    assert!(human.lines().any(|line| line == link), "same link in human output");
    assert!(human.contains("Open this address in a browser on the engine's machine:"));
    assert_eq!(json(&ran, 5), packet, "acceptance does not turn a replay into a new install");
    assert_eq!(ran.browsed()[0].hops, [302, 200]);
    assert_eq!(ran.github().installations.len(), ONE_INSTALLATION);
    assert_eq!(row(&json(&ran, 6))["installations"].as_array().expect("installations").len(), ONE_INSTALLATION);
}

#[test]
fn independent_install_links_remain_live_and_accept_one_installation() {
    // protocol/setup.md 17.4; client/forge.md 9.1: distinct live one-use states.
    const FIRST: usize = 3;
    const SECOND: usize = 4;
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            call(&["forge-install", "1", "--key", "qa2-first-live"]),
            call(&["forge-install", "1", "--key", "qa2-second-live"]),
            calls::browse(4),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            calls::browse(3),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            calls::browse(4),
            calls::browse(3),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    for at in 0..ran.calls().len() {
        succeeded(&ran, at);
    }
    assert_ne!(ran.calls()[FIRST].stdout, ran.calls()[SECOND].stdout, "independent requests draw fresh states");
    let hops: Vec<_> = ran.browsed().iter().map(|page| page.hops.as_slice()).collect();
    assert_eq!(hops, [&[302, 200][..], &[302, 200], &[302, 410], &[302, 410]]);
    assert_eq!(ran.github().installations.len(), ONE_INSTALLATION, "same account is installed once");
    for at in [5, 6] {
        let packet = json(&ran, at);
        let installations = row(&packet)["installations"].as_array().expect("installation rows");
        assert_eq!(installations.len(), ONE_INSTALLATION, "Accepted is a set");
        assert_eq!(installations[0]["id"], github::INSTALLATION);
        assert_eq!(installations[0]["stray"], false);
    }
}

#[test]
fn refused_and_replayed_pastes_keep_keys_out_of_persisted_public_state() {
    // protocol/credentials.md 2.2: the carried PEM and DER never enter public records.
    const REFUSED: usize = 7;
    let ran = run_github(fresh(), no_agents, || {
        vec![
            calls::sign_in(D1),
            connect(calls::number(u64::MAX), "qa2-secrecy-failed"),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            connect(calls::number(u64::MAX), "qa2-secrecy-failed"),
            connect(calls::github_app(), "qa2-secrecy-ok"),
            call(&["wait", "forges", "--until", "connected:1", "--json"]),
            connect(calls::github_app(), "qa2-secrecy-ok"),
            connect(calls::github_app(), "qa2-secrecy-refused"),
            call(&["forge-install", "1", "--key", "qa2-secrecy-install"]),
            calls::browse(8),
            call(&["wait", "forges", "--until", "settled", "--json"]),
        ]
    });
    no_git_writes_or_key_leaks(&ran);
    for at in [0, 1, 2, 3, 4, 5, 6, 8, 9] {
        succeeded(&ran, at);
    }
    assert_eq!(ran.calls()[REFUSED].exit, Some(Exit::Code(2)));
    let pem = ran.github().app.as_ref().expect("fake App").pem.expose();
    let body: Vec<_> = pem
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.starts_with(b"-----"))
        .flatten()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    assert!(body.len() > 100, "nonempty fake key body");
    let mut pending = vec![ran.path().to_path_buf()];
    let mut checked = 0;
    while let Some(path) = pending.pop() {
        // Test input is intentionally private; the secrets store is never opened.
        if path.file_name().is_some_and(|name| name == "secrets" || name == "github-app.pem") {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).expect("scratch metadata");
        if metadata.is_dir() {
            for entry in std::fs::read_dir(&path).expect("scratch directory") {
                pending.push(entry.expect("scratch entry").path());
            }
        } else if metadata.is_file() {
            let bytes = std::fs::read(&path).expect("public scratch artifact");
            assert!(!bytes.windows(body.len()).any(|part| part == body), "no unwrapped PEM body in {}", path.display());
            assert!(
                !bytes.windows(b"BEGIN RSA PRIVATE KEY".len()).any(|part| part == b"BEGIN RSA PRIVATE KEY"),
                "no PEM armour in {}",
                path.display()
            );
            for der in skein_fake_forge_github::keys::KEYS {
                assert!(!bytes.windows(der.len()).any(|part| part == der), "no DER in {}", path.display());
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "nonsecret persisted files checked");
    assert_eq!(ran.github().installations.len(), ONE_INSTALLATION);
}

#[test]
#[ignore = "gap: no public setup-write journal or carried-secret row-count observer, including erases"]
fn replayed_pastes_and_accepts_make_no_duplicate_store_writes() {
    // client/README.md 8; protocol/credentials.md 2.2: one run, unused values erased.
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            connect(calls::github_app(), "qa-connect"),
            call(&["forge-install", "1", "--key", "qa2-write-count"]),
            call(&["forge-install", "1", "--key", "qa2-write-count"]),
            calls::browse(4),
            calls::browse(5),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    assert_eq!(ran.github().installations.len(), ONE_INSTALLATION);
    // Exact counts of Paste/Accept runs, AppKey/Accepted writes and carried-row
    // keeps/erases require an observer. Never inspect jig's secret values to count.
    panic!("gap: no public write or carried-row-count observer");
}

#[test]
#[ignore = "gap: Ran exposes successful mint count but no fake token fingerprint or redacted leakage observer"]
fn installation_tokens_stay_out_of_logs_records_and_saved_client_state() {
    // protocol/credentials.md 2.2; protocol/forge.md 2.4–5: no token outside secrets.
    let ran = run_github(fresh(), no_agents, || {
        let mut steps = begin();
        steps.extend([
            call(&["forge-install", "1", "--key", "qa2-token-secrecy"]),
            calls::browse(3),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            adopt("acme", "app", "qa2-token-adopt"),
        ]);
        steps
    });
    no_git_writes_or_key_leaks(&ran);
    for at in 0..ran.calls().len() {
        succeeded(&ran, at);
    }
    assert!(ran.github().minted > 0, "a token was exercised");
    // An observer must compare emitted artifacts to the fake's minted tokens
    // without opening any credential record or jig's secrets store.
    panic!("gap: the real-loop result exposes no token leakage observation");
}

#[test]
fn an_app_lacking_permissions_still_connects() {
    // protocol/setup.md 17.3 and 17.5: Lacking is done, never a refused paste.
    // client/forge.md 5.4 defers the row's Lacking line; only connection is required.
    const HUMAN: usize = 3;
    fn narrow() -> skein_fake_forge_github::Config {
        let mut config = (github::fixture().config)();
        config.app_facts = skein_fake_forge_github::service::AppFacts::Narrow;
        config
    }
    let ran = run_github(github::Fixture { config: narrow, ..fresh() }, no_agents, || {
        vec![
            calls::sign_in(D1),
            connect(calls::github_app(), "qa2-lacking-connect"),
            call(&["wait", "forges", "--until", "settled", "--json"]),
            call(&["forges"]),
        ]
    });
    no_git_writes_or_key_leaks(&ran);
    for at in 0..ran.calls().len() {
        succeeded(&ran, at);
    }
    let packet = json(&ran, 2);
    assert_eq!(row(&packet)["state"], "connected");
    assert_eq!(row(&packet)["login"], "temper-person[bot]");
    assert!(row(&packet)["ended"].is_null(), "Lacking ends done");
    assert!(String::from_utf8_lossy(&ran.calls()[HUMAN].stdout).contains("Connecting: installed nowhere yet"));
    assert_eq!(ran.github().minted, 0);
}
