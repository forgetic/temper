//! QA-B: connected GitHub after pull creation, on the shipped real loop.
//! Design, public harness surface and explicitly allowed usage examples only.
use serde_json::{Value, json};
use skein_fake_forge_domain::{Observation, api};
use skein_fake_llm_domain::api::{Finish, Line, Part, Script, Turn};
use std::{fmt::Write, net::SocketAddr, path::Path};
use temper_real_loop_world::{
    Exit, Ran, argv,
    calls::{self, D1, Step, call, file, number, sign_in, task},
    github, landing, run_github, terminal,
};

const CHANGE: u64 = 2;
const CHAT_CALL: usize = 4;
const WORKER: u64 = 3;
const LANDING_HEARD: usize = 8;
const HELD_TOLD: usize = 10;
const ONE: usize = 1;
const LANDED: &[u8] = b"The change landed.";

fn fixture() -> github::Fixture {
    github::Fixture {
        configure: |value, agent, address| {
            // Keep the ordinary enforced CI, exact review and up-to-date rule.
            (landing::fixture().configure)(value, agent, address);
            value["engine"]["limits"]["overrides"]["documents.rows"] = 3.into();
            value["engine"]["templates"][0]["ended_by"] = "requester".into();
            // The generic fixture's chat is unlisted; these stories follow it on the person's page.
            value["engine"]["templates"][0]["listed"] = true.into();
        },
        ..github::fixture()
    }
}

fn scripts(address: SocketAddr) -> Box<[Script]> {
    let mut scripts = landing::scripts(address, u64::from(github::REPOSITORY), landing::README);
    // Keep the person's ordinary requester-ended chat open after its report.
    scripts[0].turns[2] = Turn {
        lines: Box::new([
            Line::Text { text: LANDED.into() },
            Line::Call { name: b"wait".as_slice().into(), arguments: b"{}".as_slice().into() },
        ]),
        finish: Finish::ToolCalls,
        tokens: 20,
    };
    scripts
}

fn begin() -> Vec<Step> {
    vec![
        sign_in(D1),
        call(D1, argv!["add-key", "1", file("fixture.key"), "--deployment", "--key", "qa-llm"]),
        call(D1, argv!["wait", "accounts", "--until", "usable"]),
        call(
            D1,
            argv![
                "adopt",
                "--project",
                "1",
                "--owner",
                "acme",
                "--name",
                "app",
                "--landing",
                "main",
                "--key",
                "qa-adopt"
            ],
        ),
        call(D1, argv!["chat", "--project", "1", "--key", "qa-chat", "@forge_chat Make one small edit"]),
    ]
}

fn landed_steps() -> Vec<Step> {
    let mut steps = begin();
    steps.extend([
        // The chat's committed first reply follows delegate creation. Opening
        // a watch on T2 before that would correctly answer unknown.
        call(D1, argv!["wait", "task", task(CHAT_CALL), "--until", "replied"]),
        call(D1, argv!["wait", "task", number(CHANGE), "--until", "ended", "--json"]),
        // Ended tasks retain a plain result; --tree is a live watch.
        call(D1, argv!["task", number(CHANGE), "--json"]),
        calls::until(D1, argv!["chat", task(CHAT_CALL), "--json"], LANDED, b"Change requested"),
        call(D1, argv!["task", number(CHANGE), "--json"]),
    ]);
    steps
}

fn succeeded(ran: &Ran) {
    for (index, result) in ran.calls().iter().enumerate() {
        assert_eq!(
            result.exit,
            Some(Exit::Code(0)),
            "call {index}: {} {}; pull observations {:?}; branch heads {:?}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
            ran.github().pulls,
            ran.github()
                .branches
                .iter()
                .map(|(name, branch)| (String::from_utf8_lossy(name), branch.head))
                .collect::<Vec<_>>()
        );
    }
}

fn document(ran: &Ran, at: usize) -> Value {
    serde_json::from_slice(&ran.calls()[at].stdout).expect("client JSON document")
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    calls::occurrences(bytes, needle) > 0
}

