//! Adversarial GitHub real-loop QA from domain/testing.md, 2.4, story 6.
use skein_fake_llm_domain::api::{Finish, Line, Part, Script, Turn};
use std::{net::SocketAddr, path::Path};
use temper_real_loop_world::{
    Exit, Ran, argv,
    calls::{self, D1, Step, call, delegate, file, number, sign_in, task, until},
    github, landing, run_github,
};

fn adopt(owner: &'static str, name: &'static str, key: &'static str) -> Step {
    call(
        D1,
        argv!["adopt", "--project", "1", "--owner", owner, "--name", name, "--landing", "main", "--key", key, "--json"],
    )
}

fn no_agents(_: SocketAddr) -> Box<[Script]> {
    Box::new([])
}

fn account() -> Vec<Step> {
    vec![
        sign_in(D1),
        call(D1, argv!["add-key", "1", file("fixture.key"), "--deployment", "--key", "qa-llm"]),
        call(D1, argv!["wait", "accounts", "--until", "usable"]),
    ]
}

fn read_only(_: SocketAddr) -> Box<[Script]> {
    Box::new([Script {
        cue: b"@qa_read".as_slice().into(),
        turns: Box::new([
            Turn {
                lines: Box::new([Line::Call {
                    name: b"read".as_slice().into(),
                    arguments: br#"{"path":"README.md"}"#.as_slice().into(),
                }]),
                finish: Finish::ToolCalls,
                tokens: 1,
            },
            Turn {
                lines: Box::new([
                    Line::Text { text: b"Read without delivering.".as_slice().into() },
                    Line::Call { name: b"wait".as_slice().into(), arguments: b"{}".as_slice().into() },
                ]),
                finish: Finish::ToolCalls,
                tokens: 1,
            },
        ]),
    }])
}

fn delivered(at: SocketAddr) -> Box<[Script]> {
    landing::scripts(at, u64::from(github::REPOSITORY), landing::README)
}

fn successful(ran: &Ran, indices: &[usize]) {
    for &index in indices {
        assert_eq!(
            ran.calls()[index].exit,
            Some(Exit::Code(0)),
            "call {index}: stdout {}; stderr {}",
            String::from_utf8_lossy(&ran.calls()[index].stdout),
            String::from_utf8_lossy(&ran.calls()[index].stderr)
        );
    }
}

fn refused(ran: &Ran, index: usize) {
    assert_eq!(ran.calls()[index].exit, Some(Exit::Code(2)), "call {index}: {:?}", ran.calls()[index]);
    let answer =
        if ran.calls()[index].stdout.is_empty() { &ran.calls()[index].stderr } else { &ran.calls()[index].stdout };
    let _: serde_json::Value = serde_json::from_slice(answer).expect("typed adoption answer");
    let reason: String = String::from_utf8_lossy(answer)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    assert!(
        reason.contains("noaccess") || reason.contains("notinstalled"),
        "adoption must name the installation refusal: {}",
        String::from_utf8_lossy(answer)
    );
    assert_eq!(
        ran.github().installations,
        vec![(github::INSTALLATION, 10, b"acme".to_vec())],
        "a refused account must not acquire an installation"
    );
}

fn owner_authenticated(ran: &Ran) {
    assert!(!ran.github().git_calls.is_empty(), "the scenario must reach the authenticated git route");
    for (index, request) in ran.github().git_calls.iter().enumerate() {
        // Git probes a not-yet-created change branch before its first push.
        // That read may be 404; authentication failures may not be hidden by it.
        assert!(request.status == 200 || (!request.writes && request.status == 404), "git call {index}: {request:?}");
        let minted = request.minted.expect("every git call uses an installation token, never an App JWT");
        assert_eq!(minted.installation, github::INSTALLATION, "git call {index}: {request:?}");
        assert!(minted.mint > 0 && minted.mint <= ran.github().minted, "git call {index}: {request:?}");
    }
}

fn read_succeeded(ran: &Ran) {
    assert!(
        ran.llm().iter().any(|seen| {
            let skein_fake_peers::llm::Observation::Query { query, .. } = seen else { return false };
            query.messages.iter().flat_map(|message| message.parts.iter()).any(|part| {
                matches!(part, Part::ToolOutput { output, is_error: false, .. }
                    if calls::occurrences(output, b"# app") > 0)
            })
        }),
        "the agent must actually read the installed repository's README"
    );
}

