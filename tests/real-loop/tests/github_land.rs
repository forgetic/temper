//! A chat's change is checked, reviewed and landed by its connected GitHub App.
use serde_json::Value;
use skein_fake_forge_domain::{Observation, api, object_bytes};
use std::fmt::Write as _;
use temper_real_loop_world::{
    Exit, Ran, argv,
    calls::{D1, Step, browse, call, file, github_app, number, sign_in},
    github, landing, run_github,
};

const INSTALL: usize = 5;
const CHANGE: usize = 10;
const CHAT: usize = 9;
const CHAT_TASK: u64 = 1;
const CHANGE_TASK: u64 = 2;
const ONE_PUSH: usize = 1;
const ONE_MERGE: usize = 1;

#[test]
fn a_chats_change_lands_on_connected_github() {
    let mut fixture = github::fixture();
    fixture.connected = false;
    fixture.configure = |value, agent, at| (landing::fixture().configure)(value, agent, at);
    let ran = run_github(fixture, scripts, connect_adopt_and_ask_for_a_change);
    check_connection(&ran);
    let commit = check_landing(&ran);
    check_chat_heard_the_landing(&ran, &commit);
    check_authenticated_git(&ran);
    let files = files_outside_the_secrets_store(&ran);
    no_installation_token_pattern_or_app_key_outside_the_secrets_store(&ran, &files);
}

// Main starts green; the coder removes its heading, and the repair restores it.
const BROKEN: &str = "Broken by temper.\n";
const REPAIR_CUE: &[u8] = b"Repair the failed CI on the change branch";

fn repair_fixture() -> github::Fixture {
    let mut fixture = github::fixture();
    fixture.connected = false;
    fixture.setup = || {
        let mut setup = (temper_real_loop_world::forge::fixture().setup)();
        setup.checks.cue = Some(api::Cue::File { path: b"README.md".as_slice().into(), green: landing::README.into() });
        setup
    };
    fixture.configure_domain = |config| {
        // Keep CI for both main and the change, with no mergeability delay.
        config.limits.statuses = 2;
        config.limits.observations = 32;
        config.settle_min = skein_lib::Duration::ZERO;
        config.settle_max = skein_lib::Duration::ZERO;
    };
    fixture.prepare = |forge, env| {
        // Setup starts no CI. Seed a green main through the same content cue.
        skein_fake_forge_domain::advance(
            forge,
            env,
            b"acme/app",
            b"main",
            b"README.md",
            landing::README,
            temper_github_app::APP.bot,
        )
        .expect("main checked before the chat's change");
    };
    fixture.configure = |value, agent, at| {
        (landing::fixture().configure)(value, agent, at);
        // The required CI section carries the failure to the repair worker.
        let limits = &mut value["application"]["limits"]["overrides"];
        for name in ["read_bytes", "budgets.ci", "brief_bytes"] {
            limits[format!("root.brief.{name}")] = 512.into();
        }
        limits["forge.client.poll_ms"] = 30.into();
        limits["forge_component.log_poll_ms"] = 100.into();
    };
    fixture
}