fn landed(ran: &Ran) -> u64 {
    succeeded(ran);
    let opened = ran.github().pulls.iter().filter(|seen| matches!(seen, Observation::Opened { .. })).count();
    let merged: Vec<_> = ran
        .github()
        .pulls
        .iter()
        .filter_map(
            |seen| if let Observation::Merged { head, commit, .. } = seen { Some((*head, *commit)) } else { None },
        )
        .collect();
    assert_eq!(opened, 1, "one pull through repeated client reads: {:?}", ran.github().pulls);
    assert_eq!(merged.len(), ONE, "one merge: {:?}", ran.github().pulls);
    let (head, commit) = merged[0];
    let topic: Vec<_> = ran.github().branches.iter().filter(|(name, _)| name.as_slice() != b"main").collect();
    assert_eq!(topic.len(), ONE, "one worker branch: {:?}", ran.github().branches);
    assert_eq!(topic[0].1.head, head, "merge exactly the worker's pushed head");
    let main = ran.github().branches.get(b"main".as_slice()).expect("observed main head after merge");
    assert_eq!(main.head, commit, "main is the observed merge commit");
    assert_eq!(main.tree.get(b"README.md".as_slice()), Some(&[landing::README, landing::EDIT].concat()));
    let mut sha = String::new();
    for byte in skein_fake_forge_domain::object_bytes(main.head) {
        write!(sha, "{byte:02x}").expect("format commit id");
    }
    let task = document(ran, 7);
    assert_eq!(task["chip"]["task"], CHANGE);
    assert_eq!(task["chip"]["phase"], "ended");
    let result = &task["result"]["value"]["change"];
    assert_eq!(result["kind"], 1, "landed change");
    assert_eq!(result["words"]["bytes"], sha, "change result names main's merge commit");
    for request in &ran.github().git_calls {
        assert!(request.status == 200 || (!request.writes && request.status == 404), "authenticated git: {request:?}");
        assert_eq!(request.minted.expect("installation token").installation, github::INSTALLATION);
    }
    assert!(ran.github().git_calls.iter().any(|request| request.writes));
    head
}

#[test]
fn checked_head_lands_once_and_repeated_reads_name_main() {
    // QA: domain/forge.md 8.2, 8.7, 8.8; domain/testing.md 2.4 story 1.
    let ran = run_github(fixture(), scripts, landed_steps);
    landed(&ran);
    let before = document(&ran, 7);
    let after = document(&ran, 9);
    let before = before.get("result").expect("first result field");
    let after = after.get("result").expect("repeated result field");
    assert!(!before.is_null(), "landed task has a result");
    assert_eq!(before, after, "terminal result survives repeated reads");
    assert!(contains(&ran.calls()[LANDING_HEARD].stdout, LANDED), "the chat hears the landing before replying");
    assert!(
        ran.github().app_calls.iter().any(|(_, path)| path.starts_with(b"/app/hook/deliveries")),
        "real puller must poll its delivery log"
    );
}

fn page_steps() -> Vec<Step> {
    let mut steps = landed_steps();
    steps.extend([
        terminal::open(),
        // client/README.md 3.1, 5.1: this person starts in project 1.
        // Wait for its selected control before opening the project's chats.
        terminal::see(b"Project 1 (pressed)"),
        terminal::see(b"Chats"),
        terminal::type_on(b"Chats", b""),
        terminal::see(b"T1"),
        terminal::type_on(b"T1", b""),
        terminal::see(LANDED),
        terminal::close(),
    ]);
    steps
}

#[test]
fn landed_chat_is_readable_on_the_persons_page() {
    // QA: domain/testing.md 2.4 story 1; domain/forge.md 8.2, 8.8.
    let ran = run_github(fixture(), scripts, page_steps);
    landed(&ran);
    assert_eq!(ran.terminal().exit, Some(Exit::Code(0)));
    assert!(ran.terminal().pages.iter().any(|page| contains(&page.bytes, LANDED)));
}

fn no_token(bytes: &[u8], where_: &str) {
    // The fake's public mint contract is ghs_ + 36 alphanumeric bytes.
    // This supplemental check deliberately makes no all-byte observer claim.
    for needle in [b"ghs_".as_slice(), b"eC1hY2Nlc3MtdG9rZW46Z2hz".as_slice()] {
        assert!(!contains(bytes, needle), "installation credential in {where_}");
    }
}

fn scan(path: &Path) -> usize {
    // Never open keys, credential records or auth.json, including fixture ones.
    let name = path.file_name().expect("scratch path").to_string_lossy();
    if matches!(name.as_ref(), "secrets" | "accounts" | "auth.json")
        || name.ends_with(".key")
        || name.ends_with(".pem")
        || name.ends_with(".secret")
    {
        return 0;
    }
    let kind = std::fs::symlink_metadata(path).expect("scratch metadata").file_type();
    if kind.is_dir() {
        std::fs::read_dir(path).expect("scratch directory").map(|entry| scan(&entry.expect("entry").path())).sum()
    } else if kind.is_file() {
        no_token(&std::fs::read(path).expect("noncredential scratch file"), &path.display().to_string());
        1
    } else {
        0
    }
}