fn read_steps(mut steps: Vec<Step>) -> Vec<Step> {
    let chat = steps.len();
    steps.push(call(D1, argv!["chat", "--project", "1", "--key", "read", "@qa_read Read the README"]));
    steps.push(call(D1, argv!["wait", "task", task(chat), "--until", "replied"]));
    steps
}

// Fresh deployment: chat T1, change T2, coder T3. The push run's chat is
// its call 4, and the tree read naming the coder its call 6.
const PUSH_CHAT: usize = 4;
const PROCEDURE: u64 = 2;
const CODER: u64 = 3;
const CODER_SEEN: usize = 6;

fn push_run() -> Ran {
    run_github(github::fixture(), delivered, || {
        let mut steps = account();
        steps.extend([
            adopt("acme", "app", "installed"),
            call(D1, argv!["chat", "--project", "1", "--key", "push", "@forge_chat Make one small edit"]),
            call(D1, argv!["wait", "task", task(PUSH_CHAT), "--until", "replied"]),
            // Fresh deployment: chat T1, change T2, coder T3. The coder's
            // terminal follows its delivery, without waiting for review/landing.
            // The procedure creates its coder after the chat can already park;
            // discover it before opening a watch on that task.
            until(D1, argv!["task", number(PROCEDURE), "--tree", "--json"], b"\"task\":3", b"\"task\":2"),
            call(D1, argv!["wait", "task", delegate(CODER_SEEN), "--until", "ended", "--json"]),
            call(D1, argv!["me", "--json"]),
            call(D1, argv!["task", number(CODER), "--json"]),
        ]);
        steps
    })
}

/// domain/testing.md, 2.4, story 6: refusal precedes every git call.
#[test]
fn uninstalled_adoption_leaves_no_git_or_installation() {
    let ran = run_github(github::fixture(), no_agents, || {
        vec![sign_in(D1), adopt("umbra", "tool", "uninstalled"), call(D1, argv!["me", "--json"])]
    });
    successful(&ran, &[0, 2]);
    refused(&ran, 1);
    assert_eq!(ran.github().git_after_call[1], 0, "no git before the refusal's answer");
    assert!(ran.github().git_calls.is_empty(), "refusal and shutdown must leave git untouched");
    assert!(ran.github().branches.is_empty(), "no branch created by a refused adoption");
    for entry in std::fs::read_dir(ran.path().join("worker-root")).expect("worker root") {
        let entry = entry.expect("worker scaffold");
        assert!(entry.file_type().expect("scaffold type").is_dir(), "no checkout files after refusal");
        assert!(
            std::fs::read_dir(entry.path()).expect("worker scaffold").next().is_none(),
            "no checkout after refusal"
        );
    }
}

/// protocol/credentials.md, 4; protocol/channel.md, 4 and 6: one account's installation grants no other account.
#[test]
fn installed_adoption_does_not_authorize_another_account() {
    let ran = run_github(github::fixture(), read_only, || {
        let mut steps = account();
        steps.extend([adopt("acme", "app", "installed"), adopt("umbra", "tool", "uninstalled")]);
        read_steps(steps)
    });
    successful(&ran, &[0, 1, 2, 3, 5, 6]);
    refused(&ran, 4);
    assert_eq!(ran.github().git_after_call[3], ran.github().git_after_call[4]);
    assert_eq!(ran.github().git_after_call[4], 0, "the refused account must not reach git");
    owner_authenticated(&ran);
    read_succeeded(&ran);
}

/// domain/testing.md, 2.4, story 6: a refusal must not obstruct a later valid adoption/run.
#[test]
fn refused_adoption_does_not_poison_a_later_installed_checkout() {
    let ran = run_github(github::fixture(), read_only, || {
        let mut steps = account();
        steps.extend([adopt("umbra", "tool", "uninstalled"), adopt("acme", "app", "installed")]);
        read_steps(steps)
    });
    refused(&ran, 3);
    successful(&ran, &[0, 1, 2, 4, 5, 6]);
    assert_eq!(ran.github().git_after_call[3], 0);
    owner_authenticated(&ran);
    read_succeeded(&ran);
}

/// protocol/channel.md, 4; protocol/credentials.md, 4: a read-only run authenticates reads and writes nothing remotely.
#[test]
fn read_only_chat_uses_the_owner_token_without_remote_writes() {
    let ran = run_github(github::fixture(), read_only, || {
        let mut steps = account();
        steps.push(adopt("acme", "app", "installed"));
        read_steps(steps)
    });
    successful(&ran, &[0, 1, 2, 3, 4, 5]);
    owner_authenticated(&ran);
    read_succeeded(&ran);
    assert!(ran.github().git_calls.iter().all(|request| !request.writes), "read-only chat made a git write");
    assert!(ran.github().branches.is_empty(), "read-only chat moved a remote branch");
}