// GitHub compare uses per_page=1, so a two-commit change is refused.
#[test]
#[ignore = "skein compare per_page=1 (o/github-land-2 M2)"]
fn a_chats_failed_github_check_is_repaired_at_a_new_head_and_lands_once() {
    let ran = run_github(repair_fixture(), repair_scripts, repair_acts);
    let records = ran.github().observations();
    let seeded_main = records
        .iter()
        .find_map(|row| {
            let Observation::Moved { branch, to, .. } = row else { return None };
            (branch.as_ref() == b"main").then_some(*to)
        })
        .expect("seeded main head");
    assert!(
        records.iter().any(|row| matches!(row,
            Observation::Reported { commit, state: api::Check::Passed, .. } if *commit == seeded_main
        )),
        "main passed its content-cued CI"
    );
    let heads: Vec<_> = records
        .iter()
        .filter_map(|row| {
            let Observation::Moved { repository, branch, to, .. } = row else { return None };
            (repository.as_ref() == b"acme/app" && branch.as_ref() != b"main").then_some(*to)
        })
        .collect();
    let [first_head, repaired_head] = heads.as_slice() else { panic!("two pushed change heads: {heads:?}") };
    let reports: Vec<_> = records
        .iter()
        .enumerate()
        .filter_map(|(at, row)| {
            let Observation::Reported { repository, commit, context, state, .. } = row else { return None };
            (repository.as_ref() == b"acme/app"
                && context.as_ref() == b"check"
                && (commit == first_head || commit == repaired_head))
                .then_some((at, *commit, *state))
        })
        .collect();
    let [
        (pending_a, a, api::Check::Pending),
        (failed, failed_head, api::Check::Failed),
        (pending_b, b, api::Check::Pending),
        (passed, passed_head, api::Check::Passed),
    ] = reports.as_slice()
    else {
        panic!("failed then passed at two heads: {reports:?}")
    };
    assert_eq!(a, first_head);
    assert_eq!(b, repaired_head);
    assert_eq!(a, failed_head);
    assert_eq!(b, passed_head);
    assert_ne!(a, b, "repair pushes a new head");
    assert!(pending_a < failed && failed < pending_b && pending_b < passed);
    let merges: Vec<_> = records
        .iter()
        .enumerate()
        .filter_map(|(at, row)| {
            let Observation::Merged { head, .. } = row else { return None };
            Some((at, *head))
        })
        .collect();
    assert_eq!(merges.len(), ONE_MERGE, "the repaired head must merge once");
    assert_eq!(merges[0].1, *b, "only the repaired, passing head merged");
    assert!(*passed < merges[0].0, "CI passed before merge");
    assert_eq!(ran.github().git_calls.iter().filter(|call| call.writes).count(), 2);
    assert!(
        ran.llm().iter().any(|seen| matches!(seen,
            skein_fake_peers::llm::Observation::Query { query, .. }
            if skein_lib::bytes::find(&query.system, REPAIR_CUE).is_some()
        )),
        "the worker ran the engine's repair task"
    );
    check_connection(&ran);
    let commit = check_landing(&ran);
    check_chat_heard_the_landing(&ran, &commit);
}

fn repair_acts() -> Vec<Step> {
    let mut steps = connect_adopt_and_ask_for_a_change();
    let wait = steps.len() - 2;
    let Step::Call { argv, .. } = &mut steps[wait] else { panic!("chat wait before retained task read") };
    argv.0.extend(["--timeout".into(), "4s".into()]);
    steps
}

fn repair_scripts(at: std::net::SocketAddr) -> Box<[skein_fake_llm_domain::api::Script]> {
    use serde_json::json;
    use skein_fake_llm_domain::api::{Finish, Line, Script, Turn};
    let mut scripts = landing::scripts(at, u64::from(github::REPOSITORY), landing::README).into_vec();
    // The shared exchange delegates enough authority for producer, repair and review.
    scripts[1].turns[1].lines = Box::new([Line::Call {
        name: b"edit".as_slice().into(),
        arguments: serde_json::to_vec(&json!({"path":"README.md","edits":[{"old":"# app\n","new":BROKEN}]}))
            .expect("edit JSON")
            .into(),
    }]);
    let call = |name: &[u8], arguments: Value| Turn {
        lines: Box::new([Line::Call {
            name: name.into(),
            arguments: serde_json::to_vec(&arguments).expect("repair JSON").into(),
        }]),
        finish: Finish::ToolCalls,
        tokens: 20,
    };
    scripts.push(Script {
        cue: REPAIR_CUE.into(),
        turns: Box::new([
            call(b"read", json!({"path":"README.md"})),
            call(b"edit", json!({"path":"README.md","edits":[{"old":BROKEN,"new":"# app\nChanged by temper.\n"}]})),
            call(b"finish", json!({"form":"change","fields":{"title":"Restore README heading","body":"Repair CI."}})),
        ]),
    });
    scripts.into_boxed_slice()
}

fn connect_adopt_and_ask_for_a_change() -> Vec<Step> {
    vec![
        sign_in(D1),
        call(D1, argv!["add-key", "1", file("fixture.key"), "--deployment", "--key", "github-llm"]),
        call(D1, argv!["wait", "accounts", "--until", "usable"]),
        call(
            D1,
            argv!["forge-connect", "1", "github-app", github_app(), file("github-app.pem"), "--key", "github-app"],
        ),
        call(D1, argv!["wait", "forges", "--until", "connected:1"]),
        call(D1, argv!["forge-install", "1", "--key", "github-install"]),
        browse(INSTALL),
        call(D1, argv!["wait", "forges", "--until", "settled"]),
        call(
            D1,
            argv![
                "adopt",
                "--project",
                "1",
                "--forge",
                "1",
                "--owner",
                "acme",
                "--name",
                "app",
                "--landing",
                "main",
                "--key",
                "adopt-app"
            ],
        ),
        call(D1, argv!["chat", "--project", "1", "--key", "github-land", "@forge_chat Update README"]),
        call(D1, argv!["wait", "task", number(CHAT_TASK), "--until", "ended", "--json"]),
        call(D1, argv!["task", number(CHANGE_TASK), "--json"]),
    ]
}