#[test]
fn available_client_pages_context_and_nonsecret_store_have_no_token() {
    // QA: protocol/credentials.md 4, 5, 7; draft/github.md 17.
    let ran = run_github(fixture(), scripts, page_steps);
    landed(&ran);
    for result in ran.calls() {
        no_token(&result.stdout, "client stdout");
        no_token(&result.stderr, "client stderr");
    }
    assert!(!ran.terminal().pages.is_empty(), "page scan exercised");
    for page in &ran.terminal().pages {
        no_token(&page.bytes, "printed page");
    }
    for line in &ran.terminal().lines {
        no_token(line, "terminal diagnostic");
    }
    for seen in ran.llm() {
        let skein_fake_peers::llm::Observation::Query { query, .. } = seen else { continue };
        no_token(&query.system, "agent system text");
        for part in query.messages.iter().flat_map(|message| message.parts.iter()) {
            match part {
                Part::Text { text } => no_token(text, "agent text"),
                Part::Opaque { bytes } => no_token(bytes, "opaque context"),
                Part::ToolCall { id, name, arguments } => {
                    no_token(id, "call id");
                    no_token(name, "tool name");
                    no_token(arguments, "tool arguments");
                }
                Part::ToolOutput { id, output, .. } => {
                    no_token(id, "answer id");
                    no_token(output, "tool answer");
                }
            }
        }
    }
    assert!(ran.path().join("state/generations/1/store").is_dir());
    assert!(scan(&ran.path().join("state/generations/1/store")) > 0, "persisted store scan exercised");
    assert!(scan(&ran.path().join("state/transcripts")) > 0, "transcript scan exercised");
    scan(&ran.path().join("worker-root"));
}

fn tool(name: &[u8], arguments: &Value) -> Turn {
    Turn {
        lines: Box::new([Line::Call {
            name: name.into(),
            arguments: serde_json::to_vec(arguments).expect("tool JSON").into(),
        }]),
        finish: Finish::ToolCalls,
        tokens: 20,
    }
}

fn repair_scripts(address: SocketAddr) -> Box<[Script]> {
    let mut scripts = scripts(address).into_vec();
    scripts[1].turns[1] =
        tool(b"edit", &json!({"path":"README.md","edits":[{"old":"# app\n","new":"# app\nQA failing head\n"}]}));
    scripts.push(Script {
        cue: b"CI".as_slice().into(),
        turns: Box::new([
            tool(b"read", &json!({"path":"README.md"})),
            tool(
                b"edit",
                &json!({"path":"README.md","edits":[{"old":"QA failing head\n","new":"Changed by temper.\n"}]}),
            ),
            tool(b"finish", &json!({"form":"change","fields":{"title":"Repair CI","body":"Fixed failing head."}})),
        ]),
    });
    scripts.into_boxed_slice()
}

#[test]
#[ignore = "needs: GitHub Fixture repository Setup hook with content-cued failing/passing CI and per-head check observations"]
fn q1_failed_check_worker_repair_rechecked_head_lands() {
    // QA: domain/forge.md 8.2-8.4, 8.7; domain/testing.md 2.4.
    // Required fixture: Checks::cue File(README.md, green=landing::EDIT),
    // so the first coder push fails and only the repair push can pass.
    let _required_checks = api::Checks {
        cue: Some(api::Cue::File { path: b"README.md".as_slice().into(), green: landing::EDIT.into() }),
        ..(temper_real_loop_world::forge::fixture().setup)().checks
    };
    let ran = run_github(fixture(), repair_scripts, landed_steps);
    let repaired = landed(&ran);
    assert_eq!(ran.github().branches[b"main".as_slice()].head, repaired);
    assert!(
        ran.llm().iter().any(|seen| {
            let skein_fake_peers::llm::Observation::Query { query, .. } = seen else { return false };
            contains(&query.system, b"repair")
        }),
        "the worker must actually run the repair, with its failing brief"
    );
    // Needs the per-head CI observer to assert failed(old), passed(repaired)
    // in that order before merge; final green content alone is insufficient.
}

const STALL_MS: u64 = 800;
const HELD_REPLY: &[u8] = b"The change is held while CI has not reported.";