/// domain/testing.md, 2.4, story 6; skein's fake-forge.md, 10.1: every git request carries the owner's minted token.
#[test]
fn delivered_change_authenticates_every_git_call_and_pushes_the_edit() {
    let ran = push_run();
    successful(&ran, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    owner_authenticated(&ran);
    assert!(ran.github().git_calls.iter().any(|request| request.writes), "delivery must make a remote write");
    let changed: Vec<_> = ran
        .github()
        .branches
        .iter()
        .filter(|(_, branch)| {
            branch
                .tree
                .get(b"README.md".as_slice())
                .is_some_and(|bytes| bytes.as_slice() == [landing::README, landing::EDIT].concat())
        })
        .collect();
    let [(name, branch)] = changed.as_slice() else {
        panic!("exactly one branch must hold the coder's edit: {:?}", ran.github().branches)
    };
    assert_ne!(name.as_slice(), b"main", "the coder must push its change branch");
    assert!(branch.head > 0);
}

fn no_git_secret(bytes: &[u8], place: &str) {
    // The fake's public mint contract is ghs_ plus 36 alphanumeric bytes.
    // The Basic prefix also catches an environment header accidentally persisted
    // after base64 encoding, where the raw token is no longer a substring.
    let key = skein_fake_forge_github::keys::KEYS[temper_github_app::APP.key];
    let key_line = key.split(|byte| *byte == b'\n').nth(1).expect("public fixture PEM body");
    for needle in [b"ghs_".as_slice(), b"eC1hY2Nlc3MtdG9rZW46Z2hz".as_slice(), key_line] {
        assert_eq!(calls::occurrences(bytes, needle), 0, "git secret exposed in {place}");
    }
}

fn scan_files(path: &Path, secrets: &Path, count: &mut usize) {
    let kind = std::fs::symlink_metadata(path).expect("scratch entry").file_type();
    if kind.is_dir() {
        for entry in std::fs::read_dir(path).expect("scratch directory") {
            scan_files(&entry.expect("scratch entry").path(), secrets, count);
        }
    } else if kind.is_file() {
        *count += 1;
        let bytes = std::fs::read(path).expect("scratch file");
        if path.starts_with(secrets) {
            // protocol/credentials.md, 4: installation tokens are memory-only, even
            // though the App's key is allowed in jig's secrets store.
            assert_eq!(calls::occurrences(&bytes, b"ghs_"), 0, "installation token persisted in jig's secrets");
        } else {
            no_git_secret(&bytes, path.to_str().expect("scratch path"));
        }
    }
}

/// protocol/credentials.md, 7; domain/testing.md, 6: no git token or App key in observable state outside jig's secrets.
#[test]
fn git_secrets_stay_out_of_files_client_answers_and_agent_context() {
    let ran = push_run();
    successful(&ran, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    owner_authenticated(&ran);
    assert!(ran.github().git_calls.iter().any(|request| request.writes), "scan must follow a real delivery");
    for result in ran.calls() {
        no_git_secret(&result.stdout, "ctl stdout");
        no_git_secret(&result.stderr, "ctl stderr");
    }
    for seen in ran.llm() {
        let skein_fake_peers::llm::Observation::Query { query, .. } = seen else { continue };
        no_git_secret(&query.system, "agent system prompt");
        for part in query.messages.iter().flat_map(|message| message.parts.iter()) {
            match part {
                Part::Opaque { bytes } => no_git_secret(bytes, "agent opaque context"),
                Part::Text { text } => no_git_secret(text, "agent text"),
                Part::ToolCall { id, name, arguments } => {
                    no_git_secret(id, "agent tool call id");
                    no_git_secret(name, "agent tool call name");
                    no_git_secret(arguments, "agent tool call arguments");
                }
                Part::ToolOutput { id, output, .. } => {
                    no_git_secret(id, "agent tool output id");
                    no_git_secret(output, "agent tool output");
                }
            }
        }
    }
    let mut files = 0;
    assert!(ran.path().join("state/generations/1/store").is_dir(), "main store is part of the scan");
    assert!(ran.path().join("state/transcripts").is_dir(), "agent transcripts are part of the scan");
    scan_files(ran.path(), &ran.path().join("state/generations/1/secrets"), &mut files);
    assert!(files > 0, "the scan must inspect persisted files");
}