fn check_connection(ran: &Ran) {
    for (index, result) in ran.calls().iter().enumerate() {
        assert_eq!(result.exit, Some(Exit::Code(0)), "call {index}: {result:?}");
    }
    let [visit] = ran.browsed() else { panic!("one installation visit") };
    assert_eq!(visit.call, INSTALL);
    assert_eq!(visit.hops, [302, 200]);
    assert!(
        skein_lib::bytes::find(&visit.body, b"temper has the installation; you may close this tab.").is_some(),
        "the setup page says so"
    );
}

fn check_landing(ran: &Ran) -> String {
    let seen = ran.github();
    let opened: Vec<_> =
        seen.pulls.iter().filter(|row| matches!(row, Observation::Opened { kind: api::Kind::Pull, .. })).collect();
    let merged: Vec<_> = seen.pulls.iter().filter(|row| matches!(row, Observation::Merged { .. })).collect();
    let [Observation::Opened { repository, number: pull, by: author, .. }] = opened.as_slice() else {
        panic!("one opened pull: {opened:?}")
    };
    let [Observation::Merged { repository: merged_repository, number, base, head, commit, by }] = merged.as_slice()
    else {
        panic!("one merge: {merged:?}")
    };
    assert_eq!(repository.as_ref(), b"acme/app");
    assert_eq!(merged_repository, repository);
    assert_eq!(number, pull);
    assert_eq!(base.as_ref(), b"main");
    assert_eq!(*by, temper_github_app::APP.bot);
    assert_eq!(author, by);
    let branches: Vec<_> = seen.branches.iter().filter(|(name, _)| name.as_slice() != b"main").collect();
    let [(_, branch)] = branches.as_slice() else { panic!("one pushed branch: {branches:?}") };
    assert_eq!(*head, branch.head, "merge at the coder's pushed head");
    let main = seen.branches.get(b"main".as_slice()).expect("main moved by merge");
    assert_eq!(main.head, *commit);
    assert_eq!(main.tree.get(b"README.md".as_slice()).expect("README"), &[landing::README, landing::EDIT].concat());
    let task: Value = serde_json::from_slice(&ran.calls()[CHANGE].stdout).expect("change document");
    assert_eq!(task["chip"]["task"], 2);
    assert_eq!(task["chip"]["phase"], "ended");
    let result = &task["result"]["value"]["change"];
    assert_eq!(result["kind"], 1, "landed change");
    assert_eq!(result["part"]["key"]["number"], *pull);
    let mut hex = String::new();
    for byte in object_bytes(*commit) {
        write!(hex, "{byte:02x}").expect("commit hex");
    }
    assert_eq!(result["words"]["bytes"], hex, "result names the merge commit");
    hex
}