fn silent_fixture() -> github::Fixture {
    github::Fixture {
        setup: || {
            let mut setup = (github::fixture().setup)();
            setup.checks.silent = 1000;
            setup
        },
        configure_domain: |config| {
            config.limits.observations = 64;
            config.settle_min = skein_lib::Duration::ZERO;
            config.settle_max = skein_lib::Duration::ZERO;
        },
        configure: |value, agent, address| {
            (fixture().configure)(value, agent, address);
            // QA: protocol/configuration.md 7, dotted duration overrides;
            // preserve the shipped rule, shortening only the wait for this story.
            value["application"]["limits"]["overrides"]["forge.change_policy.stall_ms"] = STALL_MS.into();
        },
        ..github::fixture()
    }
}

fn silent_scripts(address: SocketAddr) -> Box<[Script]> {
    let mut scripts = scripts(address);
    scripts[0].turns[2].lines[0] = Line::Text { text: HELD_REPLY.into() };
    scripts
}

fn silent_steps() -> Vec<Step> {
    let mut steps = begin();
    steps.extend([
        // Synchronize on actual task creation and the worker's completed push,
        // then observe both sides of the short stall through client reads.
        call(D1, argv!["wait", "task", task(CHAT_CALL), "--until", "replied"]),
        call(D1, argv!["wait", "task", number(WORKER), "--until", "ended", "--timeout", "2s"]),
        call(D1, argv!["task", number(CHANGE), "--json"]),
        call(D1, argv!["wait", "task", number(CHANGE), "--until", "held", "--timeout", "2s", "--json"]),
        call(D1, argv!["task", number(CHANGE), "--json"]),
        calls::until(D1, argv!["chat", task(CHAT_CALL), "--json"], HELD_REPLY, b"Change requested"),
        // Reading the service again after the hold also checks it still serves.
        call(D1, argv!["me", "--json"]),
        terminal::open(),
        terminal::see(b"Project 1 (pressed)"),
        terminal::see(b"Chats"),
        terminal::type_on(b"Chats", b""),
        terminal::see(b"T1"),
        terminal::type_on(b"T1", b""),
        terminal::see(HELD_REPLY),
        terminal::see(b"T2"),
        terminal::type_on("Task 2 ↗".as_bytes(), b""),
        terminal::see(b"Release"),
        terminal::close(),
    ]);
    steps
}

#[test]
fn q2_never_reporting_check_holds_and_tells_chat_and_page() {
    // QA: domain/forge.md 8.4, 8.8; client/forge.md 2.4-2.5.
    let ran = run_github(silent_fixture(), silent_scripts, silent_steps);
    succeeded(&ran);
    let before = document(&ran, 7);
    assert_eq!(before["chip"]["task"], CHANGE, "live change's identity: {before}");
    assert!(before.get("hold").expect("hold field").is_null(), "not held immediately after producing: {before}");
    let held = document(&ran, 9);
    assert_eq!(held["chip"]["task"], CHANGE, "held change's identity: {held}");
    assert_eq!(held["chip"]["phase"], "held", "stall holds the change: {held}");
    assert!(!held.get("hold").expect("hold field").is_null(), "hold has a reason: {held}");
    let chat_queries: Vec<_> = ran
        .llm()
        .iter()
        .filter_map(|seen| match seen {
            skein_fake_peers::llm::Observation::Query { query, .. } if contains(&query.system, b"@forge_chat") => {
                Some(query)
            }
            skein_fake_peers::llm::Observation::Accepted { .. }
            | skein_fake_peers::llm::Observation::Head { .. }
            | skein_fake_peers::llm::Observation::Credential { .. }
            | skein_fake_peers::llm::Observation::Query { .. }
            | skein_fake_peers::llm::Observation::Answered { .. }
            | skein_fake_peers::llm::Observation::Closed { .. } => None,
        })
        .collect();
    let wake = chat_queries.get(2).expect("requester's chat wakes after delegating and parking");
    let notice: Vec<_> = wake
        .messages
        .iter()
        .flat_map(|message| message.parts.iter())
        .filter_map(|part| match part {
            Part::Text { text } | Part::ToolOutput { output: text, .. } => Some(text.as_ref()),
            Part::Opaque { .. } | Part::ToolCall { .. } => None,
        })
        .flatten()
        .map(u8::to_ascii_lowercase)
        .collect();
    assert!(contains(&ran.calls()[HELD_TOLD].stdout, HELD_REPLY), "requester wakes and tells the person");
    assert_eq!(ran.terminal().exit, Some(Exit::Code(0)));
    assert!(ran.terminal().pages.iter().any(|page| contains(&page.bytes, HELD_REPLY)));
    assert!(
        ran.terminal().pages.iter().any(|page| {
            let words = page.bytes.to_ascii_lowercase();
            contains(&words, b"stall") && contains(&words, b"release")
        }),
        "the held change's page says why, with its release act"
    );
    assert_eq!(ran.github().pulls.iter().filter(|row| matches!(row, Observation::Opened { .. })).count(), 1);
    assert!(!ran.github().pulls.iter().any(|row| matches!(row, Observation::Merged { .. })));
    assert!(ran.github().git_calls.iter().any(|request| request.writes), "worker pushed a change");
    let records = ran.github().observations();
    assert!(
        !records
            .iter()
            .any(|row| matches!(row, Observation::Reported { state: api::Check::Passed | api::Check::Failed, .. })),
        "silent CI never produced a terminal report: {records:?}"
    );
    assert!(
        contains(&notice, b"held task needs a decision") && contains(&notice, b"2"),
        "requester receives the change's held-task notice before its scripted reply: {}",
        String::from_utf8_lossy(&notice)
    );
}