fn check_chat_heard_the_landing(ran: &Ran, hex: &str) {
    let chat: Value = serde_json::from_slice(&ran.calls()[CHAT].stdout).expect("chat document");
    assert_eq!(chat["task"]["result"]["value"]["report"]["bytes"], "The change landed.");
    let told = format!("Result from task 2:\n{hex}");
    let chat_queries: Vec<_> = ran
        .llm()
        .iter()
        .filter_map(|seen| {
            let skein_fake_peers::llm::Observation::Query { query, .. } = seen else { return None };
            query.system.windows(b"@forge_chat".len()).any(|word| word == b"@forge_chat").then_some(query)
        })
        .collect();
    let received: Vec<_> = chat_queries
        .iter()
        .map(|query| {
            query
                .messages
                .iter()
                .flat_map(|message| message.parts.iter())
                .filter_map(|part| match part {
                    skein_fake_llm_domain::api::Part::Text { text }
                    | skein_fake_llm_domain::api::Part::ToolOutput { output: text, .. } => {
                        Some(String::from_utf8_lossy(text).into_owned())
                    }
                    skein_fake_llm_domain::api::Part::Opaque { .. }
                    | skein_fake_llm_domain::api::Part::ToolCall { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    assert!(received.iter().any(|words| words.contains(&told)), "chat heard the landing before replying: {received:?}");
}

fn check_authenticated_git(ran: &Ran) {
    let seen = ran.github();
    assert_eq!(seen.installations, [(github::INSTALLATION, 10, b"acme".to_vec())]);
    assert!(seen.minted >= 1);
    assert_eq!(seen.git_calls.iter().filter(|call| call.writes).count(), ONE_PUSH);
    assert!(seen.git_calls.iter().any(|call| !call.writes && call.status == 200));
    for call in &seen.git_calls {
        let minted = call.minted.expect("installation token on every git call");
        assert_eq!(minted.installation, github::INSTALLATION);
        assert!(minted.mint > 0 && minted.mint <= seen.minted);
        assert!(call.status == 200 || (!call.writes && call.status == 404), "{call:?}");
    }
}

fn scripts(at: std::net::SocketAddr) -> Box<[skein_fake_llm_domain::api::Script]> {
    let mut scripts = landing::scripts(at, u64::from(github::REPOSITORY), landing::README);
    scripts[0].turns[0].lines = Box::new([skein_fake_llm_domain::api::Line::Call {
        name: b"change".as_slice().into(),
        arguments: br#"{"words":"@coding Fix","repository":"acme/app"}"#.as_slice().into(),
    }]);
    scripts
}

fn no_installation_token_pattern_or_app_key_outside_the_secrets_store(ran: &Ran, files: &[std::path::PathBuf]) {
    use skein_lib::{
        Writer,
        base64::{self, Alphabet},
    };
    let pem = ran.github().app.as_ref().expect("converted App").pem.expose();
    let text = std::str::from_utf8(pem).expect("PEM text");
    let compact: String = text.lines().filter(|line| !line.starts_with("-----")).collect();
    let key = base64::decode(Alphabet::Standard, compact.as_bytes(), 4096).expect("converted key DER");
    let size = base64::encoded_len(Alphabet::Standard, u32::try_from(key.len()).expect("test key length"))
        .expect("base64 length");
    let mut encoded = Writer::new(usize::try_from(size).expect("base64 size"));
    base64::encode(Alphabet::Standard, &key, &mut encoded).expect("base64 key");
    let encoded = encoded.finish();
    let clean = |bytes: &[u8]| {
        assert!(
            !bytes.windows(40).any(|part| part.starts_with(b"ghs_") && part[4..].iter().all(u8::is_ascii_alphanumeric)),
            "minted-token pattern outside jig's secrets store"
        );
        assert!(skein_lib::bytes::find(bytes, pem).is_none(), "App PEM outside jig's secrets store");
        assert!(skein_lib::bytes::find(bytes, &key).is_none(), "App DER outside jig's secrets store");
        assert!(skein_lib::bytes::find(bytes, &encoded).is_none(), "App base64 outside jig's secrets store");
    };
    for path in files {
        clean(&std::fs::read(path).expect("scratch file"));
    }
    for visit in ran.browsed() {
        clean(&visit.body);
    }
    for call in ran.calls() {
        clean(&call.stdout);
        clean(&call.stderr);
    }
}

// Walk once for both checks, never following a symlink into the secrets store.
fn files_outside_the_secrets_store(ran: &Ran) -> Vec<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let secrets = ran.path().join("state/generations/1/secrets");
    assert!(secrets.is_dir(), "the excluded directory is jig's secrets store");
    // The person's original download is the sole deliberate plaintext input.
    // Exclude exactly this path; every other scratch file and all ctl output are scanned.
    let downloaded = ran.path().join("github-app.pem");
    assert_eq!(std::fs::metadata(&downloaded).expect("download").permissions().mode() & 0o777, 0o600);
    let mut files = Vec::new();
    let mut pending = vec![ran.path().to_path_buf()];
    while let Some(path) = pending.pop() {
        if path == secrets || path == downloaded {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).expect("scratch metadata");
        if metadata.is_dir() {
            pending.extend(
                std::fs::read_dir(path).expect("scratch directory").map(|entry| entry.expect("scratch entry").path()),
            );
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    files
}