#[test]
#[ignore = "needs: fake-issued raw token observer across all logs, store records, raw client wire and every page; refresh synchronization"]
fn q7_g6_whole_run_and_refresh_observe_every_token_byte() {
    // QA: protocol/credentials.md 4, 5, 7; draft/github.md 17.
    // This existing run supplies the entire path to the missing observer.
    // Mint ids and a ghs_ prefix scan cannot establish the byte-level promise.
    let ran = run_github(fixture(), scripts, page_steps);
    landed(&ran);
    assert!(ran.github().minted >= 2, "whole-run observer includes renewal");
    panic!("needs: raw fake-issued token bytes and complete boundary observer; do not substitute prefix scans");
}

#[test]
fn pull_watch_wakes_landing_before_the_slow_poll() {
    // QA: draft/github.md 7; domain/forge.md 5.2, 8.2;
    // domain/testing.md 2.4, 6.1: a whole GitHub chat has a 3 s guide.
    // An absent pull watch or lost wake must not hide behind a 10 ms poll.
    let fixture = github::Fixture {
        configure: |value, agent, address| {
            (fixture().configure)(value, agent, address);
            for name in ["poll_ms", "poll_max_ms", "slow_ms"] {
                value["application"]["limits"]["overrides"][format!("forge.client.{name}")] = 5000.into();
            }
        },
        ..github::fixture()
    };
    #[expect(
        clippy::disallowed_types,
        reason = "this external QA timer includes setup and drain in the real whole-chat wall-time guide"
    )]
    let start = std::time::Instant::now();
    let ran = run_github(fixture, scripts, landed_steps);
    let elapsed = start.elapsed();
    landed(&ran);
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "pull watch must wake within the whole-chat guide, before slow polling: {elapsed:?}"
    );
}

#[test]
#[ignore = "needs: real-loop per-request GitHub pause/release hook for a required brief read across fixture-short gather"]
fn an_unanswered_repair_brief_is_bounded_by_its_gather_deadline() {
    // QA: domain/root.md 7.2, 10: gather (shipped 30 s) bounds a stuck
    // connector; a missing required section never becomes a partial brief.
    // The public GitHub setup/timing callbacks cannot pause a request.
    // Needs: hold CI-log or PullFiles after preparation starts, through a
    // short gather; take the following reads while it remains held, then
    // release it. Expiry's exact task phase is not specified by the design.
    let ran = run_github(fixture(), repair_scripts, || {
        let mut steps = begin();
        steps.extend([
            call(D1, argv!["wait", "task", task(CHAT_CALL), "--until", "replied"]),
            call(D1, argv!["me", "--json"]),
            call(D1, argv!["task", number(CHANGE), "--json"]),
        ]);
        steps
    });
    succeeded(&ran);
    assert!(!ran.github().pulls.iter().any(|seen| matches!(seen, Observation::Merged { .. })));
    assert!(
        !ran.llm().iter().any(|seen| matches!(seen,
            skein_fake_peers::llm::Observation::Query { query, .. }
                if contains(&query.system, b"Repair the failed CI on the change branch")
        )),
        "no repair assignment while its required section is missing"
    );
    panic!("needs: real-loop required-brief read pause/release and gather-boundary synchronization");
}
