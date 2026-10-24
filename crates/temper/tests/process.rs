//! The shipped binary, on a scratch deployment, drained on a signal.
pub mod support;
use skein_io::kernel::{Exit, Signal};
use skein_lib::Duration;
use support::forge_land::{
    assert_cached_binary_unchanged, assert_forgejo_setup, assert_plan_box, cache_forgejo_binaries, deployment_flags,
    forgejo_setup_steps, plan_change_steps,
};

// Each whole-example box story is bounded at 120 seconds.
const BOX_DEADLINE: Duration = Duration::from_secs(240);

#[test]
fn a_held_clients_listener_stops_serve() {
    held_listener_stops_serve("clients", "temper serve: client listener failed\n");
}

#[test]
fn a_held_webhooks_listener_stops_serve() {
    held_listener_stops_serve("webhooks", "temper serve: webhooks listener failed\n");
}

#[test]
fn a_held_workers_listener_stops_serve() {
    held_listener_stops_serve("workers", "temper serve: client listener failed\n");
}

fn held_listener_stops_serve(listener: &str, expected: &str) {
    use skein_world::end_to_end::Command;
    let scratch = support::Scratch::new(&format!("held_{listener}_listener_stop"));
    support::configuration(scratch.path(), &support::Forge::Unreached);
    // The kernel draws both ports; the held one stays bound until serve has stopped.
    let held = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("held port");
    let path = scratch.path().join("temper.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("fixture document")).expect("fixture JSON");
    let free = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("free port");
    document["application"]["clients"]["listener"]["socket"] =
        free.local_addr().expect("free address").to_string().into();
    let address = serde_json::json!({"socket": held.local_addr().expect("held address").to_string()});
    match listener {
        "clients" => document["application"]["clients"]["listener"] = address,
        "webhooks" => document["application"]["webhooks"] = serde_json::json!({"listener": address}),
        "workers" => {
            document["engine"]["hosts"]["listener"] = serde_json::json!([address]);
            document["workers"] = serde_json::json!([]);
            document["application"]["workers"] = serde_json::json!([]);
        }
        _ => unreachable!(),
    }
    drop(free);
    std::fs::write(path, serde_json::to_vec(&document).expect("held-listener JSON")).expect("held-listener fixture");
    let ended = support::boxed::run_command(
        Command {
            program: env!("CARGO_BIN_EXE_temper").into(),
            arguments: std::iter::once("serve".into()).chain(support::configuration::flags(scratch.path())).collect(),
            directory: scratch.path().into(),
            environment: Vec::new(),
        },
        None,
        support::boxed::Plan::Stop,
        Duration::from_secs(5),
    );
    let diagnostic = String::from_utf8_lossy(&ended.stderr);
    assert_eq!(ended.exit, Some(Exit::Code(1)), "{diagnostic}");
    assert!(diagnostic.contains(expected), "{diagnostic}");
    assert_eq!(ended.left, Some(Vec::new()), "no surviving child: {diagnostic}");
    scratch.remove();
}

#[test]
fn a_temper_told_to_stop_with_a_sign_in_under_way_ends_at_once() {
    let scratch = support::Scratch::new("sign_in_stop");
    let issuer = support::sign_in::configuration(scratch.path());
    support::sign_in::run(scratch.path(), issuer);
    scratch.remove();
}

#[test]
fn an_account_moves_between_two_deployments_and_the_first_holds_nothing() {
    let clock = skein_shell::Clock::new();
    let started = clock.now().now;
    let scratch = support::Scratch::new(&format!("account_move_{}", std::process::id()));
    support::account_move::run(scratch.path());
    scratch.remove();
    eprintln!("account move story: {:?}", clock.now().now.saturating_since(started));
}

#[test]
fn a_person_signs_in_with_chatgpt_and_a_chat_on_the_plan_lasts() {
    let scratch = support::Scratch::new("sign_in_chat");
    let issuer = support::sign_in::configuration(scratch.path());
    support::sign_in::answered(scratch.path(), issuer);
    scratch.remove();
}

#[test]
fn a_welcome_sign_in_with_chatgpt_returns_to_ready_and_its_account_survives_a_restart() {
    let scratch = support::Scratch::new("welcome_sign_in");
    let issuer = support::sign_in::configuration(scratch.path());
    let clock = skein_shell::Clock::new();
    let started = clock.now().now;
    support::sign_in_welcome::run(scratch.path(), issuer);
    eprintln!("welcome sign-in and restart: {:?}", clock.now().now.saturating_since(started));
    scratch.remove();
}

#[test]
fn a_plan_chat_whose_reasoning_passes_its_allowance_is_held_with_its_reason() {
    let scratch = support::Scratch::new("plan_chat_reasoning_bound");
    let issuer = support::sign_in::configuration(scratch.path());
    support::sign_in::exceeded(scratch.path(), issuer);
    scratch.remove();
}

#[test]
fn a_first_owner_adds_a_key_under_the_deployments_accounts() {
    let scratch = support::Scratch::new("deployment_accounts");
    let _issuer = support::sign_in::configuration(scratch.path());
    let clock = skein_shell::Clock::new();
    let started = clock.now().now;
    let seen = support::run_box(scratch.path(), support::person::deployment_account(), support::WITHIN);
    let elapsed = clock.now().now.saturating_since(started);
    assert!(seen.finished, "the first owner saw the deployment's account");
    assert!(elapsed < Duration::from_secs(5), "accounts story took {elapsed:?}");
    eprintln!("deployment accounts story: {elapsed:?}");
    scratch.remove();
}

fn ctl_scratch(case: &str) -> support::Scratch {
    let scratch = support::Scratch::new(case);
    support::configuration(scratch.path(), &support::Forge::Unreached);
    scratch
}

#[test]
fn temper_ctl_help_prints_the_guide() {
    let scratch = ctl_scratch("ctl_help");
    let ended = support::ctl::alone(scratch.path(), support::ctl::Call::bare(&["help"]));
    assert_eq!(ended.exit, Some(Exit::Code(0)));
    assert!(ended.stdout.starts_with(b"Agent tools\n"));
    assert_eq!(ended.stderr, b"");
    scratch.remove();
}

#[test]
fn temper_ctl_without_a_tool_exits_1() {
    let scratch = ctl_scratch("ctl_usage");
    let ended = support::ctl::alone(scratch.path(), support::ctl::Call::new(&[]));
    assert_eq!(ended.exit, Some(Exit::Code(1)));
    assert!(ended.stderr.starts_with(b"temper ctl: "));
    let line = ended.stderr.strip_suffix(b"\n").expect("terminated diagnostic");
    assert!(!line.contains(&b'\n'), "one diagnostic line");
    scratch.remove();
}

#[test]
fn temper_ctl_signs_in_by_the_link_read_on_stdin() {
    use support::ctl::{Call, Step, Story};
    let scratch = ctl_scratch("ctl_sign_in");
    let steps = vec![Step::Run(Call::sign_in()), Step::Run(Call::new(&["me", "--json"]))];
    let seen = support::ctl::run(Story::new(scratch.path(), steps));
    let [signed, me] = seen.calls.as_slice() else { panic!("two ctl ends") };
    assert_eq!(signed.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&signed.stderr));
    let document = support::ctl::json(me);
    assert_eq!((me.exit, document["party"]["number"].is_u64()), (Some(Exit::Code(0)), true), "{document}");
    scratch.remove();
}

#[test]
fn a_fresh_link_keeps_the_sign_in_held() {
    use support::ctl::{Call, Step, Story};
    let scratch = ctl_scratch("fresh_link_keeps_sign_in");
    let clock = skein_shell::Clock::new();
    let mut mismatches = Vec::new();
    let mut first_party = None;
    let mut first_token = Vec::new();
    // Three native runs are one more than the configuration's people.sign_ins pool holds (2).
    for run in 1..=3 {
        let began = clock.now().now;
        let seen = support::ctl::run(Story::new(
            scratch.path(),
            vec![Step::Run(Call::sign_in()), Step::Run(Call::new(&["me", "--json"]))],
        ));
        eprintln!("fresh link serve run {run}: {:?}", clock.now().now.saturating_since(began));
        let [signed, me] = seen.calls.as_slice() else { panic!("sign-in and me each ended") };
        if signed.exit != Some(Exit::Code(0)) {
            mismatches.push(format!(
                "run {run}: sign-in exited {:?}, expected Code(0); stderr: {}",
                signed.exit,
                String::from_utf8_lossy(&signed.stderr).trim_end()
            ));
        }
        let document = serde_json::from_slice::<serde_json::Value>(&me.stdout).unwrap_or_default();
        let party = document["party"]["number"].as_u64();
        if me.exit != Some(Exit::Code(0)) || party.is_none() || run > 1 && party != first_party {
            mismatches.push(format!(
                "run {run}: me exited {:?}, party {party:?}, expected Code(0) and {}; stderr: {}",
                me.exit,
                first_party.map_or_else(|| "a party number".into(), |number| format!("party {number}")),
                String::from_utf8_lossy(&me.stderr).trim_end()
            ));
        }
        let token = fs_read(&scratch.path().join("client/token"));
        if token.len() != 40 {
            mismatches.push(format!("run {run}: client/token has {} bytes, expected 40", token.len()));
        }
        if run == 1 {
            first_party = party;
            // All 40 bytes: the big-endian sign-in number followed by its 32-byte random secret.
            first_token = token;
        } else if token != first_token {
            mismatches.push(format!("run {run}: client/token differs from run 1 (sign-in number or secret changed)"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    scratch.remove();
}

#[test]
fn a_full_pool_says_so_and_the_same_link_signs_in_once_one_ends() {
    use support::ctl::{Call, Step, Story};
    const FULL_CTL: &str = "temper ctl: all 2 sign-ins are in use; the earliest ends at {T}; this link stays usable until {U}; after that, temper serve prints a fresh one when it is tried\n";
    const FULL_OPEN: &str = "temper open: all 2 sign-ins are in use; the earliest ends at {T}; this link stays usable until {U}; after that, temper serve prints a fresh one when it is tried\n";
    const REFUSED: &[u8] = b"temper ctl: the sign-in link was refused\n";
    let scratch = ctl_scratch("full_pool_same_link");
    let sign_in = |name| Step::Run(Call::sign_in().in_directory(name));
    let sign_out = |name| Step::Run(Call::new(&["sign-out"]).in_directory(name));
    let open = || Step::Open { directory: "d", until: None };
    let seen = support::ctl::run(
        Story::new(
            scratch.path(),
            vec![
                Step::Mark("start"),
                sign_in("a"),
                Step::Mark("pool step 1"),
                sign_in("b"),
                Step::NextLink,
                Step::Mark("pool step 2"),
                sign_in("b"),
                Step::Mark("pool step 3"),
                sign_in("c"),
                Step::NextLink,
                Step::Mark("pool step 4"),
                sign_in("c"),
                Step::Mark("pool step 5"),
                open(),
                Step::Mark("pool step 6"),
                sign_out("a"),
                Step::Mark("pool step 7"),
                sign_in("c"),
                Step::Run(Call::new(&["me", "--json"]).in_directory("c")),
                Step::Mark("pool step 8"),
                sign_in("e"),
                Step::NextLink,
                Step::Mark("pool step 9"),
                open(),
                Step::Mark("pool step 10"),
                sign_out("b"),
                Step::Mark("pool step 11"),
                Step::Open { directory: "d", until: Some(b"== Inbox == Live") },
                Step::Mark("pool step 12"),
            ],
        )
        .measured(&["c", "d"]),
    );
    let mut mismatches = seen.mismatches.clone();
    if seen.calls.len() != 13 {
        mismatches.push(format!("story: {} calls ended, expected 13", seen.calls.len()));
        scratch.remove();
        panic!("{}", mismatches.join("\n"));
    }
    // CLI host::fail finishes with status 1, including a refused sign-in.
    for (index, step, status, stderr) in [
        (0, "1", 0, b"".as_slice()),
        (1, "2", 1, REFUSED),
        (2, "3", 0, b""),
        (3, "4", 1, REFUSED),
        (6, "7", 0, b""),
        (7, "8 sign-in", 0, b""),
        (8, "8 me", 0, b""),
        (9, "9", 1, REFUSED),
        (11, "11", 0, b""),
        (12, "12", 0, b""),
    ] {
        pool_call(&seen.calls[index], step, status, stderr, &mut mismatches);
    }
    // The tier's engine fixture selects Small; configuration overrides the pool, not its lifetime.
    // jig's protocol-config profile/small.rs supplies people.sign_in_lifetime (24 hours).
    let profile = jig_protocol_config::profile(jig_protocol_config::Profile::Small);
    let full = |index: usize, step: &str, words, owner, link, mismatches: &mut Vec<String>| {
        pool_full(
            &seen.calls[index],
            step,
            words,
            [seen.spans[owner], seen.link_spans[link]],
            [profile.core.people.sign_in_lifetime, profile.clients.link_span],
            mismatches,
        )
    };
    // jig's protocol-config profile/small.rs supplies clients.link_span (five minutes).
    // link_spans encloses the call that made serve print the link and the step that waited for it.
    let a_ctl = full(4, "5", FULL_CTL, 0, 1, &mut mismatches);
    let a_open = full(5, "6", FULL_OPEN, 0, 1, &mut mismatches);
    if let (Some(ctl), Some(open)) = (a_ctl, a_open)
        && ctl != open
    {
        mismatches.push(format!("step 6: expiries {open:?} differ from step 5's {ctl:?}"));
    }
    full(10, "10", FULL_OPEN, 2, 2, &mut mismatches);
    for (index, step, token) in [(4, "5", 0), (5, "6", 1)] {
        if seen.tokens[index][token].is_some() {
            mismatches.push(format!("step {step}: {}/token exists", ["c", "d"][token]));
        }
    }
    pool_signed_in(&seen, scratch.path(), &mut mismatches);
    scratch.remove();
    // Sort by the story's numbered steps, so its first failure is the first broken promise.
    mismatches.sort_by_key(|line| {
        line.strip_prefix("step ")
            .and_then(|s| s.split([':', ' ']).next())
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0)
    });
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

fn pool_signed_in(seen: &support::ctl::Seen, scratch: &std::path::Path, mismatches: &mut Vec<String>) {
    let me = serde_json::from_slice::<serde_json::Value>(&seen.calls[8].stdout).unwrap_or_default();
    if !me["party"]["number"].is_u64() {
        mismatches.push("step 8: me --json has no party number".into());
    }
    if skein_lib::bytes::find(&seen.calls[12].stdout, b"== Inbox == Live").is_none() {
        mismatches.push("step 12: open did not print == Inbox == Live".into());
    }
    if seen.tokens[12][1] != Some(0o600) || !support::private(&scratch.join("d/token")) {
        mismatches.push("step 12: d/token is not kept 0o600".into());
    }
}

fn pool_call(ended: &support::Ended, step: &str, status: u8, stderr: &[u8], mismatches: &mut Vec<String>) {
    if ended.exit != Some(Exit::Code(status)) || ended.stderr != stderr {
        mismatches.push(format!(
            "step {step}: exit {:?}, stderr {:?}; expected Code({status}), {:?}",
            ended.exit,
            String::from_utf8_lossy(&ended.stderr),
            String::from_utf8_lossy(stderr)
        ));
    }
}

fn pool_full(
    ended: &support::Ended,
    step: &str,
    words: &str,
    windows: [(skein_lib::Wall, skein_lib::Wall); 2],
    spans: [Duration; 2],
    mismatches: &mut Vec<String>,
) -> Option<(u64, u64)> {
    let (prefix, rest) = words.split_once("{T}").expect("one sign-in expiry hole");
    let (middle, suffix) = rest.split_once("{U}").expect("one link expiry hole");
    let times = ended.stderr.strip_prefix(prefix.as_bytes()).and_then(|rest| rest.strip_suffix(suffix.as_bytes()));
    let parsed = times.and_then(|times| {
        let second = times.get(20..)?.strip_prefix(middle.as_bytes())?;
        Some((pool_utc_second(times.get(..20)?)?, pool_utc_second(second)?))
    });
    if ended.exit != Some(Exit::Code(1)) || parsed.is_none() {
        mismatches.push(format!(
            "step {step}: exit {:?}, stderr {:?}; expected Code(1), {words:?} with T and U in UTC to the second",
            ended.exit,
            String::from_utf8_lossy(&ended.stderr)
        ));
    }
    if let Some((sign_in, link)) = parsed {
        for ((name, second), ((began, end), span)) in
            [("T", sign_in), ("U", link)].into_iter().zip(windows.into_iter().zip(spans))
        {
            let first = (began.as_nanos() + span.as_nanos()) / 1_000_000_000;
            let last = (end.as_nanos() + span.as_nanos()) / 1_000_000_000;
            if second < first || second > last {
                mismatches
                    .push(format!("step {step}: {name} expiry {second} outside [{first}, {last}] (step plus span)"));
            }
        }
    }
    parsed
}

/// Strict UTC, Gregorian calendar, no parser dependency or local timezone.
fn pool_utc_second(text: &[u8]) -> Option<u64> {
    if text.len() != 20 || [text[4], text[7], text[10], text[13], text[16], text[19]] != *b"--T::Z" {
        return None;
    }
    let number = |range: std::ops::Range<usize>| {
        text[range].iter().try_fold(0_u64, |n, b| b.is_ascii_digit().then(|| n * 10 + u64::from(b - b'0')))
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let leap = |y| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let lengths = [31, if leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if year < 1970
        || !(1..=12).contains(&month)
        || day == 0
        || day > lengths[usize::try_from(month - 1).ok()?]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let years = (1970..year).map(|y| if leap(y) { 366 } else { 365 }).sum::<u64>();
    let months = lengths[..usize::try_from(month - 1).ok()?].iter().sum::<u64>();
    Some(((years + months + day - 1) * 24 + hour) * 3600 + minute * 60 + second)
}

#[test]
fn a_wait_past_its_timeout_exits_3() {
    use support::ctl::{Call, Step, Story};
    let scratch = ctl_scratch("ctl_timeout");
    let seen = support::ctl::run(Story::new(
        scratch.path(),
        vec![
            Step::Run(Call::sign_in()),
            Step::Run(Call::new(&["me", "--json"])),
            Step::Run(Call::new(&["wait", "me", "--timeout", "1", "--json"])),
        ],
    ));
    let [_, me, wait] = seen.calls.as_slice() else { panic!("three ctl ends") };
    assert_eq!(wait.exit, Some(Exit::Code(3)), "{}", String::from_utf8_lossy(&wait.stderr));
    assert_eq!(support::ctl::json(wait)["me"]["digest"], support::ctl::json(me)["digest"]);
    scratch.remove();
}

#[test]
fn an_adopt_cut_by_a_signal_is_made_once() {
    use support::ctl::{Call, PROJECT, Step, Story, json};
    let scratch = support::Scratch::new("ctl_cut");
    let forgejo = support::forgejo::Throwaway::start(&scratch);
    support::configuration(scratch.path(), &forgejo.forge());
    let adopt = &[
        "adopt",
        "--project",
        PROJECT,
        "--owner",
        "acme",
        "--name",
        "widget",
        "--landing",
        "main",
        "--key",
        "widget",
        "--json",
    ];
    let seen = support::ctl::run(Story::on(
        scratch.path(),
        &forgejo,
        vec![
            Step::Run(Call::sign_in()),
            Step::Run(Call::new(&["me", "--json"])),
            Step::Pause,
            Step::Start(Call::new(adopt)),
            Step::Until(Call::new(&["pending"]), b"adopt"),
            Step::Cut,
            Step::Run(Call::new(&["pending", "--json"])),
            Step::Resume,
            Step::Run(Call::new(adopt)),
            Step::Run(Call::new(&["pending"])),
        ],
    ));
    let [_, me, cut, _, listed, again, empty] = seen.calls.as_slice() else { panic!("seven ctl ends") };
    assert_eq!((cut.exit, cut.stdout.as_slice()), (Some(Exit::Code(143)), b"".as_slice()), "128 + SIGTERM");
    let (party, listed) = (json(me)["party"]["number"].as_u64().expect("signed in"), json(listed));
    let first_sent = listed["pending"][0]["first_sent"].clone();
    assert!(first_sent.is_u64() || first_sent.is_null(), "{listed}");
    let row = serde_json::json!({"operation": "adopt", "party": party, "first_sent": first_sent});
    assert_eq!(listed, serde_json::json!({"pending": [row], "unread": 0}));
    assert_eq!(again.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&again.stderr));
    let adopted = json(again);
    let kept: serde_json::Value = serde_json::from_slice(&again.stderr).expect("one kept line, JSON");
    assert_eq!(kept, serde_json::json!({"operation": "adopt", "outcome": adopted}), "the first answer");
    assert_eq!((empty.exit, empty.stdout.as_slice()), (Some(Exit::Code(0)), b"".as_slice()));
    forgejo.stop();
    scratch.remove();
}

#[test]
fn temper_serve_prints_its_link_and_drains_on_a_signal() {
    let scratch = support::Scratch::new("drain");
    support::configuration(scratch.path(), &support::Forge::Unreached);
    let ended = support::until_link_then(scratch.path(), Signal::Terminate, Duration::from_secs(120));
    let link = support::printed_link(&ended.stdout).expect("one printed line");
    assert!(link.starts_with(b"http://127.0.0.1:"), "a loopback link: {}", String::from_utf8_lossy(link));
    assert!(skein_lib::bytes::find(link, b"/jig/v1/sign-in/link?secret=").is_some(), "the deployment's sign-in link");
    assert_eq!(ended.exit, Some(Exit::Code(0)), "drained: {}", String::from_utf8_lossy(&ended.stderr));
    scratch.remove();
}

#[test]
fn temper_open_signs_in_prints_the_inbox_and_exits_on_q() {
    let scratch = support::Scratch::new("temper_open_inbox");
    support::configuration(scratch.path(), &support::Forge::Unreached);
    let mut serve = support::serve(scratch.path());
    let link = support::terminal::link(&mut serve, support::WITHIN);
    let open = support::terminal::open(scratch.path(), &scratch.path().join("client"), link.as_ref());
    let mut typist = support::terminal::Typist::new(open, serve, link);
    typist.wait_for(b"== Inbox == Live", support::WITHIN);
    typist.type_line(b"q");
    assert_eq!(typist.exit(support::WITHIN), 0, "stderr: {}", typist.stderr());
    assert!(support::private(&scratch.path().join("client/token")), "the bearer token is kept 0o600");
    scratch.remove();
}

#[test]
fn temper_open_adopts_a_repository_by_commands() {
    let scratch = support::Scratch::new("temper_open_adopt");
    let forgejo = support::forgejo::Throwaway::start(&scratch);
    support::configuration(scratch.path(), &forgejo.forge());
    let mut serve = support::serve(scratch.path());
    let link = support::terminal::link(&mut serve, support::WITHIN);
    let open = support::terminal::open(scratch.path(), &scratch.path().join("client"), link.as_ref());
    let mut typist = support::terminal::Typist::new(open, serve, link);
    typist.wait_for(b"== Inbox == Live", support::WITHIN);
    typist.wait_for(b"All goals (pressed)\n", support::WITHIN);
    typist.type_on(b"Adopt a repository", b"");
    for (control, words, printed) in [
        (b"Owner: (empty)".as_slice(), b"acme".as_slice(), b"Owner: acme".as_slice()),
        (b"Name: (empty)", b"widget", b"Name: widget"),
        (b"Landing branch: (empty)", b"main", b"Landing branch: main"),
        (
            b"Checks: gate numbers, separated by spaces or commas: (empty)",
            b"1",
            b"Checks: gate numbers, separated by spaces or commas: 1",
        ),
    ] {
        typist.type_on(control, words);
        typist.wait_for(printed, support::WITHIN);
    }
    typist.type_on(b"[x] It runs CI", b"");
    typist.wait_for(b"[ ] It runs CI", support::WITHIN);
    typist.type_on(b"Adopt", b"");
    typist.wait_for(b"Adopted acme/widget on forge 1 as owned.", support::WITHIN);
    typist.type_line(b"q");
    assert_eq!(typist.exit(support::WITHIN), 0, "stderr: {}", typist.stderr());
    forgejo.stop();
    scratch.remove();
}

#[test]
fn a_push_is_delivered_signed_with_the_secret_reconcile_drew() {
    let scratch = support::Scratch::new("hook_delivery");
    let forgejo = support::forgejo::Throwaway::start(&scratch);
    let listener = std::net::TcpListener::bind(forgejo.webhooks()).expect("nothing holds the address");
    forgejo.push("hook.txt");
    let delivery = loop {
        let delivery = support::webhooks::receive(&listener, support::WITHIN);
        if skein_lib::bytes::find(&delivery.body, b"Add hook.txt").is_some() {
            break delivery;
        }
    };
    assert_eq!(delivery.event, "push", "Forgejo names the event");
    assert!(skein_lib::bytes::find(&delivery.body, b"refs/heads/main").is_some(), "a push to main");
    assert_eq!(delivery.signature, support::webhooks::signature(&forgejo.secret(), &delivery.body));
    forgejo.stop();
    scratch.remove();
}

#[test]
fn a_persons_change_lands_on_the_forge_and_is_found_after_a_restart() {
    landing("landed", false);
}

#[test]
fn a_change_lands_by_deliveries_with_every_poll_raised() {
    landing("hinted", true);
}

/// The landing story; the hinted case raises four limits and pushes the slow job marker.
fn landing(case: &str, raised: bool) {
    let scratch = support::Scratch::new(case);
    let forgejo = if raised {
        support::forgejo::Throwaway::start_with_job_sleep(&scratch, 5)
    } else {
        support::forgejo::Throwaway::start(&scratch)
    };
    if raised {
        forgejo.push("slow");
    }
    support::configuration(scratch.path(), &forgejo.forge());
    if raised {
        support::raise_paces(scratch.path());
    }
    let peer = support::fake_llm(support::landing_scripts(support::ANSWER, &forgejo));
    let clock = skein_shell::Clock::new();
    let began = clock.now().now;
    let first = support::run_until_landed(scratch.path(), support::person::adopt_and_change(&forgejo), peer, &forgejo);
    assert_eq!((first.signed_in, first.finished), (1, true), "{}", String::from_utf8_lossy(&first.stderr));
    eprintln!("stage job: {} ms", clock.now().now.saturating_since(began).as_nanos() / 1_000_000);
    let merged = forgejo.merged_pull().expect("the change's pull request is merged");
    assert_eq!(forgejo.job_state(merged), "success");
    assert!(forgejo.main_readme().ends_with(support::EDIT), "main holds the agent's edit");
    let second = support::run(scratch.path(), support::person::found(), None, Duration::from_secs(120));
    assert!(second.finished, "the work is found after the restart");
    assert_eq!(forgejo.merged_pull(), Some(merged), "the forge's state is unchanged by the restart");
    forgejo.stop();
    scratch.remove();
}

#[test]
fn forge_prepare_makes_an_owned_forge_then_remakes_nothing() {
    let scratch = support::Scratch::new("forge_prepare_real");
    let _port = support::owned::prepared_processes(scratch.path());
    let before: Vec<_> = support::owned::files(scratch.path())
        .iter()
        .map(|path| {
            assert!(support::private(path), "{} is private", path.display());
            fs_read(path)
        })
        .collect();
    let secret = fs_read(&scratch.path().join("forge-1/runner/secret"));
    assert_eq!(secret.len(), 40);
    assert!(secret.iter().all(u8::is_ascii_hexdigit));
    let second = support::owned::prepare(scratch.path(), 1);
    assert_eq!(second.status.code(), Some(0), "{}", String::from_utf8_lossy(&second.stderr));
    let after: Vec<_> = support::owned::files(scratch.path()).iter().map(|path| fs_read(path)).collect();
    assert_eq!(after, before);
    let ini = scratch.path().join("forge-1/forgejo/app.ini");
    let settings = fs_read(&ini);
    let admin_token = scratch.path().join("forge-1/admin_token");
    std::fs::remove_file(&admin_token).expect("removed token");
    let third = support::owned::prepare(scratch.path(), 1);
    assert_eq!(third.status.code(), Some(0), "{}", String::from_utf8_lossy(&third.stderr));
    let token = fs_read(&admin_token);
    let line = token.split(|byte| *byte == b'\n').next().expect("token line");
    assert_eq!(line.len(), 40);
    assert!(line.iter().all(u8::is_ascii_hexdigit));
    assert_eq!(fs_read(&ini), settings);
    scratch.remove();
}
fn fs_read(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).expect("prepared file")
}

#[test]
fn a_prepared_forge_runs_forgejo_and_its_runner_once_each() {
    use support::owned;
    let scratch = support::Scratch::new("forge_run_real");
    let port = owned::prepared_processes(scratch.path());
    let forgejo = owned::Running::start(scratch.path(), "forgejo");
    let version = owned::await_answer(port, "/api/v1/version", None);
    assert_eq!(
        version["version"].as_str().expect("version").split('+').next(),
        Some(skein_forge::forgejo::pin::VERSION)
    );
    let token = std::fs::read_to_string(scratch.path().join("forge-1/admin_token")).expect("admin token");
    let admin = owned::await_answer(port, "/api/v1/user", Some(token.trim()));
    assert_eq!(admin["login"], "temper-admin");
    assert_eq!(admin["is_admin"], true);
    let mut runner = owned::Running::start(scratch.path(), "runner");
    runner.declared();
    let refused = owned::command(scratch.path(), "forgejo").output().expect("second run");
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(refused.stderr, b"temper forge run: forge 1: forgejo is running or being prepared already\n");
    runner.stop();
    forgejo.stop();
    assert!(owned::answer(port, "/api/v1/version", None).is_err());
    scratch.remove();
}

#[test]
fn an_older_recorded_forgejo_is_dumped_then_upgraded() {
    use std::fs;
    use support::owned;
    use temper_forge_setup::{place::File, state};
    let scratch = support::Scratch::new("forge_upgrade_real");
    let _port = owned::prepared_processes(scratch.path());
    owned::record_versions(scratch.path(), Some(b"16.0.5"), None);
    let partial = owned::file(scratch.path(), 1, File::DumpPartial);
    fs::write(&partial, b"interrupted dump").expect("partial");
    let upgraded = owned::prepare(scratch.path(), 1);
    assert!(upgraded.status.success(), "{}", String::from_utf8_lossy(&upgraded.stderr));
    let (backup, before, meta) = owned::dump(scratch.path());
    assert!(!partial.exists());
    let record = state::decode(&fs::read(owned::file(scratch.path(), 1, File::State)).expect("upgrade fixture"))
        .expect("upgrade fixture");
    assert_eq!(&*record.forgejo, skein_forge::forgejo::pin::VERSION.as_bytes());
    owned::record_versions(scratch.path(), Some(b"16.0.5"), None);
    let repeated = owned::prepare(scratch.path(), 1);
    assert!(repeated.status.success(), "{}", String::from_utf8_lossy(&repeated.stderr));
    assert_eq!(fs::read(&backup).expect("backup kept"), before);
    assert_eq!(
        fs::metadata(&backup).expect("upgrade fixture").modified().expect("upgrade fixture"),
        meta.modified().expect("upgrade fixture")
    );
    scratch.remove();
}
#[test]
fn an_older_recorded_runner_has_its_files_prepared_again_without_a_dump() {
    use std::fs;
    use support::owned;
    use temper_forge_setup::place::File;
    let scratch = support::Scratch::new("runner_upgrade_real");
    let _port = owned::prepared_processes(scratch.path());
    owned::record_versions(scratch.path(), None, Some(b"13.1.0"));
    let configuration = owned::file(scratch.path(), 1, File::RunnerConfiguration);
    let before = fs::read(&configuration).expect("connection");
    fs::remove_file(&configuration).expect("remove configuration");
    let upgraded = owned::prepare(scratch.path(), 1);
    assert!(upgraded.status.success(), "{}", String::from_utf8_lossy(&upgraded.stderr));
    assert_eq!(fs::read(configuration).expect("configuration remade"), before);
    assert_eq!(fs::read_dir(scratch.path().join("forge-1/backups")).expect("upgrade fixture").count(), 0);
    scratch.remove();
}

#[test]
fn reconcile_converges_an_owned_forgejo_and_a_second_run_writes_nothing() {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt},
    };
    use support::owned;
    let scratch = support::Scratch::new("owned_reconcile");
    let port = owned::reconcile_configuration(scratch.path());
    let running = owned::start(scratch.path());
    let first = owned::reconcile(scratch.path(), 1);
    assert_eq!(first.status.code(), Some(0), "{}", String::from_utf8_lossy(&first.stderr));
    assert_eq!(first.stdout, b"");
    let instance = owned::attach(scratch.path(), port, running);
    owned::assert_converged(scratch.path(), &instance);
    let files: Vec<_> =
        ["api_token", "git_token", "user"].iter().map(|name| scratch.path().join("forge-1").join(name)).collect();
    let before: Vec<_> = files.iter().map(|path| fs::metadata(path).expect("written file")).collect();
    for meta in &before {
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
    let second = owned::reconcile(scratch.path(), 1);
    assert_eq!(second.status.code(), Some(0), "{}", String::from_utf8_lossy(&second.stderr));
    for (path, before) in files.iter().zip(before) {
        let after = fs::metadata(path).expect("kept file");
        assert_eq!(
            (after.ino(), after.mtime(), after.mtime_nsec()),
            (before.ino(), before.mtime(), before.mtime_nsec())
        );
    }
    instance.stop().expect("forge stops");
    scratch.remove();
}

#[test]
fn temper_box_prints_its_link_and_stops_its_processes() {
    let scratch = support::Scratch::new("box_stop");
    let owned = boxed_example(&scratch);
    let forgejo = owned.forgejo;
    let ended = support::boxed::run(scratch.path(), Some(forgejo), support::boxed::Plan::Stop, BOX_DEADLINE);
    assert_box_streams(&ended);
    assert_eq!(ended.forgejo_at_link, Some(200), "Forgejo answered its version when the link came");
    assert_eq!(ended.exit, Some(Exit::Code(0)), "stopped as asked: {}", String::from_utf8_lossy(&ended.stderr));
    assert_eq!(ended.left, Some(Vec::new()), "no process the box started outlived it");
    assert_eq!(support::boxed::status(forgejo, "/api/v1/version"), None, "Forgejo is gone");
    scratch.remove();
}
fn boxed_example(scratch: &support::Scratch) -> support::owned::Owned {
    let owned = support::owned::Owned::tier();
    support::owned::lay(scratch.path(), &owned);
    std::fs::write(scratch.path().join("temper.json"), support::owned::example_document(scratch.path(), &owned))
        .expect("box example");
    owned
}
fn assert_box_streams(ended: &support::boxed::Outcome) {
    let link = support::printed_link(&ended.stdout).unwrap_or_else(|| {
        panic!("the box's stdout carries the link: {}", String::from_utf8_lossy(&ended.stderr));
    });
    assert!(skein_lib::bytes::find(link, b"/jig/v1/sign-in/link?secret=").is_some(), "the sign-in link");
    let alone = |line: &[u8]| line.is_empty() || line.starts_with(b"http://127.0.0.1:");
    assert!(ended.stdout.split(|byte| *byte == b'\n').all(alone), "stdout holds links alone");
    let steps: [&[u8]; 5] = [b"prepare 1: ", b"forgejo 1: ", b"runner 1: ", b"serve: ", b"box: "];
    let lines = || ended.stderr.split(|byte| *byte == b'\n').filter(|line| !line.is_empty());
    assert!(lines().all(|line| steps.iter().any(|step| line.starts_with(step))), "each stderr line names its step");
    assert!(lines().any(|line| line.starts_with(b"runner 1: ")), "the runner's lines arrive prefixed");
}

#[test]
fn temper_box_restarts_a_killed_forgejo_while_the_engine_serves() {
    let scratch = support::Scratch::new("box_kill");
    let owned = boxed_example(&scratch);
    let forgejo = owned.forgejo;
    let kill = support::boxed::Plan::KillForgejo;
    let ended = support::boxed::run(scratch.path(), Some(forgejo), kill, BOX_DEADLINE);
    let (killed, restarted) = ended.forgejo_pids.unwrap_or_else(|| {
        panic!("Forgejo was killed and came back: {}", String::from_utf8_lossy(&ended.stderr));
    });
    assert_ne!(killed, restarted, "another Forgejo process answers");
    assert_eq!(ended.engine_served, Some(true), "the same engine process answered while Forgejo was back");
    assert_eq!(ended.exit, Some(Exit::Code(0)), "stopped as asked: {}", String::from_utf8_lossy(&ended.stderr));
    assert_eq!(ended.left, Some(Vec::new()), "no process the box started outlived it");
    scratch.remove();
}

#[test]
fn temper_serve_answers_a_signed_delivery_204() {
    use support::webhooks::{self, KNOWN_FORGE, PING, Post};
    let scratch = support::Scratch::new("signed_hook");
    let ended = webhooks::delivered(&scratch, Post::signed(KNOWN_FORGE, b"ping", PING));
    assert_eq!(ended.statuses, [204]);
    assert_eq!(ended.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&ended.stderr));
    scratch.remove();
}
#[test]
fn temper_serve_answers_an_unsigned_delivery_403() {
    use support::webhooks::{self, KNOWN_FORGE, PING, Post};
    let scratch = support::Scratch::new("unsigned_hook");
    let ended = webhooks::delivered(&scratch, Post::unsigned(KNOWN_FORGE, b"ping", PING));
    assert_eq!(ended.statuses, [403]);
    assert_eq!(ended.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&ended.stderr));
    scratch.remove();
}
#[test]
fn temper_serve_answers_an_unknown_forge_404() {
    use support::webhooks::{self, PING, Post, UNKNOWN_FORGE};
    let scratch = support::Scratch::new("unknown_hook");
    let ended = webhooks::delivered(&scratch, Post::signed(UNKNOWN_FORGE, b"ping", PING));
    assert_eq!(ended.statuses, [404]);
    assert_eq!(ended.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&ended.stderr));
    scratch.remove();
}

#[test]
fn a_native_chat_reaches_the_model() {
    let scratch = support::Scratch::new("native-chat");
    let forgejo = support::forgejo::Throwaway::start(&scratch);
    support::configuration(scratch.path(), &forgejo.forge());
    let peer = support::fake_llm(vec![support::chat_script(support::ANSWER)].into_boxed_slice());
    let seen = support::run(
        scratch.path(),
        support::person::answered_chat(b"Hello", support::ANSWER),
        Some(peer),
        support::WITHIN,
    );
    assert!(seen.finished);
    forgejo.stop();
    scratch.remove();
}

#[test]
fn program_login_execs_the_configured_fake_claude() {
    program_login_story("success", smith_fake_claude::Login::Immediate("claude.ai".into()), 0);
}

#[test]
fn program_login_propagates_the_fake_claudes_refusal() {
    program_login_story("refused", smith_fake_claude::Login::Refused, 1);
    program_login_story("exec-refused", smith_fake_claude::Login::default(), 1);
}

fn program_login_story(row: &str, login: smith_fake_claude::Login, exit: u8) {
    use smith_fake_claude::{LOGIN_FILE, OBSERVATIONS_FILE, STORY_FILE, Story, observations};
    let scratch = support::Scratch::new(&format!("program-login-{row}"));
    let directory = scratch.path();
    let executable = std::path::PathBuf::from(
        std::env::var_os("TEMPER_TEST_CLAUDE")
            .expect("build smith's fake-claude through heavy and set its absolute TEMPER_TEST_CLAUDE path"),
    );
    assert!(executable.is_absolute() && executable.is_file(), "configured fake binary: {}", executable.display());
    let programs = directory.join("programs");
    let configured = if row == "exec-refused" { directory.join("absent-claude") } else { executable };
    support::program_login::configuration(directory, Some(&programs), &configured);
    let source = programs.join("source");
    if row != "exec-refused" {
        smith_agent_shell::program::prepare(&programs, "source").expect("story directory");
        std::fs::write(source.join(STORY_FILE), Story { login, ..Story::default() }.encode()).expect("fake story");
    }
    let ended = support::program_login::run(directory);
    assert_eq!(ended.exit, Some(Exit::Code(exit)), "{}", String::from_utf8_lossy(&ended.stderr));
    let stderr = String::from_utf8_lossy(&ended.stderr);
    if row == "exec-refused" {
        let lines = stderr.lines().collect::<Vec<_>>();
        let (last, before) = lines.split_last().expect("temper's refusal line");
        assert!(last.starts_with("temper program-login: exec: "), "{stderr}");
        assert!(before.iter().all(|line| line.starts_with("smith: ")), "{stderr}");
        assert!(!source.join(OBSERVATIONS_FILE).exists());
        assert!(!source.join(LOGIN_FILE).exists());
        support::program_login::no_deployment_resources(directory);
        scratch.remove();
        return;
    }
    assert!(stderr.lines().all(|line| !line.starts_with("temper program-login:")), "{stderr}");
    let records = observations(&std::fs::read(source.join(OBSERVATIONS_FILE)).expect("observations"));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].arguments, ["auth", "login"]);
    assert_eq!(records[0].environment, ["CLAUDE_CONFIG_DIR", "HOME", "PATH", "TEMPER_BENIGN"]);
    if exit == 0 {
        assert_eq!(std::fs::read(source.join(LOGIN_FILE)).expect("login written"), b"claude.ai");
        assert_eq!(ended.stdout, b"Signed in.\n");
    } else {
        assert!(!source.join(LOGIN_FILE).exists());
        assert!(ended.stderr.windows(b"sign-in refused".len()).any(|bytes| bytes == b"sign-in refused"));
    }
    support::program_login::no_deployment_resources(directory);
    scratch.remove();
}

#[test]
fn a_first_run_answers_on_the_demo() {
    first_run(false);
}

/// domain/testing.md, 2.6; protocol/setup.md, 13.4, "A first run set up for real".
#[test]
fn a_forgejo_set_up_from_the_client_starts_again_by_its_record() {
    use skein_forge::forgejo::pin::VERSION;
    use std::{fs, os::unix::fs::PermissionsExt};
    use support::ctl::Step;

    let scratch = support::Scratch::new("forgejo-from-client");
    let paths = ["config", "data", "cache"].map(|name| scratch.path().join(name));
    for path in &paths {
        fs::create_dir(path).expect("empty first-run directory");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private first-run directory");
    }
    let flags = deployment_flags(&paths);
    let (cached, sources) = cache_forgejo_binaries(&paths[2]);
    let mut bot = None;
    for run in ["first", "second"] {
        let client_root = scratch.path().join(run);
        fs::create_dir(&client_root).expect("fresh client root");
        fs::set_permissions(&client_root, fs::Permissions::from_mode(0o700)).expect("private client root");
        let mut steps = forgejo_setup_steps(run);
        let clock = skein_shell::Clock::new();
        let began = clock.now().now;
        steps.push(Step::Mark("first repository ready"));
        if run == "first" {
            // protocol/setup.md, 13.4: main's push run is taken before the owner's
            // first task; no pull exists yet to have caused this runner line.
            steps.push(Step::Stderr(b" repo is work/hello "));
            steps.extend(support::first_repository::task_steps(&client_root));
        } else {
            steps.extend(support::first_repository::found_steps());
        }
        let ended = support::boxed::run_ctl_with_llm(
            scratch.path(),
            flags.clone(),
            &client_root,
            steps,
            Duration::from_secs(190),
            || support::first_repository::live(&paths[0], &paths[1], &client_root),
            support::first_repository::llm(&paths[0]),
        );
        eprintln!("{run} Forgejo setup run: {} ms", clock.now().now.saturating_since(began).as_nanos() / 1_000_000);
        let stderr = String::from_utf8_lossy(&ended.stderr);
        assert_eq!(ended.exit, Some(Exit::Code(0)), "{run}: {stderr}");
        assert_eq!(ended.left, Some(Vec::new()), "{run}: no process outlived the box: {stderr}");
        let setup_calls = if run == "first" { 7 } else { 4 };
        bot = Some(assert_forgejo_setup(run, &ended.calls[..setup_calls], &stderr, bot, &paths[1]));
        for call in &ended.calls[setup_calls..] {
            assert_eq!(
                call.exit,
                Some(Exit::Code(0)),
                "stdout {}; stderr {}; box {stderr}",
                String::from_utf8_lossy(&call.stdout),
                String::from_utf8_lossy(&call.stderr)
            );
        }
        let evidence = ended.checked.expect("the live forge and task reads ran");
        support::first_repository::assert_task(run, &ended.calls[setup_calls..], &stderr, &evidence);
        let support::first_repository::Evidence { address, version, .. } = evidence;
        let version = version.unwrap_or_else(|why| panic!("{run}: live Forgejo version: {why:?}; box: {stderr}"));
        assert_eq!(version["version"].as_str().expect("version").split('+').next(), Some(VERSION));
        let link = support::printed_link(&ended.stdout).expect("box sign-in link");
        assert_eq!(ended.stdout, [link, b"\n"].concat(), "one printed link");
        assert_eq!(support::boxed::status(address, "/api/v1/version"), None, "Forgejo stopped with its box");
        for (path, source) in cached.iter().zip(&sources) {
            assert_cached_binary_unchanged(path, source);
            assert!(
                !path
                    .with_file_name(format!("{}.part", path.file_name().expect("binary name").to_str().expect("UTF-8")))
                    .exists(),
                "no partial download"
            );
        }
        let wrote = format!("box: wrote the configuration, {}\n", paths[0].join("temper.json").display());
        assert_eq!(stderr.contains(&wrote), run == "first", "only the first box writes configuration");
    }
    scratch.remove();
}

/// domain/testing.md, 2.7: the live story moves in only a private held record.
#[test]
fn a_held_chatgpt_record_is_admitted_only_as_a_private_regular_file_never_named() {
    use std::{fs, os::unix::fs::PermissionsExt};
    use support::forge_land::held_account;
    let scratch = support::Scratch::new("held-account");
    let file = scratch.path().join("record");
    assert_eq!(held_account(&file), Err("TEMPER_LIVE_ACCOUNT names no file"));
    fs::write(&file, b"record").expect("record");
    for (mode, admitted) in [
        (0o600, Ok(())),
        (0o640, Err("TEMPER_LIVE_ACCOUNT is not 0600")),
        (0o644, Err("TEMPER_LIVE_ACCOUNT is not 0600")),
        (0o400, Err("TEMPER_LIVE_ACCOUNT is not 0600")),
        (0o700, Err("TEMPER_LIVE_ACCOUNT is not 0600")),
    ] {
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).expect("mode");
        assert_eq!(held_account(&file), admitted, "mode {mode:o}");
    }
    let link = scratch.path().join("link");
    std::os::unix::fs::symlink(&file, &link).expect("link");
    assert_eq!(held_account(&link), Err("TEMPER_LIVE_ACCOUNT is not a regular file"));
    assert_eq!(held_account(scratch.path()), Err("TEMPER_LIVE_ACCOUNT is not a regular file"));
    scratch.remove();
}

/// domain/testing.md, 2.7: notices print an imported record's path as FILE.
#[test]
fn an_imported_records_path_is_printed_as_file() {
    use support::forge_land::printed_command;
    let words = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        printed_command(&words(&["import", "/held/record", "--key", "connect-plan"])),
        words(&["import", "FILE", "--key", "connect-plan"])
    );
    for kept in [&["accounts", "--json"][..], &["connect", "1", "--key", "connect-plan"], &["import"]] {
        assert_eq!(printed_command(&words(kept)), words(kept));
    }
}

/// protocol/setup.md, 8.4 and 13.4: the demo ends on the owned forge's runtime connect.
#[test]
fn a_first_run_plays_the_demo_then_sets_up_a_forgejo_here_with_no_restart() {
    use support::first_repository::{self, Task};

    // The demo's chat is task 1 in project 1; hello gets the next project and task tree.
    const HELLO: Task = Task { project: 2, chat: 2, change: 3, producer: 4 };
    // Seven playground reads plus the reply wait; six setup reads; six end reads
    // (including two refused starts); seven calls for hello's task and landed change.
    const DEMO_CALLS: usize = 8;
    const SETUP_END: usize = DEMO_CALLS + 6;
    const END_READS_END: usize = SETUP_END + 6;
    const ALL_CALLS: usize = END_READS_END + 7;
    let scratch = support::Scratch::new("demo-then-forgejo");
    let (paths, flags) = first_run_directories(&scratch);
    let (cached, sources) = cache_forgejo_binaries(&paths[2]);
    let steps = demo_then_forgejo_steps(scratch.path(), HELLO);
    let mut llm = first_repository::llm(&paths[0]);
    llm.expected_exits = vec![(END_READS_END - 2, 2), (END_READS_END - 1, 2)];
    let clock = skein_shell::Clock::new();
    let began = clock.now().now;
    let seen = support::boxed::run_ctl_with_llm(
        scratch.path(),
        flags,
        scratch.path(),
        steps,
        Duration::from_secs(190),
        || {
            (
                first_repository::live_for(&paths[0], &paths[1], scratch.path(), HELLO),
                first_repository::turns(scratch.path(), "1"),
            )
        },
        llm,
    );
    eprintln!("demo then Forgejo box: {} ms", clock.now().now.saturating_since(began).as_nanos() / 1_000_000);
    let stderr = String::from_utf8_lossy(&seen.stderr);
    assert_eq!(seen.exit, Some(Exit::Code(0)), "{stderr}");
    assert_eq!(seen.left, Some(Vec::new()), "no process outlived the box: {stderr}");
    assert_eq!(seen.calls.len(), ALL_CALLS, "every story read ran: {stderr}");
    for (index, call) in seen.calls.iter().enumerate() {
        let code = if (END_READS_END - 2..END_READS_END).contains(&index) { 2 } else { 0 };
        assert_eq!(
            call.exit,
            Some(Exit::Code(code)),
            "call {index}: stdout {}; stderr {}; box {stderr}",
            String::from_utf8_lossy(&call.stdout),
            String::from_utf8_lossy(&call.stderr)
        );
        if code == 0 {
            assert!(call.stderr.is_empty(), "call {index}: {}", String::from_utf8_lossy(&call.stderr));
        }
    }
    assert_eq!(seen.calls[1].stdout, b"running\n");
    for call in &seen.calls[2..4] {
        assert_playground_end_forges(call, true);
    }
    assert_eq!(support::ctl::json(&seen.calls[5])["met"], true, "the demo chat replied");
    let account = assert_playground_chat_and_account_open([&seen.calls[4], &seen.calls[6], &seen.calls[7]]);
    let _bot = support::forge_land::assert_forgejo_setup_after_sign_in(
        "first",
        &seen.calls[DEMO_CALLS..SETUP_END],
        &stderr,
        None,
        &paths[1],
    );
    assert_playground_end_forges(&seen.calls[SETUP_END - 2], false);
    assert!(
        !String::from_utf8_lossy(&seen.calls[SETUP_END - 1].stdout).lines().any(|line| line.starts_with("forge 1 ·")),
        "the ended demo leaves the text list"
    );
    assert_playground_chat_and_account_ended("runtime", &seen.calls[SETUP_END..END_READS_END], Some(account));
    assert_eq!(
        stderr.lines().filter(|line| *line == "box: forge 1's group stopped").count(),
        1,
        "the runtime connect stops the demo exactly once: {stderr}"
    );
    assert!(!stderr.contains("demo end:"), "granted end has no refusal diagnostic: {stderr}");
    let created = support::ctl::json(&seen.calls[END_READS_END + 2]);
    assert_eq!(created["outcome"]["done"]["owner"]["created"]["first"], HELLO.chat, "{created}");
    let (evidence, demo) = seen.checked.expect("live task and demo reads ran before stopping");
    assert!(
        demo.contains("I would give widget a dark theme that follows the system's preference"),
        "the scripted demo model answered: {demo}"
    );
    let producer = support::ctl::json(&seen.calls[ALL_CALLS - 1]);
    assert_eq!(producer["project"], HELLO.project, "hello's task belongs to the new project: {producer}");
    eprintln!("work/hello's project: {}", producer["project"]);
    first_repository::assert_task("first", &seen.calls[END_READS_END..], &stderr, &evidence);
    assert_eq!(
        evidence.version.expect("live Forgejo version")["version"].as_str().expect("version").split('+').next(),
        Some(skein_forge::forgejo::pin::VERSION)
    );
    assert_eq!(support::boxed::status(evidence.address, "/api/v1/version"), None, "Forgejo stopped with its box");
    for (path, source) in cached.iter().zip(&sources) {
        assert_cached_binary_unchanged(path, source);
        assert!(
            !path
                .with_file_name(format!("{}.part", path.file_name().expect("binary name").to_str().expect("UTF-8")))
                .exists(),
            "no partial download"
        );
    }
    assert_demo_then_forgejo_streams(&seen.stdout, &stderr);
    scratch.remove();
}

fn demo_then_forgejo_steps(root: &std::path::Path, task: support::first_repository::Task) -> Vec<support::ctl::Step> {
    use support::ctl::{Call, Step};
    use support::first_repository;
    let mut steps = playground_end_steps("first");
    steps.insert(5, Step::Run(Call::new(&["wait", "task", "1", "--until", "replied", "--timeout", "30s", "--json"])));
    // Keep the same signed-in client and box: skip the setup story's mark and sign-in.
    steps.extend(forgejo_setup_steps("first").into_iter().skip(2));
    steps.push(Step::Stderr(b"box: forge 1's group stopped\n"));
    // The end is read at runtime, using the restart story's chat/account/refusal reads.
    steps.extend(playground_end_steps("third").into_iter().skip(3));
    steps.push(Step::Stderr(b" repo is work/hello "));
    steps.extend(first_repository::task_steps_for(root, task));
    steps
}

fn assert_demo_then_forgejo_streams(stdout: &[u8], stderr: &str) {
    // The real forge adds its three step prefixes to the demo stream's checker.
    let demo_streams: String = stderr
        .split_inclusive('\n')
        .filter(|line| !["prepare 3: ", "forgejo 3: ", "runner 3: "].iter().any(|prefix| line.starts_with(prefix)))
        .collect();
    assert_demo_streams(stdout, demo_streams.as_bytes());
}

/// domain/testing.md, 2.6: a plan-backed change on the first owned repository.
#[test]
fn a_persons_plan_change_lands_on_the_owned_forge_and_is_found_after_a_restart() {
    use std::{fs, os::unix::fs::PermissionsExt};
    use support::forge_land::{self, Plan};

    let scratch = support::Scratch::new("forgejo-plan-change");
    let paths = ["config", "data", "cache"].map(|name| scratch.path().join(name));
    let mut issuer = Some(forge_land::fake_deployment(&paths));
    let flags = deployment_flags(&paths);
    let (cached, sources) = cache_forgejo_binaries(&paths[2]);
    let clock = skein_shell::Clock::new();
    let addition = format!("Plan change at {}-{}", std::process::id(), clock.now().now.as_nanos());
    let words = format!("Please append this line to work/hello README.md: {addition}");
    let mut bot = None;
    let mut landed = None;
    let mut ended_tasks = None;
    for run in ["first", "second"] {
        let first = run == "first";
        let client = scratch.path().join(run);
        fs::create_dir(&client).expect("fresh client root");
        fs::set_permissions(&client, fs::Permissions::from_mode(0o700)).expect("private client root");
        let mut steps = forgejo_setup_steps(run);
        let setup_calls = if first { 7 } else { 4 };
        let (account_at, found_at) = plan_change_steps(&mut steps, first, setup_calls, &words);
        let scripts = if first {
            forge_land::plan_scripts(&Plan {
                words: words.as_bytes(),
                line: addition.as_bytes(),
                repository: temper_engine_domain_forge::tag(3, 1),
                readme: b"# hello\n\n",
            })
        } else {
            // A fresh fake has no request cursor from the old process. Its
            // first turn answers Close's finish request with text, which parks
            // the Open run. Its system cue remains the original task's words.
            forge_land::restart_script(words.as_bytes())
        };
        let issuer = issuer.take().unwrap_or_else(|| support::sign_in::restart_issuer(&paths[0]));
        let clock = skein_shell::Clock::new();
        let began = clock.now().now;
        let (outcome, seen) = support::boxed::run_ctl_with_face(
            scratch.path(),
            flags.clone(),
            &client,
            steps,
            Duration::from_secs(240),
            || support::first_repository::live(&paths[0], &paths[1], &client),
            forge_land::plan_face(issuer, scripts, first),
        );
        eprintln!("{run} plan Forgejo box: {} ms", clock.now().now.saturating_since(began).as_nanos() / 1_000_000);
        let stderr = String::from_utf8_lossy(&outcome.stderr);
        eprintln!(
            "{run} peer: {} requests; answers {:?}; schemes {:?}",
            seen.requests.len(),
            seen.answers,
            seen.schemes
        );
        assert_plan_box(run, first, &outcome, &stderr);
        assert!(outcome.calls.len() > found_at + 3, "all story reads ran: {stderr}");
        bot = Some(assert_forgejo_setup(run, &outcome.calls[..setup_calls], &stderr, bot, &paths[1]));
        support::sign_in::assert_plan_account(&outcome.calls[setup_calls], &outcome.calls[account_at]);
        let evidence = outcome.checked.expect("live check ran");
        support::first_repository::assert_task(run, &outcome.calls[found_at..found_at + 3], &stderr, &evidence);
        let task = String::from_utf8(outcome.calls[found_at + 1].stdout.clone()).expect("landed task UTF-8");
        let identity = forge_land::landed_identity(&task, &evidence.page);
        let tasks_at = outcome.calls.len() - if first { 2 } else { 6 };
        let tasks = [0, 1].map(|offset| {
            forge_land::ended_tasks(&support::ctl::json(&outcome.calls[tasks_at + offset]), 2 + offset as u64)
        });
        if first {
            landed = Some(identity);
            ended_tasks = Some(tasks);
        } else {
            assert_eq!(Some(identity), landed, "same pull and merge commit");
            assert_eq!(Some(tasks), ended_tasks, "no second change or producer");
            let ended = support::ctl::json(&outcome.calls[tasks_at + 3]);
            assert_eq!(ended["met"], true);
            assert_eq!(ended["task"]["chip"]["phase"], "ended");
            forge_land::assert_no_live_roots(&support::ctl::json(&outcome.calls[tasks_at + 4]));
            let unknown = &outcome.calls[tasks_at + 5];
            assert_eq!(unknown.exit, Some(Exit::Code(2)), "no task 4: {stderr}");
            assert_eq!(unknown.stderr.trim_ascii_end(), b"{\"error\":\"unknown\"}", "no task 4: {stderr}");
        }
        forge_land::assert_reply(&outcome.calls[found_at + 4..tasks_at], first);
        // Init has no review gate. Four coder turns include the workflow read;
        // three chat turns delegate, wait, then reply after the procedure ends.
        // After the restart, the chat's lost attempt answers again, then Close's
        // finish request takes its last text.
        forge_land::assert_requests(&seen, if first { 7 } else { 2 }, words.as_bytes(), first);
        let link = support::printed_link(&outcome.stdout).expect("printed link");
        assert_eq!(outcome.stdout, [link, b"\n"].concat(), "one printed link");
        assert_eq!(support::boxed::status(evidence.address, "/api/v1/version"), None);
        assert_eq!(
            evidence.version.expect("live Forgejo")["version"].as_str().expect("version").split('+').next(),
            Some(skein_forge::forgejo::pin::VERSION)
        );
        assert!(
            !stderr.contains(&format!("box: wrote the configuration, {}\n", paths[0].join("temper.json").display())),
            "init wrote configuration before either box"
        );
        for (path, source) in cached.iter().zip(&sources) {
            assert_cached_binary_unchanged(path, source);
            assert!(
                !path
                    .with_file_name(format!("{}.part", path.file_name().expect("binary name").to_str().expect("UTF-8")))
                    .exists()
            );
        }
    }
    scratch.remove();
}

/// domain/testing.md, 2.6: a change whose coder fails holds, and its chat answers the escalation.
#[test]
fn a_change_whose_coder_fails_holds_and_its_chat_answers() {
    use std::{fs, os::unix::fs::PermissionsExt};
    use support::forge_land::{self, CtlFace, Plan};

    let scratch = support::Scratch::new("forgejo-held-change");
    let paths = ["config", "data", "cache"].map(|name| scratch.path().join(name));
    let issuer = forge_land::fake_deployment(&paths);
    let flags = deployment_flags(&paths);
    let _cached = cache_forgejo_binaries(&paths[2]);
    let clock = skein_shell::Clock::new();
    let addition = format!("Held change at {}-{}", std::process::id(), clock.now().now.as_nanos());
    let words = format!("Please append this line to work/hello README.md: {addition}");
    let client = scratch.path().join("first");
    fs::create_dir(&client).expect("fresh client root");
    fs::set_permissions(&client, fs::Permissions::from_mode(0o700)).expect("private client root");
    let mut steps = forgejo_setup_steps("first");
    let setup_calls = 7;
    let (_, found_at) = forge_land::held_change_steps(&mut steps, setup_calls, &words);
    let scripts = forge_land::held_scripts(&Plan {
        words: words.as_bytes(),
        line: addition.as_bytes(),
        repository: temper_engine_domain_forge::tag(3, 1),
        readme: b"# hello\n\n",
    });
    let (outcome, seen) = support::boxed::run_ctl_with_face(
        scratch.path(),
        flags,
        &client,
        steps,
        Duration::from_secs(240),
        || support::first_repository::live(&paths[0], &paths[1], &client),
        CtlFace { face: forge_land::fake_face(issuer, scripts), first: true, reply: forge_land::HELD },
    );
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert_plan_box("first", true, &outcome, &stderr);
    let _bot = assert_forgejo_setup("first", &outcome.calls[..setup_calls], &stderr, None, &paths[1]);
    let held = support::ctl::json(&outcome.calls[found_at]);
    assert_eq!(held["met"], true, "the change holds: {stderr}");
    let change = support::ctl::json(&outcome.calls[found_at + 1]);
    assert_eq!(change["chip"]["phase"], "held", "{change}");
    // The box stopped once the chat replied HELD; its last read of the chat
    // shows it answering, not refused before each run.
    let chat = outcome
        .calls
        .iter()
        .rev()
        .filter_map(|call| serde_json::from_slice::<serde_json::Value>(&call.stdout).ok())
        .find(|read| read["chip"]["task"] == 1)
        .expect("a read of the chat");
    assert_ne!(chat["chip"]["phase"], "backing_off", "{chat}");
    assert_eq!(chat["chip"]["details"]["owner"]["details"]["retry"], serde_json::Value::Null, "{chat}");
    // The chat delegates, replies, then answers the escalation once; the coder fails in one turn.
    assert_eq!(seen.requests.len(), 4, "model requests: {stderr}");
    scratch.remove();
}

#[test]
fn a_first_run_answers_on_the_demo_and_a_second_finds_it_running() {
    first_run(true);
}
#[test]
fn two_first_runs_one_after_the_other_choose_their_own_ports() {
    fn ports(config: &std::path::Path) -> [u16; 5] {
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config.join("temper.json")).expect("written configuration"))
                .expect("configuration JSON");
        let application = &document["application"];
        let setup = application["setup"].as_array().expect("forge setup");
        let fake = setup.iter().find(|entry| entry["forge"] == 1).expect("fake forge 1");
        let owned = setup.iter().find(|entry| entry["forge"] == 3).expect("owned forge 3");
        [
            &application["clients"]["listener"]["socket"],
            &fake["mode"]["fake"]["listen"]["socket"],
            &application["demo"]["llm"]["listen"]["socket"],
            &application["webhooks"]["listener"]["socket"],
            &owned["mode"]["owned"]["listen"]["socket"],
        ]
        .map(|value| {
            value.as_str().expect("listener socket").parse::<std::net::SocketAddr>().expect("socket address").port()
        })
    }
    fn assert_finished<T>(name: &str, config: &std::path::Path, ended: &support::boxed::StoryOutcome<T>) {
        let stderr = String::from_utf8_lossy(&ended.stderr);
        assert_eq!(ended.exit, Some(Exit::Code(0)), "{name}: {stderr}");
        assert!(ended.finished, "{name}: {stderr}");
        assert_eq!(ended.left, Some(Vec::new()), "{name}: no process outlived the box: {stderr}");
        let wrote = format!("box: wrote the configuration, {}\n", config.join("temper.json").display());
        assert!(stderr.contains(&wrote), "{name}: {stderr}");
        assert_demo_streams(&ended.stdout, &ended.stderr);
        assert!(ended.dormant_checked, "{name}: the welcome precedes any fake start");
    }
    let a = support::Scratch::new("two-first-runs-a");
    let b = support::Scratch::new("two-first-runs-b");
    let (a_paths, a_flags) = first_run_directories(&a);
    let (b_paths, b_flags) = first_run_directories(&b);
    let clock = skein_shell::Clock::new();
    let began = clock.now().now;
    let phase = |name| {
        eprintln!("two first runs: {name}: {} ms", clock.now().now.saturating_since(began).as_nanos() / 1_000_000);
    };
    phase("A started");
    let ended =
        support::boxed::run_story(a.path(), a_flags, a.path(), support::person::welcome_open(), BOX_DEADLINE, || {
            phase("A's person finished (A up)");
            phase("B started");
            let ended = support::boxed::run_story(
                b.path(),
                b_flags,
                b.path(),
                support::person::demo_start_answered(),
                BOX_DEADLINE,
                || {
                    let a_ports = ports(&a_paths[0]);
                    let b_ports = ports(&b_paths[0]);
                    for ((name, a), b) in ["clients", "fake forge", "fake LLM", "webhooks", "Forgejo"]
                        .into_iter()
                        .zip(a_ports)
                        .zip(b_ports)
                    {
                        assert_ne!(a, b, "{name}: the second first run chooses its own port");
                    }
                },
            );
            phase("B finished");
            assert_finished("B", &b_paths[0], &ended);
        });
    phase("A stopped");
    assert_finished("A", &a_paths[0], &ended);
    b.remove();
    a.remove();
}
fn first_run_directories(scratch: &support::Scratch) -> ([std::path::PathBuf; 3], Vec<String>) {
    use std::{fs, os::unix::fs::PermissionsExt};
    let paths = ["config", "data", "cache"].map(|name| scratch.path().join(name));
    for path in &paths {
        fs::create_dir(path).expect("empty first-run directory");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private first-run directory");
    }
    let flags: Vec<String> = ["--config", "--data", "--cache"]
        .into_iter()
        .zip(&paths)
        .flat_map(|(flag, path)| [flag.into(), path.display().to_string()])
        .collect();
    (paths, flags)
}

#[test]
fn a_first_run_ctl_demo_start_answers_running_twice() {
    use support::ctl::{Call, Step};
    let scratch = support::Scratch::new("first-run-ctl");
    let (_, flags) = first_run_directories(&scratch);
    let seen = support::boxed::run_ctl(
        scratch.path(),
        flags,
        scratch.path(),
        vec![
            Step::Run(Call::sign_in()),
            Step::Run(Call::new(&["demo-start", "--key", "first"])),
            Step::Run(Call::new(&["demo-start", "--key", "second"])),
        ],
        BOX_DEADLINE,
        || (),
    );
    assert_eq!(seen.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&seen.stderr));
    assert_eq!(seen.left, Some(Vec::new()), "no process outlived the box");
    assert_eq!(seen.checked, Some(()), "all calls finished before stopping");
    let [signed, first, second] = seen.calls.as_slice() else { panic!("sign-in and two demo starts") };
    assert_eq!(signed.exit, Some(Exit::Code(0)));
    for call in [first, second] {
        assert_eq!(call.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&call.stderr));
        assert_eq!(call.stdout, b"running\n");
        assert!(call.stderr.is_empty(), "{}", String::from_utf8_lossy(&call.stderr));
    }
    assert_demo_streams(&seen.stdout, &seen.stderr);
    scratch.remove();
}

// protocol/setup.md, 8.4: an unwatched end keeps its chat and account ended across restarts.
#[test]
fn a_second_forge_ends_the_playground_for_good_closing_its_chat_and_account() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let scratch = support::Scratch::new("playground-end");
    let (paths, flags) = first_run_directories(&scratch);
    let mut account = None;
    for run in ["first", "second", "third"] {
        let first = run == "first";
        let third = run == "third";
        let client = scratch.path().join(run);
        fs::create_dir(&client).expect("fresh client root");
        fs::set_permissions(&client, fs::Permissions::from_mode(0o700)).expect("private client root");
        let steps = playground_end_steps(run);
        let clock = skein_shell::Clock::new();
        let began = clock.now().now;
        let seen = if third {
            support::boxed::run_ctl_with_exits(
                scratch.path(),
                flags.clone(),
                &client,
                steps,
                BOX_DEADLINE,
                || support::playground::retired_reload(&paths[0], &client),
                &[(7, 2), (8, 2)],
            )
        } else {
            support::boxed::run_ctl(scratch.path(), flags.clone(), &client, steps, BOX_DEADLINE, || ())
        };
        eprintln!("{run} playground-end box: {} ms", clock.now().now.saturating_since(began).as_nanos() / 1_000_000);
        let stderr = String::from_utf8_lossy(&seen.stderr);
        assert_eq!(seen.exit, Some(Exit::Code(0)), "{run}: {stderr}");
        assert_eq!(seen.left, Some(Vec::new()), "{run}: no process outlived the box: {stderr}");
        assert_eq!(seen.checked, Some(()), "{run}: all calls finished before stopping: {stderr}");
        assert_eq!(
            seen.calls.len(),
            if first {
                7
            } else if third {
                9
            } else {
                3
            },
            "{run}: all calls ran: {stderr}"
        );
        for (index, call) in seen.calls.iter().enumerate() {
            let expected = if third && index >= 7 { 2 } else { 0 };
            assert_eq!(call.exit, Some(Exit::Code(expected)), "{run}: {}", String::from_utf8_lossy(&call.stderr));
            if expected == 0 {
                assert!(call.stderr.is_empty(), "{run}: {}", String::from_utf8_lossy(&call.stderr));
            }
        }
        if first {
            assert_eq!(seen.calls[1].stdout, b"running\n");
            account = Some(assert_playground_chat_and_account_open([&seen.calls[4], &seen.calls[5], &seen.calls[6]]));
        }
        if third {
            assert_playground_chat_and_account_ended(run, &seen.calls[3..9], account);
        }
        for call in &seen.calls[if first { 2..4 } else { 1..3 }] {
            assert_playground_end_forges(call, first);
        }
        assert_eq!(
            stderr.lines().filter(|line| *line == "box: forge 1's group stopped").count(),
            usize::from(run == "second"),
            "{run}: demo group stops exactly once at the end: {stderr}"
        );
        assert!(!stderr.contains("demo end:"), "{run}: granted end has no refusal diagnostic: {stderr}");
        if run == "third" {
            for prefix in ["fake forge 1: ", "prepare 1: ", "fake llm: "] {
                assert!(!stderr.lines().any(|line| line.starts_with(prefix)), "{run}: demo stays dormant: {stderr}");
            }
        }
        // These are the additional fake's step names (temper-fake's steps).
        // Reuse the first-run checker for every remaining diagnostic and stdout.
        let demo_streams: String = stderr
            .split_inclusive('\n')
            .filter(|line| first || !line.starts_with("prepare 3: ") && !line.starts_with("fake forge 3: "))
            .collect();
        assert_demo_streams(&seen.stdout, demo_streams.as_bytes());
        if first {
            playground_end_second_forge(&paths[0]);
        }
    }
    scratch.remove();
}

fn playground_end_steps(run: &str) -> Vec<support::ctl::Step> {
    use support::ctl::{Call, Step};
    let first = run == "first";
    let third = run == "third";
    let mut steps = vec![Step::Run(Call::sign_in())];
    if run == "second" {
        // The engine ends the demo before any client reads or watches it.
        steps.push(Step::Stderr(b"box: forge 1's group stopped\n"));
    }
    if first {
        steps.push(Step::Run(Call::new(&["demo-start", "--key", "first"])));
    }
    steps.extend([
        Step::Run(Call::new(&["wait", "forges", "--until", "settled", "--timeout", "30s", "--json"])),
        Step::Run(Call::new(&["forges", "--json"])),
    ]);
    if first {
        steps.extend([
            Step::Run(Call::new(&["chat", "--project", "1", "--key", "chat", "Add a dark theme to widget", "--json"])),
            Step::Run(Call::new(&["task", "1", "--json"])),
            Step::Run(Call::new(&["accounts", "--json"])),
        ]);
    }
    if third {
        steps.extend([
            Step::Run(Call::new(&["wait", "task", "1", "--until", "ended", "--timeout", "30s", "--json"])),
            Step::Run(Call::new(&["task", "1", "--json"])),
            Step::Run(Call::new(&["tasks", "--project", "1", "--label", "chat", "--mine", "--ended", "--json"])),
            Step::Run(Call::new(&["accounts", "--json"])),
            // Keep both refused starts, after observing the end without asking for it.
            Step::Run(Call::new(&["demo-start", "--key", "second"])),
            Step::Run(Call::new(&["demo-start", "--key", "third"])),
        ]);
    }
    steps
}

fn assert_playground_chat_and_account_open(calls: [&support::Ended; 3]) -> u64 {
    let [created, task, accounts] = calls.map(support::ctl::json);
    assert_eq!(created["outcome"]["done"]["owner"]["created"]["first"], 1, "{created}");
    assert_eq!(task["project"], 1, "the chat belongs to the demo project: {task}");
    assert_ne!(task["chip"]["phase"], "ended", "the demo's chat is open: {task}");
    let rows = accounts["accounts"]["deployment"].as_array().expect("deployment accounts");
    assert_eq!(rows.len(), 1, "the demo model's deployment account: {accounts}");
    assert_eq!(rows[0]["provider"], 3, "{accounts}");
    assert_eq!(rows[0]["provider_name"]["bytes"], "demo", "{accounts}");
    assert_eq!(rows[0]["holder"]["deployment"], true, "{accounts}");
    rows[0]["number"].as_u64().expect("demo account number")
}

fn assert_playground_chat_and_account_ended(run: &str, calls: &[support::Ended], account: Option<u64>) {
    let waited = support::ctl::json(&calls[0]);
    assert_eq!(waited["met"], true, "the chat ended: {waited}");
    let task = support::ctl::json(&calls[1]);
    assert_eq!(task["chip"]["phase"], "ended", "{task}");
    let cancelled = &task["chip"]["details"]["owner"]["details"]["ending"]["cancelled"];
    assert_eq!(cancelled["reason"]["bytes"], "Ended with the playground", "{task}");
    assert_eq!(cancelled["by"]["deployment"]["project"], 1, "the deployment closed its chat: {task}");
    let tasks = support::ctl::json(&calls[2]);
    let ended = tasks["ended"]["rows"]["rows"].as_array().expect("ended chats");
    assert_eq!(tasks["ended"]["rows"]["cut"], false, "{tasks}");
    assert!(tasks["ended"]["rows"]["after"].is_null(), "all ended chats: {tasks}");
    assert_eq!(ended.len(), 1, "one ended demo chat: {tasks}");
    assert_eq!(ended[0]["chip"]["task"], 1, "the same chat ended once: {tasks}");
    assert_eq!(ended[0]["chip"]["phase"], "ended", "{tasks}");
    let accounts = support::ctl::json(&calls[3]);
    let rows = accounts["accounts"]["deployment"].as_array().expect("deployment accounts");
    assert!(!rows.iter().any(|row| row["number"].as_u64() == account), "demo account removed: {accounts}");
    assert!(!rows.iter().any(|row| row["provider"] == 3), "demo provider has no account: {accounts}");
    for start in &calls[4..] {
        assert!(start.stdout.is_empty(), "{run}: refused start prints no answer on stdout");
        assert!(
            String::from_utf8_lossy(&start.stderr).contains("not offered"),
            "{run}: {}",
            String::from_utf8_lossy(&start.stderr)
        );
    }
}

fn assert_playground_end_forges(call: &support::Ended, first: bool) {
    let document = support::ctl::json(call);
    let document = if let Some(met) = document.get("met") {
        assert_eq!(met, true, "the forge wait settled: {document}");
        &document["forges"]
    } else {
        &document
    };
    let rows = document["forges"].as_array().expect("forge rows");
    let connected: Vec<_> = rows.iter().filter(|row| row["state"] == "connected").collect();
    assert_eq!(connected.len(), 1, "one connected forge: {document}");
    assert_eq!(connected[0]["number"], if first { 1 } else { 3 }, "{document}");
    if !first {
        assert!(!rows.iter().any(|row| row["number"] == 1), "the ended demo leaves every list: {document}");
    }
}

fn playground_end_second_forge(config: &std::path::Path) {
    let path = config.join("temper.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("written configuration")).expect("JSON");
    let application = &mut document["application"];
    let listen = application["forges"]
        .as_array()
        .expect("configured forges")
        .iter()
        .find(|forge| forge["number"] == 3)
        .expect("forge 3")["api"]
        .clone();
    let setup = application["setup"].as_array_mut().expect("setup entries");
    let mut second = setup.iter().find(|entry| entry["forge"] == 1).expect("demo setup").clone();
    second["forge"] = 3.into();
    second["directory"] = "forge-3".into();
    second["mode"]["fake"]["listen"] = listen;
    second["mode"]["fake"]["owner"] = "second-owner".into();
    // A fake requires at least one repository; keep the cloned widget seed.
    *setup.iter_mut().find(|entry| entry["forge"] == 3).expect("owned forge 3 setup") = second;
    support::write_private(&path, &serde_json::to_vec_pretty(&document).expect("configuration JSON"));
}

fn first_run(restart: bool) {
    use std::{fs, os::unix::fs::PermissionsExt};
    let scratch = support::Scratch::new("first-run");
    let (paths, flags) = first_run_directories(&scratch);
    let mut bot = None;
    for (run, person) in [
        (
            "first",
            support::person::demo_change(),
        ),
        (
            "second",
            // After chat 1, its change 2, and the change's review gate 3.
            support::person::demo_chat(
                4,
                b"Add a dark theme to widget",
                b"I would give widget a dark theme that follows the system's preference, with its colours as variables in one place so both themes stay in step. I am the scripted model and play this from a script, so nothing is changed: connect a model from Accounts to have the work done.",
            ),
        ),
    ].into_iter().take(if restart { 2 } else { 1 }) {
        let client = scratch.path().join(run);
        fs::create_dir(&client).expect("fresh client root");
        fs::set_permissions(&client, fs::Permissions::from_mode(0o700)).expect("private client root");
        let clock = skein_shell::Clock::new();
        let began = clock.now().now;
        let ended = support::boxed::run_story(
            scratch.path(),
            flags.clone(),
            &client,
            person,
            BOX_DEADLINE,
            || {
                let bot = demo_bot(&paths[0], &client);
                if run == "first" {
                    demo_landed(&paths[0], &client);
                }
                bot
            },
        );
        eprintln!("{run} run: {} ms", clock.now().now.saturating_since(began).as_nanos() / 1_000_000);
        let stderr = String::from_utf8_lossy(&ended.stderr);
        assert_eq!(ended.exit, Some(Exit::Code(0)), "{run}: {stderr}");
        assert!(ended.finished, "{run}: {stderr}");
        assert_eq!(ended.left, Some(Vec::new()), "{run}: no process outlived the box: {stderr}");
        let wrote = format!("box: wrote the configuration, {}\n", paths[0].join("temper.json").display());
        assert_eq!(stderr.contains(&wrote), run == "first", "{run}: {stderr}");
        assert_demo_streams(&ended.stdout, &ended.stderr);
        assert_eq!(ended.dormant_checked, run == "first", "the welcome precedes any fake start");
        if let Some(previous) = bot {
            assert_eq!(ended.checked, previous, "the restarted fake connects as the same bot");
        }
        bot = Some(ended.checked);
        for name in ["admin_token", "api_token", "git_token", "user", "webhook_secret"] {
            assert!(!paths[1].join("demo/forge").join(name).exists(), "{run}: no demo/forge/{name}");
        }
    }
    scratch.remove();
}
fn assert_demo_streams(stdout: &[u8], stderr: &[u8]) {
    let link = support::printed_link(stdout).expect("the person's sign-in link");
    assert!(skein_lib::bytes::find(link, b"/jig/v1/sign-in/link?secret=").is_some());
    assert_eq!(stdout, [link, b"\n"].concat(), "exactly one stdout line");
    let stderr = std::str::from_utf8(stderr).expect("demo diagnostics");
    assert!(!stderr.contains("temper fake seed"), "the engine owns the demo's start: {stderr}");
    let steps = ["fake llm: ", "prepare 1: ", "fake forge 1: ", "serve: ", "box: "];
    let no_bundle = "init: this temper has no browser (built with TEMPER_BUNDLE=none): the browser is not served";
    let no_git = "init: no git on PATH: a real forge's repositories need git 2.41 or later, set as workers[].git";
    for line in stderr.lines() {
        assert!(
            steps.iter().any(|prefix| line.starts_with(prefix))
                || line == no_git
                || (temper::bundle::OWN.is_none() && line == no_bundle),
            "unprefixed demo diagnostic: {line}"
        );
    }
}
// Read the connected bot through temper without opening a forge credential.
fn demo_bot(config: &std::path::Path, root: &std::path::Path) -> u64 {
    // NativeClient keeps the token; ctl also needs the public endpoint address.
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config.join("temper.json")).expect("written configuration"))
            .expect("JSON");
    let address = document["application"]["clients"]["listener"]["socket"].as_str().expect("client address");
    support::write_private(&root.join("private/address"), format!("http://{address}\n").as_bytes());
    let private = root.join("private");
    let call = support::ctl::Call::bare(&[private.to_str().expect("client path"), "forges"]);
    let ended = support::ctl::alone(root, call);
    assert_eq!(ended.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&ended.stderr));
    let stdout = String::from_utf8(ended.stdout).expect("forges row");
    let id = stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("forge 1 · Forgejo · fake · Connected as temper (")?
                .strip_suffix(')')?
                .parse::<u64>()
                .ok()
        })
        .expect("forge 1 is Connected as temper");
    assert_ne!(id, 0);
    id
}

// setup.md, 13.4: temper's landed id names the seeded tree plus the scripted edit.
fn demo_landed(config: &std::path::Path, root: &std::path::Path) {
    use std::collections::BTreeMap;
    let private = root.join("private");
    let call = support::ctl::Call::bare(&[private.to_str().expect("client path"), "task", "2"]);
    let ended = support::ctl::alone(root, call);
    assert_eq!(ended.exit, Some(Exit::Code(0)), "{}", String::from_utf8_lossy(&ended.stderr));
    let task = String::from_utf8(ended.stdout).expect("change read");
    let hex = task
        .lines()
        .find_map(|line| line.strip_prefix("result: Landed · merge commit "))
        .expect("the delegated change's full landed merge id");
    let commit = temper_worker_protocol_git::invocation::object(hex.as_bytes()).expect("merge hex");
    let merge = skein_fake_forge_protocol::routes::commit(&skein_forge::forgejo::id::ObjectId {
        format: skein_forge::forgejo::id::ObjectFormat::Sha1,
        bytes: commit.raw(),
    })
    .expect("the public fake commit spelling");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config.join("temper.json")).expect("written configuration"))
            .expect("JSON");
    let address = document["application"]["forges"][0]["api"]["socket"]
        .as_str()
        .expect("demo fake address")
        .parse()
        .expect("loopback socket");
    let landed = support::fake_objects(address, merge);
    let landed = &landed["objects"][0];
    assert_eq!(landed["commit"], merge);
    // The fake seeds its one repository with root commit 1 on every start.
    assert_eq!(landed["parent"], 1, "the merge's first parent is the seeded head");
    let seed = support::fake_objects(address, landed["parent"].as_u64().expect("first parent"));
    let seed = &seed["objects"][0];
    assert_eq!(seed["commit"], 1);
    assert!(seed["parent"].is_null() && seed["merge_parent"].is_null(), "the seed is a root");
    let tree = |object: &serde_json::Value| -> BTreeMap<Vec<u8>, Vec<u8>> {
        let decode = |value: &serde_json::Value| {
            skein_lib::base64::decode(
                skein_lib::base64::Alphabet::Standard,
                value.as_str().expect("base64 file").as_bytes(),
                8192,
            )
            .expect("file bytes")
            .into_vec()
        };
        object["tree"]
            .as_array()
            .expect("commit tree")
            .iter()
            .map(|file| (decode(&file["path"]), decode(&file["content"])))
            .collect()
    };
    let mut expected: BTreeMap<Vec<u8>, Vec<u8>> =
        temper_fake::forge::FILES.into_iter().map(|(path, bytes)| (path.to_vec(), bytes.to_vec())).collect();
    assert_eq!(tree(seed), expected, "the seed holds temper-fake's files");
    let script = document["application"]["demo"]["llm"]["cued"]
        .as_array()
        .expect("demo scripts")
        .iter()
        .find(|script| script["cue"] == "Export widget's table as CSV")
        .expect("coding script");
    let edit = &script["turns"][1]["lines"][0]["arguments"]["edits"][0];
    let old = edit["old"].as_str().expect("old main");
    let new = edit["new"].as_str().expect("new main");
    let main = expected.get_mut(b"src/main.rs".as_slice()).expect("seeded main");
    let source = std::str::from_utf8(main).expect("seeded source");
    assert_eq!(source.matches(old).count(), 1, "one coding edit in the seed");
    *main = source.replacen(old, new, 1).into_bytes();
    assert_eq!(tree(landed), expected, "the merge holds exactly the coding agent's edit");
}

// protocol/README.md, 15.6: the description's first process stories.
struct ClusterJudge {
    members: support::cluster::Members,
    deadline: skein_lib::Time,
    workers: bool,
}
impl skein_world::Referee<support::cluster::Root> for ClusterJudge {
    fn act(&mut self, now: skein_lib::Time, hosts: &mut [support::cluster::Root]) {
        if self.members.is_up() {
            if self.workers {
                for worker in ["w1", "w2"] {
                    assert_eq!(
                        self.members.capture(worker, "engine"),
                        self.members.capture("engine", "workers"),
                        "{worker} linked to this engine"
                    );
                }
            }
            self.members.stop();
        }
        self.members.act(now, hosts);
    }
    fn observe(&mut self, now: skein_lib::Time, hosts: &[support::cluster::Root]) {
        self.members.observe(now, hosts);
        if let Some(stopped) = self.members.stopped() {
            if self.workers {
                assert_eq!(
                    stopped.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                    ["w2", "w1", "engine", "llm", "forge"]
                );
            } else {
                let [engine, llm, forge] = stopped else { panic!("engine, llm and forge stopped") };
                assert_eq!(engine.name, "engine");
                assert_eq!(llm.name, "llm");
                assert_eq!(forge.name, "forge");
                engine_lines(engine);
            }
            for member in stopped {
                clean_cluster_exit(member);
            }
        }
    }
    fn next_deadline(&self) -> Option<skein_lib::Time> {
        Some(self.members.next_deadline().map_or(self.deadline, |at| at.min(self.deadline)))
    }
    fn overdue(&self, now: skein_lib::Time) -> Option<String> {
        self.members.overdue(now).or_else(|| {
            (now >= self.deadline && !self.members.passed()).then(|| "cluster did not settle within WITHIN".into())
        })
    }
    fn passed(&self) -> bool {
        self.members.passed()
    }
}
fn cluster_story(scratch: &std::path::Path, workers: bool) {
    let cluster = support::cluster::Cluster::read(scratch);
    let (mut members, hosts) = cluster.members();
    members.up(if workers { &["w1", "w2"] } else { &["engine"] });
    let clock = skein_shell::Clock::new();
    let mut world = skein_world::real::World::new(ClusterJudge {
        members,
        deadline: clock.now().now.saturating_add(support::WITHIN),
        workers,
    });
    for host in hosts {
        world.spawn(|| host);
    }
    let outcome = world.run(&clock, support::WITHIN);
    assert!(outcome.procs.iter().all(skein_shell::Host::is_empty), "every cluster root settled");
}
fn clean_cluster_exit(stopped: &support::cluster::Stopped) {
    assert_eq!(stopped.exit, Some(Exit::Code(0)), "{}: {}", stopped.name, String::from_utf8_lossy(&stopped.stderr));
    assert!(!stopped.killed, "{} stopped before STOP_SECONDS", stopped.name);
    assert!(stopped.survivors.is_empty(), "{} left no survivor", stopped.name);
}
#[test]
fn an_engine_alone_prints_its_lines_and_drains_on_a_signal() {
    let scratch = support::Scratch::new("cluster_engine_alone");
    cluster_story(scratch.path(), false);
    scratch.remove();
}
fn engine_lines(engine: &support::cluster::Stopped) {
    let stderr = std::str::from_utf8(&engine.stderr).expect("engine lines");
    let lines: Vec<_> = stderr.lines().collect();
    let [_, _, _] = lines.as_slice() else { panic!("exactly the three listening lines") };
    let clients: std::net::SocketAddr = lines
        .iter()
        .find_map(|line| line.strip_prefix("listening clients "))
        .expect("clients line")
        .parse()
        .expect("clients address");
    let workers: std::net::SocketAddr = lines
        .iter()
        .find_map(|line| line.strip_prefix("listening workers "))
        .expect("workers line")
        .parse()
        .expect("workers address");
    let webhooks: std::net::SocketAddr = lines
        .iter()
        .find_map(|line| line.strip_prefix("listening webhooks "))
        .expect("webhooks line")
        .parse()
        .expect("webhooks address");
    for address in [clients, workers, webhooks] {
        assert_eq!(address.ip(), std::net::Ipv4Addr::LOCALHOST);
        assert_ne!(address.port(), 0, "a bound port");
    }
    assert_ne!(clients.port(), workers.port(), "distinct listeners");
    assert_ne!(clients.port(), webhooks.port(), "distinct listeners");
    assert_ne!(workers.port(), webhooks.port(), "distinct listeners");
    let stdout = std::str::from_utf8(&engine.stdout).expect("link text");
    let link = stdout.strip_suffix('\n').expect("terminated sign-in link");
    assert!(!link.contains('\n'), "exactly one stdout line");
    let secret = link
        .strip_prefix(&format!("http://{clients}/jig/v1/sign-in/link?secret="))
        .expect("sign-in link at the clients listener");
    assert!(!secret.is_empty(), "a sign-in secret");
}

#[test]
fn a_mixed_document_exits_1_naming_its_path() {
    use skein_world::end_to_end::{Binary, Command, Expectation, Mode};
    struct Refusal {
        deadline: skein_lib::Time,
        settled: bool,
    }
    impl skein_world::Referee<Binary> for Refusal {
        fn act(&mut self, _now: skein_lib::Time, _hosts: &mut [Binary]) {}
        fn observe(&mut self, _now: skein_lib::Time, hosts: &[Binary]) {
            self.settled = hosts.iter().all(skein_shell::Host::is_empty);
        }
        fn next_deadline(&self) -> Option<skein_lib::Time> {
            Some(self.deadline)
        }
        fn overdue(&self, now: skein_lib::Time) -> Option<String> {
            (now >= self.deadline && !self.settled).then(|| "mixed document did not exit".into())
        }
        fn passed(&self) -> bool {
            self.settled
        }
    }
    let scratch = support::Scratch::new("cluster_mixed_document");
    let cluster = support::cluster::Cluster::documents(
        scratch.path(),
        &[("forge", "address", "127.0.0.1:3"), ("llm", "address", "127.0.0.1:1"), ("engine", "workers", "127.0.0.1:2")],
    );
    let path = cluster.dir("engine").join("config/temper.json");
    let mut engine: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("engine document")).expect("engine JSON");
    let worker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cluster.dir("w1").join("config/temper.json")).expect("worker document"))
            .expect("worker JSON");
    engine["workers"] = worker["workers"].clone();
    std::fs::write(&path, serde_json::to_vec(&engine).expect("mixed JSON")).expect("mixed document");
    let directory = cluster.dir("engine");
    let binary = Binary::start_with_tree(
        Command {
            program: env!("CARGO_BIN_EXE_temper").into(),
            arguments: vec![
                "serve".into(),
                "--config".into(),
                directory.join("config").into(),
                "--data".into(),
                directory.join("data").into(),
                "--cache".into(),
                directory.join("cache").into(),
            ],
            environment: Vec::new(),
            directory,
        },
        Mode::Pipes,
        65_536,
        Expectation::Supervise,
        false,
    )
    .expect("mixed serve starts");
    let clock = skein_shell::Clock::new();
    let mut world = skein_world::real::World::new(Refusal {
        deadline: clock.now().now.saturating_add(support::WITHIN),
        settled: false,
    });
    world.spawn_with_fds(binary.descriptors(), || binary);
    let outcome = world.run(&clock, support::WITHIN);
    let [binary] = outcome.procs.as_slice() else { panic!("one serve") };
    assert_eq!(binary.exit_status(), Some(Exit::Code(1)));
    assert_eq!(binary.stderr(), b"temper serve: workers: an engine alone takes no local worker, one plaintext loopback listener and workers with distinct digest paths\n");
    scratch.remove();
}

#[test]
fn the_cluster_stops_in_reverse_each_exiting_0() {
    let scratch = support::Scratch::new("cluster_reverse_stop");
    cluster_story(scratch.path(), true);
    scratch.remove();
}

// The worker retains the owned example's limits; derive its exact memory
// envelope through this fixture's agent reader and process preparation.
fn prepare_owned_worker(members: &mut support::cluster::Members) {
    let directory = members.prepare("w1");
    let configuration = directory.join("config");
    let agent_path = configuration.join("agent.json");
    let mut agent: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/examples/owned/agent.json")).expect("owned agent");
    agent["programs"]["claude_code"]["directory"] = directory.join("programs").to_str().expect("programs").into();
    support::write_private(&agent_path, &serde_json::to_vec(&agent).expect("agent JSON"));
    let example: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/examples/owned.json")).expect("owned example");
    let mut document = serde_json::json!({
        "workers": example["workers"],
        "application": {"workers": example["application"]["workers"], "limits": example["application"]["limits"]}
    });
    let worker = &mut document["workers"][0];
    worker["engine"] = serde_json::json!({"socket": members.capture("engine", "workers").expect("engine listener")});
    worker["name"] = "w1".into();
    worker["secret"] = "secrets/w1.secret".into();
    worker["agent"] = serde_json::json!({
        "program": env!("CARGO_BIN_EXE_temper"), "configuration": agent_path,
        "arguments": ["agent", agent_path]
    });
    // Derive through the binary's actual agent reader and process preparation.
    worker["memory_bytes"] = (1_u64 << 52).into();
    let arguments = [
        "--config".to_owned(),
        configuration.to_str().expect("config path").to_owned(),
        "--data".to_owned(),
        directory.join("data").to_str().expect("data path").to_owned(),
        "--cache".to_owned(),
        directory.join("cache").to_str().expect("cache path").to_owned(),
    ];
    let arguments: Vec<Box<[u8]>> = arguments.iter().map(|s| s.as_bytes().into()).collect();
    let (directories, _) = temper::directories::resolve(&arguments, &[], b"/").expect("worker directories");
    let (bytes, operations) = temper::envelope_in(&directories, &serde_json::to_vec(&document).expect("worker JSON"))
        .expect("owned worker envelope");
    assert!(operations <= 32_768, "Linux IORING_MAX_ENTRIES");
    document["workers"][0]["memory_bytes"] = bytes.into();
    support::write_private(&configuration.join("temper.json"), &serde_json::to_vec(&document).expect("worker JSON"));
}

enum OwnedWorkerPhase {
    Engine,
    Worker,
    Linked,
    Stopping,
}
struct OwnedWorkerJudge {
    members: support::cluster::Members,
    deadline: skein_lib::Time,
    phase: OwnedWorkerPhase,
}
impl skein_world::Referee<support::cluster::Root> for OwnedWorkerJudge {
    fn act(&mut self, now: skein_lib::Time, hosts: &mut [support::cluster::Root]) {
        if self.members.is_up() {
            match self.phase {
                OwnedWorkerPhase::Engine => {
                    let directory = self.members.prepare("engine");
                    let path = directory.join("config/temper.json");
                    let mut engine: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).expect("engine document")).expect("engine JSON");
                    // Negotiate the owned worker's full terms, including its turn bounds.
                    let example: serde_json::Value =
                        serde_json::from_str(include_str!("../../../docs/examples/owned.json")).expect("owned example");
                    engine["engine"]["limits"] = example["engine"]["limits"].clone();
                    engine["application"]["limits"] = example["application"]["limits"].clone();
                    // The example's 17 s worker stop bound must fit the engine's grace.
                    engine["engine"]["limits"]["overrides"]["fleet.grace_ms"] = 30_000.into();
                    support::write_private(&path, &serde_json::to_vec(&engine).expect("engine JSON"));
                    self.members.up(&["engine"]);
                    self.phase = OwnedWorkerPhase::Worker;
                }
                OwnedWorkerPhase::Worker => {
                    prepare_owned_worker(&mut self.members);
                    self.members.up(&["w1"]);
                    self.phase = OwnedWorkerPhase::Linked;
                }
                OwnedWorkerPhase::Linked => {
                    assert_eq!(self.members.capture("w1", "engine"), self.members.capture("engine", "workers"));
                    assert_eq!(self.members.linked("w1"), 1, "the owned worker linked");
                    self.members.stop();
                    self.phase = OwnedWorkerPhase::Stopping;
                }
                OwnedWorkerPhase::Stopping => {}
            }
        }
        self.members.act(now, hosts);
    }
    fn observe(&mut self, now: skein_lib::Time, hosts: &[support::cluster::Root]) {
        self.members.observe(now, hosts);
        if let Some(stopped) = self.members.stopped() {
            assert_eq!(stopped.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["w1", "engine", "llm", "forge"]);
            for member in stopped {
                clean_cluster_exit(member);
            }
        }
    }
    fn next_deadline(&self) -> Option<skein_lib::Time> {
        Some(self.members.next_deadline().map_or(self.deadline, |at| at.min(self.deadline)))
    }
    fn overdue(&self, now: skein_lib::Time) -> Option<String> {
        self.members.overdue(now).or_else(|| {
            (now >= self.deadline && !self.members.passed()).then(|| "owned worker did not settle within WITHIN".into())
        })
    }
    fn passed(&self) -> bool {
        self.members.passed()
    }
}
#[test]
fn a_worker_alone_with_the_owned_examples_worker_links_and_drains_on_a_signal() {
    let scratch = support::Scratch::new("owned_worker_alone_ring");
    let (mut members, hosts) = support::cluster::Cluster::read(scratch.path()).members();
    members.up(&["llm"]);
    let clock = skein_shell::Clock::new();
    let mut world = skein_world::real::World::new(OwnedWorkerJudge {
        members,
        deadline: clock.now().now.saturating_add(support::WITHIN),
        phase: OwnedWorkerPhase::Engine,
    });
    for host in hosts {
        world.spawn(|| host);
    }
    let outcome = world.run(&clock, support::WITHIN);
    assert!(outcome.procs.iter().all(skein_shell::Host::is_empty), "every cluster root settled");
    scratch.remove();
}

// Both the idle threshold and ordinary time allowance hold an open chat's slot.
// Past the story's deadline, a run that parks has already failed it.
const CHAT_HOLDS_ITS_SLOT: Duration = support::WITHIN.saturating_mul(2);

// A lost link heals: a stall past the 1 s silence loses it. The worker's contact
// grace, graces.grace_ms 1000 (temper-service worker_settings.rs:145; jig 8f1acb2
// worker-link component.rs:629), outlasts two redials (125–250 ms, doubling:
// backoff.rs:16; profile/small.rs:239); a re-hello clears it (component.rs:455).
// The engine keeps that channel's attempts adrift for fleet.grace_ms 3000
// (jig-domain-fleet channel.rs:250); re-hello lists them back (attempt.rs:49),
// so nothing is placed twice.
#[test]
fn a_task_runs_on_each_worker() {
    cluster_chats("cluster_each_worker", ClusterStory::Each);
}

// A waiting reply is uncommitted: killing w1 loses it. The retry on w2
// resumes without an assistant message, so the same cue replies and waits
// again. Only after close/end is that reply committed, exactly once; the
// old attempt ends lost and the replacement parks. This covers loss
// and retry; the hung-worker story will test fencing a surviving old run.
#[test]
fn a_worker_killed_mid_run_is_lost_and_its_run_answered_once_on_another() {
    cluster_chats("cluster_killed", ClusterStory::Kill);
}

// protocol/README.md, 15.6: heal within fleet grace, then fence a surviving run.
// Continue follows the connected:false read immediately. CLOCK_MONOTONIC keeps
// advancing during STOP (skein f5f8b3e, skein-shell clock.rs:17), but w1 arms its
// contact grace only on detecting its own loss after CONT (jig e7a93cd,
// jig-protocol-worker-link component.rs:169, :622). Its first redial is 125–250 ms
// (backoff.rs:15; jig-protocol-config profile/small.rs:239); its re-hello must
// reach the engine within the 3000 ms fleet grace (jig-domain-fleet
// attempt.rs:505). This is a margin, checked by the unchanged attempt below.
#[test]
fn a_worker_hung_past_the_grace_is_fenced_and_links_again() {
    cluster_chats("cluster_hung", ClusterStory::Hung);
}

// Jig admits only with an available placement (e7a93cd, jig-domain-core
// routing/run_preparation.rs:466–492, run_choice.rs:73–77): the second task
// is due, unclaimed and unplaced; no attempt exists to be placed twice.
#[test]
fn a_worker_started_later_takes_the_waiting_task() {
    cluster_chats("cluster_started_later", ClusterStory::Later);
}

#[test]
fn a_worker_restarted_on_its_directories_links_at_its_position() {
    cluster_chats("cluster_restarted", ClusterStory::Restart);
}

#[test]
fn an_unexpected_member_exit_is_reported_after_its_client_settles() {
    let failure = std::panic::catch_unwind(|| cluster_chats("cluster_unexpected", ClusterStory::Unexpected))
        .expect_err("unexpected member fails the story");
    let message = failure.downcast_ref::<String>().expect("member diagnostic");
    assert!(message.contains("w1 exited unexpectedly: Some(Signal(9))"), "{message}");
    assert!(message.contains("linked w1 "), "stderr tail: {message}");
    assert!(!message.contains("EBUSY") && !message.contains("ResourceBusy"), "{message}");
}

fn cluster_chats(case: &str, story: ClusterStory) {
    let scratch = support::Scratch::new(case);
    let mut cluster = support::cluster::Cluster::read(scratch.path());
    if matches!(story, ClusterStory::Kill | ClusterStory::Hung) {
        cluster.record_llm();
    }
    let engine = cluster.dir("engine").join("config/temper.json");
    let key = cluster.dir("llm").join("llm.key");
    let (mut members, hosts) = cluster.members();
    members.up(if story == ClusterStory::Each { &["w1", "w2"] } else { &["w1"] });
    let clock = skein_shell::Clock::new();
    let mut world = skein_world::real::World::new(ClusterChatJudge {
        members,
        deadline: clock.now().now.saturating_add(support::WITHIN),
        engine,
        key,
        client: scratch.path().join("client"),
        stage: ClusterChatStage::Start,
        tasks: Vec::new(),
        workers: std::collections::BTreeSet::new(),
        story,
        reading: None,
        checking: std::collections::VecDeque::new(),
        last_read: String::new(),
        lost_attempt: None,
        healed_attempt: None,
        linked_before: 0,
        fenced: None,
    });
    for host in hosts {
        world.spawn(|| host);
    }
    world.spawn(support::cluster::Root::client);
    let outcome = world.run(&clock, support::WITHIN);
    assert!(outcome.procs.iter().all(skein_shell::Host::is_empty), "every cluster root settled");
    eprintln!("cluster chats settled in {:?}", outcome.end.saturating_since(outcome.start));
    scratch.remove();
}

// lab/cluster has two chats, one per slot, and two lives in the restart story.
const CLUSTER_CHATS: usize = 2;
const FIRST_LIFE: usize = 0;
const SECOND_LIFE: usize = 1;
const RESTART_LINKS: usize = 2;
const REPLY_ONCE: usize = 1;
const FIRST_LINK: usize = 1;
const LOST_AND_RETRY: usize = 2; // the killed run and its replacement
const KILL_SIGNAL: u32 = 9; // Linux SIGKILL, as reported by wait
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClusterStory {
    Each,
    Unexpected,
    Kill,
    Hung,
    Later,
    Restart,
}
#[derive(Debug)]
enum ClusterChatStage {
    Start,
    Created,
    Replied,
    WaitingSlot,
    Crashed,
    Armed,
    Killed,
    HealSilence,
    HealRelink,
    FenceReady,
    FenceReplacement,
    FenceSnapshot,
    FenceRelink,
    FenceCheck,
    StoppedOne,
    Relinking,
    Closed,
    Verifying,
    Stopping,
}
struct ClusterChatJudge {
    members: support::cluster::Members,
    deadline: skein_lib::Time,
    engine: std::path::PathBuf,
    key: std::path::PathBuf,
    client: std::path::PathBuf,
    stage: ClusterChatStage,
    tasks: Vec<String>,
    workers: std::collections::BTreeSet<String>,
    story: ClusterStory,
    reading: Option<ClusterRead>,
    checking: std::collections::VecDeque<String>,
    last_read: String,
    lost_attempt: Option<u64>,
    healed_attempt: Option<u64>,
    linked_before: usize,
    fenced: Option<(Vec<serde_json::Value>, Vec<serde_json::Value>)>,
}
impl ClusterChatJudge {
    fn queue(&self, host: &mut support::cluster::Root, calls: Vec<support::ctl::Call>) {
        let link = format!("http://{}", self.members.capture("engine", "link").expect("engine link"));
        let mut client = support::ctl::Calls::linked(&link, &self.client);
        for call in calls {
            client.push(support::ctl::Step::Run(call));
        }
        let support::cluster::Root::Client { calls, .. } = host else { panic!("client root") };
        *calls = Some(Box::new(client));
    }
    fn begin(&mut self, client: &mut support::cluster::Root) {
        use support::ctl::{Call, PROJECT};
        let engine: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&self.engine).expect("rendered engine document"))
                .expect("engine JSON");
        let template = engine["engine"]["templates"]
            .as_array()
            .expect("templates")
            .iter()
            .find(|template| template["name"] == "chat")
            .expect("chat template");
        let charter = engine["engine"]["charters"]
            .as_array()
            .expect("charters")
            .iter()
            .find(|charter| charter["number"] == template["executor"]["charter"])
            .expect("chat charter");
        assert!(
            Duration::from_millis(charter["waiting_ms"].as_u64().expect("chat waiting_ms")) >= CHAT_HOLDS_ITS_SLOT,
            "lab/cluster/engine.json: chat charter waiting_ms must hold its slot past the story deadline"
        );
        let time = charter["time_ms"].as_u64().expect("chat time_ms");
        let reserve = charter["reserve_time_ms"].as_u64().expect("chat reserve_time_ms");
        assert!(
            time.checked_sub(reserve).is_some_and(|ordinary| Duration::from_millis(ordinary) >= CHAT_HOLDS_ITS_SLOT),
            "lab/cluster/engine.json: chat charter time_ms less its reserve must hold its slot past the story deadline"
        );
        self.queue(
            client,
            vec![
                Call::sign_in(),
                Call::new(&[
                    "add-key",
                    "1",
                    self.key.to_str().expect("LLM key path"),
                    "--deployment",
                    "--key",
                    "cluster-llm",
                ]),
                Call::new(&["wait", "accounts", "--until", "usable"]),
                Call::new(&["chat", "--project", PROJECT, "--key", "first", "cluster-first-chat", "--json"]),
            ],
        );
        self.stage = ClusterChatStage::Created;
    }
    fn create_second(&mut self, client: &mut support::cluster::Root) {
        use support::ctl::{Call, PROJECT};
        self.queue(
            client,
            vec![Call::new(&["chat", "--project", PROJECT, "--key", "second", "cluster-second-chat", "--json"])],
        );
        self.stage = ClusterChatStage::Created;
    }
    fn current(&self) -> &str {
        self.checking.front().or_else(|| self.tasks.last()).expect("current chat")
    }
    fn poll(&self, client: &mut support::cluster::Root) {
        use support::ctl::Call;
        let task = self.current();
        let calls = if matches!(
            self.stage,
            ClusterChatStage::Verifying | ClusterChatStage::FenceSnapshot | ClusterChatStage::FenceCheck
        ) {
            vec![Call::new(&["task", task, "--activity", &temper_client_tools::limits::ROWS.to_string(), "--json"])]
        } else if matches!(self.stage, ClusterChatStage::Replied) {
            // The live reply watch returns a task; read.waiting() still checks its exact work and placement.
            vec![Call::new(&["wait", "task", task, "--until", "replied", "--json"]), Call::new(&["accounts", "--json"])]
        } else {
            vec![Call::new(&["task", task, "--json"]), Call::new(&["accounts", "--json"])]
        };
        self.queue(client, calls);
    }
    fn read(&mut self, client: &mut support::cluster::Root, ends: &[support::Ended]) -> Option<ClusterRead> {
        use support::ctl::{Call, json};
        if self.reading.as_ref().is_some_and(|read| read.turns) {
            let mut read = self.reading.take().expect("turn reads");
            let turns = read.rows.iter_mut().filter(|row| row["event"]["owner"]["source"]["line"]["turn"].is_object());
            for (row, ended) in turns.zip(ends) {
                let turn = json(ended);
                complete_cluster_json(&turn);
                assert!(turn["turn"]["decoded"].is_object(), "committed turn decoded: {turn}");
                row["event"]["owner"]["decoded"] = turn;
            }
            return Some(read);
        }
        let (mut read, page) = match ends {
            [task, accounts] => {
                let task = json(task);
                let task = if matches!(self.stage, ClusterChatStage::Replied) { task["task"].clone() } else { task };
                return Some(ClusterRead::new(task, json(accounts)));
            }
            [activity] if self.reading.is_some() => (self.reading.take().expect("continued activity"), json(activity)),
            [packet] => {
                let packet = json(packet);
                (ClusterRead::new(packet["task"].clone(), serde_json::Value::Null), packet["activity"].clone())
            }
            _ => panic!("task/accounts, task with activity, or continued activity"),
        };
        complete_cluster_json(&page);
        read.rows.extend(page["rows"].as_array().expect("activity rows").iter().cloned());
        let task = self.current();
        if let Some(before) = page["before"]["value"].as_str() {
            self.queue(client, vec![Call::new(&["activity", task, "--before", before, "--json"])]);
        } else {
            let calls: Vec<_> = read
                .rows
                .iter()
                .filter_map(|row| {
                    let turn = row.pointer("/event/owner/source/line/turn")?;
                    let attempt = turn["attempt"].as_u64().expect("turn attempt");
                    let number = turn["turn"].as_u64().expect("turn number");
                    Some(Call::new(&["turn", task, &format!("{attempt}.{number}"), "--json"]))
                })
                .collect();
            if calls.is_empty() {
                return Some(read);
            }
            self.queue(client, calls);
            read.turns = true;
        }
        self.reading = Some(read);
        None
    }
    fn advance_read(&mut self, client: &mut support::cluster::Root, read: &ClusterRead) {
        self.last_read = read.owner().to_string();
        match self.stage {
            ClusterChatStage::WaitingSlot => {
                if read.owner()["phase"] == serde_json::json!({"active": "due"})
                    && read.owner()["details"]["attempt"].is_null()
                    && read.owner()["details"]["observation"].is_null()
                {
                    self.members.up(&["w2"]);
                    self.stage = ClusterChatStage::Replied;
                }
                self.poll(client);
            }
            ClusterChatStage::Relinking => {
                if self.members.linked("w1") == RESTART_LINKS && read.connected("w1") {
                    self.create_second(client);
                } else {
                    self.poll(client);
                }
            }
            ClusterChatStage::Replied => self.replied(client, read),
            ClusterChatStage::HealSilence
            | ClusterChatStage::HealRelink
            | ClusterChatStage::FenceReady
            | ClusterChatStage::FenceReplacement
            | ClusterChatStage::FenceSnapshot
            | ClusterChatStage::FenceRelink
            | ClusterChatStage::FenceCheck => self.hung_read(client, read),
            ClusterChatStage::Armed => {
                if self.members.is_up() && self.members.linked("w2") >= FIRST_LINK && read.connected("w2") {
                    self.members.kill("w1");
                    self.stage = ClusterChatStage::Killed;
                }
                self.poll(client);
            }
            ClusterChatStage::Killed => {
                if self.members.ended("w1")
                    && read.worker() == Some("w2")
                    && read.waiting()
                    && Some(read.attempt()) != self.lost_attempt
                {
                    self.close(client, false);
                } else {
                    self.poll(client);
                }
            }
            ClusterChatStage::Verifying => self.verify(client, read),
            ClusterChatStage::Start
            | ClusterChatStage::Created
            | ClusterChatStage::StoppedOne
            | ClusterChatStage::Crashed
            | ClusterChatStage::Closed
            | ClusterChatStage::Stopping => unreachable!("read stage"),
        }
    }
    fn verify(&mut self, client: &mut support::cluster::Root, read: &ClusterRead) {
        if matches!(self.story, ClusterStory::Kill | ClusterStory::Hung) {
            // Keep causal client evidence beside the fake's query records on failure.
            // Neither includes the linked client's sign-in link or engine stdout.
            let path = self.members.dir("llm").join(format!("task-{}-attempt-{}.json", self.current(), read.attempt()));
            let evidence = serde_json::json!({"task": &read.task, "activity": &read.rows});
            support::write_private(&path, &serde_json::to_vec(&evidence).expect("chat evidence"));
            eprintln!("cluster close evidence: {}", path.display());
        }
        // The chat is ended by its requester (domain/agent.md, 4.6): its person's Close
        // asks its Open run to wind down; the final text parks with no task result.
        let cancelled = &read.owner()["details"]["ending"]["cancelled"];
        assert_eq!(cancelled["by"]["person"], 1, "chat closed by its person: {}", read.task);
        assert!(cancelled["result"].is_null(), "a closed chat keeps no finish's result: {}", read.task);
        let text = if self.tasks.first().is_some_and(|task| task == self.current())
            || (self.story == ClusterStory::Hung && self.tasks.get(1).is_some_and(|task| task == self.current()))
        {
            "First worker replied."
        } else {
            "Second worker replied."
        };
        assert_eq!(read.replies(text), REPLY_ONCE, "reply once across complete committed turns");
        if self.story == ClusterStory::Kill
            || (self.story == ClusterStory::Hung && self.tasks.get(1).is_some_and(|task| task == self.current()))
        {
            self.loss(read);
        }
        if self.story == ClusterStory::Hung && self.tasks.len() == FIRST_LINK {
            let attempt = self.healed_attempt.expect("healed attempt");
            assert_eq!(read.attempt(), attempt, "heal retains its attempt");
            let lines: Vec<_> = read.rows.iter().map(|row| &row["event"]["owner"]["source"]["line"]).collect();
            let began: Vec<_> = lines.iter().filter(|line| line["run_began"].is_object()).collect();
            assert_eq!(began.len(), REPLY_ONCE, "heal places exactly once");
            assert_eq!(began[0]["run_began"]["attempt"], attempt);
            let ends: Vec<_> = lines.iter().filter(|line| line["run_ended"].is_object()).collect();
            assert_eq!(ends.len(), REPLY_ONCE, "same healed attempt ended once");
            assert_eq!(ends[0]["run_ended"]["attempt"], attempt);
            // The admitted finish request makes the Open run's next text park (jig's domain/hosts.md, 9).
            assert_eq!(ends[0]["run_ended"]["end"], "parked");
            self.checking.pop_front();
            self.queue(
                client,
                vec![support::ctl::Call::new(&[
                    "chat",
                    "--project",
                    support::ctl::PROJECT,
                    "--key",
                    "fence",
                    "cluster-first-chat",
                    "--json",
                ])],
            );
            self.stage = ClusterChatStage::Created;
            return;
        }
        self.checking.pop_front().expect("verified chat");
        if self.checking.is_empty() {
            if self.story == ClusterStory::Restart && self.tasks.len() < CLUSTER_CHATS {
                self.members.stop_one("w1");
                self.stage = ClusterChatStage::StoppedOne;
            } else {
                self.members.stop();
                self.stage = ClusterChatStage::Stopping;
            }
        } else {
            self.poll(client);
        }
    }
    fn replied(&mut self, client: &mut support::cluster::Root, read: &ClusterRead) {
        if self.story == ClusterStory::Hung {
            if read.worker() == Some("w1") && read.waiting() {
                match self.tasks.len() {
                    1 => {
                        self.healed_attempt = Some(read.attempt());
                        self.linked_before = self.members.linked("w1");
                        self.members.signal("w1", Signal::Stop);
                        self.stage = ClusterChatStage::HealSilence;
                    }
                    2 => {
                        self.lost_attempt = Some(read.attempt());
                        self.members.up(&["w2"]);
                        self.stage = ClusterChatStage::FenceReady;
                    }
                    3 => {
                        self.checking.push_back(self.tasks[1].clone());
                        self.stage = ClusterChatStage::FenceCheck;
                    }
                    _ => unreachable!("heal, fence and new chat"),
                }
            }
            self.poll(client);
            return;
        }
        let first = self.tasks.len() < CLUSTER_CHATS;
        if let Some(worker) = read.worker().filter(|_| read.waiting()) {
            eprintln!(
                "client placement: task {} attempt {} worker {worker}",
                self.tasks.last().expect("chat"),
                read.attempt()
            );
            if self.story == ClusterStory::Each {
                assert!(self.workers.insert(worker.into()), "each chat on its own worker");
            } else {
                assert_eq!(worker, if first || self.story == ClusterStory::Restart { "w1" } else { "w2" });
            }
            if self.story == ClusterStory::Unexpected {
                self.queue(
                    client,
                    vec![
                        support::ctl::Call::new(&["wait", "task", self.current(), "--until", "ended"]),
                        support::ctl::Call::new(&["unknown"]),
                    ],
                );
                self.members.signal("w1", Signal::Kill);
                self.stage = ClusterChatStage::Crashed;
            } else if self.story == ClusterStory::Kill {
                self.lost_attempt = Some(read.attempt());
                self.members.up(&["w2"]);
                self.stage = ClusterChatStage::Armed;
                self.poll(client);
            } else if first {
                if self.story == ClusterStory::Restart {
                    self.close(client, true);
                } else {
                    self.create_second(client);
                }
            } else {
                if self.story == ClusterStory::Each {
                    assert_eq!(self.workers.iter().map(String::as_str).collect::<Vec<_>>(), ["w1", "w2"]);
                }
                self.close(client, false);
            }
        } else {
            self.poll(client);
        }
    }
    fn hung_read(&mut self, client: &mut support::cluster::Root, read: &ClusterRead) {
        match self.stage {
            ClusterChatStage::HealSilence => {
                if read.hosts().iter().any(|host| host["name"] == "w1" && host["connected"] == false) {
                    self.members.signal("w1", Signal::Continue);
                    self.stage = ClusterChatStage::HealRelink;
                }
            }
            ClusterChatStage::HealRelink => {
                if self.members.linked("w1") > self.linked_before && read.connected("w1") {
                    assert_eq!(Some(read.attempt()), self.healed_attempt, "same attempt healed on w1");
                    self.close(client, true);
                    return;
                }
            }
            ClusterChatStage::FenceReady => {
                if self.members.is_up() && read.connected("w2") {
                    self.linked_before = self.members.linked("w1");
                    self.members.signal("w1", Signal::Stop);
                    self.stage = ClusterChatStage::FenceReplacement;
                }
            }
            ClusterChatStage::FenceReplacement => {
                if read.worker() == Some("w2") && read.waiting() && Some(read.attempt()) != self.lost_attempt {
                    self.stage = ClusterChatStage::FenceSnapshot;
                }
            }
            ClusterChatStage::FenceSnapshot => {
                self.fenced = Some(self.fenced_rows(read));
                self.members.signal("w1", Signal::Continue);
                self.stage = ClusterChatStage::FenceRelink;
            }
            ClusterChatStage::FenceRelink => {
                if self.members.linked("w1") > self.linked_before && read.connected("w1") {
                    self.create_second(client);
                    return;
                }
            }
            ClusterChatStage::FenceCheck => {
                // Jig e7a93cd: w1's hello listing b1 gets Cancel while b2 is
                // claimed (jig-domain-fleet attempt.rs:1030). Cancel ends b1's
                // run, completing its held reply turn; w1 sends that turn and
                // its terminal answer. The engine acknowledges the answer and
                // drops its payload (attempt.rs:539, :599), and drops any b1
                // turn (turn.rs:79). C waiting on w1's only slot proves w1
                // acted on Cancel: unchanged rows/turns show a drop, not silence.
                assert_eq!(Some(self.fenced_rows(read)), self.fenced, "fenced activity and committed turns unchanged");
                let old = self.lost_attempt.expect("fenced attempt");
                assert!(
                    read.rows.iter().any(|row| {
                        let end = &row["event"]["owner"]["source"]["line"]["run_ended"];
                        end["attempt"] == old && end["end"]["failed"]["class"] == "lost"
                    }),
                    "fenced attempt ended lost"
                );
                self.checking.clear();
                self.close(client, false);
                return;
            }
            ClusterChatStage::Start
            | ClusterChatStage::Created
            | ClusterChatStage::Replied
            | ClusterChatStage::WaitingSlot
            | ClusterChatStage::Crashed
            | ClusterChatStage::Armed
            | ClusterChatStage::Killed
            | ClusterChatStage::StoppedOne
            | ClusterChatStage::Relinking
            | ClusterChatStage::Closed
            | ClusterChatStage::Verifying
            | ClusterChatStage::Stopping => unreachable!("hung read stage"),
        }
        self.poll(client);
    }
    fn fenced_rows(&self, read: &ClusterRead) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        let old = self.lost_attempt.expect("fenced attempt");
        let activity = read
            .rows
            .iter()
            .filter(|row| {
                let line = &row["event"]["owner"]["source"]["line"];
                ["run_began", "run_ended", "turn"].iter().any(|kind| line[kind]["attempt"] == old)
            })
            .cloned()
            .collect();
        let turns = read
            .rows
            .iter()
            .filter(|row| row["event"]["owner"]["source"]["line"]["turn"].is_object())
            .cloned()
            .collect();
        (activity, turns)
    }
    fn loss(&self, read: &ClusterRead) {
        let old = self.lost_attempt.expect("killed attempt");
        let replacement = read.attempt();
        assert_ne!(replacement, old, "replacement attempt on w2");
        let lines: Vec<_> = read.rows.iter().map(|row| &row["event"]["owner"]["source"]["line"]).collect();
        assert_eq!(lines.iter().filter(|line| line["run_began"].is_object()).count(), LOST_AND_RETRY);
        for (attempt, end) in
            [(old, serde_json::json!({"failed": {"class": "lost"}})), (replacement, serde_json::json!("parked"))]
        {
            assert!(lines.iter().any(|line| line["run_began"]["attempt"] == attempt), "attempt began");
            let ended = lines.iter().find(|line| line["run_ended"]["attempt"] == attempt).expect("attempt ended");
            assert_eq!(ended["run_ended"]["end"], end);
        }
        assert!(!lines.iter().any(|line| line["turn"]["attempt"] == old), "killed reply was never committed");
    }
    fn close(&mut self, client: &mut support::cluster::Root, first_only: bool) {
        use support::ctl::Call;
        let mut close = Vec::new();
        for task in &self.tasks {
            if matches!(self.story, ClusterStory::Restart | ClusterStory::Hung)
                && !first_only
                && self.tasks.first() == Some(task)
            {
                continue;
            }
            self.checking.push_back(task.clone());
            close.push(Call::new(&["close", task, "--key", &format!("close-{task}")]));
            close.push(Call::new(&["wait", "task", task, "--until", "ended"]));
        }
        self.queue(client, close);
        self.stage = ClusterChatStage::Closed;
    }
}
struct ClusterRead {
    task: serde_json::Value,
    accounts: serde_json::Value,
    rows: Vec<serde_json::Value>,
    turns: bool,
}
impl ClusterRead {
    fn new(task: serde_json::Value, accounts: serde_json::Value) -> Self {
        Self { task, accounts, rows: Vec::new(), turns: false }
    }
    fn owner(&self) -> &serde_json::Value {
        &self.task["chip"]["details"]["owner"]
    }
    fn waiting(&self) -> bool {
        self.owner()["details"]["observation"]["work"] == "waiting"
    }
    fn attempt(&self) -> u64 {
        self.owner()["details"]["attempt"].as_u64().expect("chat attempt")
    }
    fn hosts(&self) -> &[serde_json::Value] {
        self.accounts["accounts"]["deployment_facts"]["program_hosts"].as_array().expect("deployment hosts")
    }
    fn connected(&self, name: &str) -> bool {
        self.hosts().iter().any(|host| host["name"] == name && host["connected"] == true)
    }
    fn worker(&self) -> Option<&str> {
        let channel = self.owner()["details"]["observation"]["channel"].as_u64()?;
        self.hosts().iter().find(|host| host["channel"] == channel && host["connected"] == true)?["name"].as_str()
    }
    fn replies(&self, text: &str) -> usize {
        self.rows
            .iter()
            .filter_map(|row| row["event"]["owner"]["decoded"]["turn"]["decoded"]["messages"].as_array())
            .flatten()
            .filter(|message| message["speaker"] == "assistant")
            .flat_map(|message| message["blocks"].as_array().expect("message blocks"))
            .filter_map(|block| block["text"]["text"].as_str())
            .map(|words| words.matches(text).count())
            .sum()
    }
}
fn complete_cluster_json(value: &serde_json::Value) {
    if let Some(cut) = value.get("cut") {
        assert_eq!(cut, false, "complete committed transcript");
    }
    if let Some(left) = value.get("bytes_left") {
        assert_eq!(left, 0, "complete turn text");
    }
    for item in
        value.as_object().into_iter().flat_map(serde_json::Map::values).chain(value.as_array().into_iter().flatten())
    {
        complete_cluster_json(item);
    }
}

impl skein_world::Referee<support::cluster::Root> for ClusterChatJudge {
    fn act(&mut self, now: skein_lib::Time, hosts: &mut [support::cluster::Root]) {
        use skein_shell::Host;
        use support::ctl::json;
        self.members.act(now, hosts);
        if self.members.failed() {
            self.stage = ClusterChatStage::Stopping;
            return;
        }
        let (client, _) = hosts.split_last_mut().expect("cluster client root");
        if matches!(self.stage, ClusterChatStage::StoppedOne) && self.members.ended("w1") {
            self.members.restart("w1");
            self.stage = ClusterChatStage::Relinking;
            self.poll(client);
        }
        if self.members.is_up() || self.members.ended("w1") {
            if matches!(self.stage, ClusterChatStage::Start) {
                self.begin(client);
            } else if client.is_empty() && !matches!(self.stage, ClusterChatStage::Start | ClusterChatStage::Stopping) {
                let support::cluster::Root::Client { calls, .. } = client else { panic!("client root") };
                let calls = calls.take().expect("completed cluster calls");
                for ended in calls.ends() {
                    assert_eq!(ended.exit, Some(Exit::Code(0)), "ctl: {}", String::from_utf8_lossy(&ended.stderr));
                }
                match self.stage {
                    ClusterChatStage::Created => {
                        let created = json(calls.ends().last().expect("created chat"));
                        let task = created["outcome"]["done"]["owner"]["created"]["first"]
                            .as_u64()
                            .unwrap_or_else(|| panic!("created chat number: {created}"))
                            .to_string();
                        self.tasks.push(task);
                        self.stage = if self.story == ClusterStory::Later && self.tasks.len() == CLUSTER_CHATS {
                            ClusterChatStage::WaitingSlot
                        } else {
                            ClusterChatStage::Replied
                        };
                        self.poll(client);
                    }
                    ClusterChatStage::Replied
                    | ClusterChatStage::WaitingSlot
                    | ClusterChatStage::Relinking
                    | ClusterChatStage::Verifying
                    | ClusterChatStage::Armed
                    | ClusterChatStage::Killed
                    | ClusterChatStage::HealSilence
                    | ClusterChatStage::HealRelink
                    | ClusterChatStage::FenceReady
                    | ClusterChatStage::FenceReplacement
                    | ClusterChatStage::FenceSnapshot
                    | ClusterChatStage::FenceRelink
                    | ClusterChatStage::FenceCheck => {
                        if let Some(read) = self.read(client, calls.ends()) {
                            self.advance_read(client, &read);
                        }
                    }
                    ClusterChatStage::Closed => {
                        self.stage = ClusterChatStage::Verifying;
                        self.poll(client);
                    }
                    ClusterChatStage::Start
                    | ClusterChatStage::Stopping
                    | ClusterChatStage::StoppedOne
                    | ClusterChatStage::Crashed => {
                        unreachable!("active calls")
                    }
                }
            }
        }
    }
    fn observe(&mut self, now: skein_lib::Time, hosts: &[support::cluster::Root]) {
        use skein_shell::Host;
        let (client, hosts) = hosts.split_last().expect("cluster client root");
        self.members.observe(now, hosts);
        if self.members.failed() {
            if self.story != ClusterStory::Unexpected {
                return;
            }
            let support::cluster::Root::Client { calls: Some(calls), .. } = client else {
                panic!("failed client's calls")
            };
            if self.story == ClusterStory::Unexpected && matches!(self.stage, ClusterChatStage::Crashed) {
                assert!(!calls.is_empty(), "in-flight child not yet reaped");
            }
            if self.story == ClusterStory::Unexpected && self.members.stopped().is_some() {
                assert!(calls.is_empty(), "client settled before remaining members stop");
                let [cancelled] = calls.ends() else { panic!("one in-flight call; queued unknown never started") };
                assert_eq!(cancelled.exit, Some(Exit::Signal(KILL_SIGNAL)), "client killed, waited and reaped");
            }
            return;
        }
        if let Some(stopped) = self.members.stopped() {
            let expected = if self.story == ClusterStory::Restart {
                vec![
                    ("w1", FIRST_LIFE),
                    ("w1", SECOND_LIFE),
                    ("engine", FIRST_LIFE),
                    ("llm", FIRST_LIFE),
                    ("forge", FIRST_LIFE),
                ]
            } else if self.story == ClusterStory::Kill {
                vec![
                    ("w1", FIRST_LIFE),
                    ("w2", FIRST_LIFE),
                    ("engine", FIRST_LIFE),
                    ("llm", FIRST_LIFE),
                    ("forge", FIRST_LIFE),
                ]
            } else {
                vec![
                    ("w2", FIRST_LIFE),
                    ("w1", FIRST_LIFE),
                    ("engine", FIRST_LIFE),
                    ("llm", FIRST_LIFE),
                    ("forge", FIRST_LIFE),
                ]
            };
            assert_eq!(stopped.iter().map(|s| (s.name.as_str(), s.life)).collect::<Vec<_>>(), expected);
            for member in stopped {
                if self.story == ClusterStory::Kill && member.name == "w1" {
                    assert_eq!(member.ending, support::cluster::Ending::Kill);
                    assert_eq!(member.exit, Some(Exit::Signal(KILL_SIGNAL)));
                    assert!(!member.killed && member.survivors.is_empty(), "intentional kill needed no cleanup");
                    continue;
                }
                let alone = self.story == ClusterStory::Restart && member.name == "w1" && member.life == FIRST_LIFE;
                assert_eq!(
                    member.ending,
                    if alone { support::cluster::Ending::Alone } else { support::cluster::Ending::Cluster }
                );
                clean_cluster_exit(member);
            }
        }
    }
    fn next_deadline(&self) -> Option<skein_lib::Time> {
        Some(self.members.next_deadline().map_or(self.deadline, |at| at.min(self.deadline)))
    }
    fn overdue(&self, now: skein_lib::Time) -> Option<String> {
        (now >= self.deadline && !self.members.passed()).then(|| {
            format!(
                "cluster {:?} waited in {:?}: {}; last read: {}",
                self.story,
                self.stage,
                match self.stage {
                    ClusterChatStage::Start => "requested members ready",
                    ClusterChatStage::Created => "chat creation",
                    ClusterChatStage::Replied => "waiting run and connected placement",
                    ClusterChatStage::WaitingSlot => "due second chat without attempt or observation",
                    ClusterChatStage::Crashed => "unexpected exit with a call in flight",
                    ClusterChatStage::Armed => "w2 linked and connected before the kill",
                    ClusterChatStage::Killed => "w1 tree settled and a new attempt waiting on w2",
                    ClusterChatStage::HealSilence => "heal: w1 program host connected:false after silence",
                    ClusterChatStage::HealRelink => "heal: another w1 linked line and connected host",
                    ClusterChatStage::FenceReady => "fence: w2 connected before stopping w1",
                    ClusterChatStage::FenceReplacement => "fence: different attempt waiting on w2 before continuing w1",
                    ClusterChatStage::FenceSnapshot =>
                        "fence: complete old activity and committed turns before continue",
                    ClusterChatStage::FenceRelink => "fence: another w1 linked line and connected host before chat C",
                    ClusterChatStage::FenceCheck =>
                        "fence: unchanged old activity and turns after chat C waiting on w1",
                    ClusterChatStage::Closed => "closed chats ended",
                    ClusterChatStage::Verifying => "ended done and complete committed turns",
                    ClusterChatStage::StoppedOne => "w1's first life settled",
                    ClusterChatStage::Relinking => "w1's second linked line and connected host",
                    ClusterChatStage::Stopping => "every process tree settled",
                },
                self.last_read
            )
        })
    }
    fn passed(&self) -> bool {
        matches!(self.stage, ClusterChatStage::Stopping) && self.members.passed()
    }
}

#[test]
#[ignore = "backlog: a start without DNS fails while sign-in endpoints resolve at start (director 554)"]
fn a_default_box_with_a_sign_in_registration_starts_without_dns() {
    use std::{ffi::OsString, fs, os::unix::fs::PermissionsExt};
    let scratch = support::Scratch::new("first-run-no-dns");
    let hosts = scratch.path().join("hosts");
    let nsswitch = scratch.path().join("nsswitch.conf");
    fs::write(&hosts, b"").expect("empty names");
    fs::write(&nsswitch, b"hosts: files\npasswd: files\ngroup: files\n").expect("no DNS resolver");
    let paths = ["config", "data", "cache"].map(|name| scratch.path().join(name));
    for path in &paths {
        fs::create_dir(path).expect("empty deployment directories");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private deployment directories");
    }
    let mut init = std::process::Command::new(env!("CARGO_BIN_EXE_temper"));
    init.arg("init");
    for (flag, path) in ["--config", "--data", "--cache"].into_iter().zip(&paths) {
        init.arg(flag).arg(path);
    }
    let initialized = init.output().expect("initialize the default document without resolving names");
    assert!(initialized.status.success(), "{}", String::from_utf8_lossy(&initialized.stderr));
    // Smith also resolves the agent's endpoints at preflight. Supply local endpoint
    // facts there so this regression reaches accounts_runtime; forge defaults stay intact.
    let agent_path = paths[0].join("agent.json");
    let mut agent: serde_json::Value =
        serde_json::from_slice(&fs::read(&agent_path).expect("agent fixture")).expect("agent JSON");
    for endpoint in agent["endpoints"].as_array_mut().expect("endpoints") {
        if endpoint["address"].as_str().expect("endpoint address").parse::<std::net::SocketAddr>().is_err() {
            endpoint["address"] = "127.0.0.1:9".into();
        }
    }
    fs::write(agent_path, serde_json::to_vec(&agent).expect("local agent fixture")).expect("local preflight endpoints");
    // The default registration reaches the sign-in startup resolver.
    let document_path = paths[0].join("temper.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&document_path).expect("default document")).expect("JSON");
    document["application"]["limits"]["overrides"]["git_component.operations"] = 1.into();
    document["application"]["limits"]["overrides"]["checkout.workspaces"] = 1.into();
    fs::write(document_path, serde_json::to_vec(&document).expect("sign-in fixture")).expect("registered sign-in");
    // The private network has only loopback. Files-only name lookup and a private /run
    // exclude DNS and host resolver daemons; the demo peers alone use its sockets.
    let script = r#"set -eu
/usr/bin/mount --make-rprivate /
/usr/bin/mount --bind "$1" /etc/hosts
/usr/bin/mount --bind "$2" /etc/nsswitch.conf
/usr/bin/mount -t tmpfs tmpfs /run
/usr/bin/ip link set lo up
shift 2
exec "$@"
"#;
    let mut arguments: Vec<OsString> = ["-Urnm", "/bin/sh", "-c", script, "no-dns"].map(OsString::from).into();
    arguments.extend([
        hosts.into_os_string(),
        nsswitch.into_os_string(),
        env!("CARGO_BIN_EXE_temper").into(),
        "box".into(),
    ]);
    for (flag, path) in ["--config", "--data", "--cache"].into_iter().zip(&paths) {
        arguments.extend([flag.into(), path.as_os_str().into()]);
    }
    let ended = support::boxed::run_command(
        skein_world::end_to_end::Command {
            program: "/usr/bin/unshare".into(),
            arguments,
            directory: scratch.path().into(),
            environment: Vec::new(),
        },
        None,
        support::boxed::Plan::Stop,
        Duration::from_secs(6),
    );
    let diagnostic = String::from_utf8_lossy(&ended.stderr).into_owned();
    let linked = support::printed_link(&ended.stdout).is_some();
    scratch.remove();
    assert!(!diagnostic.contains("application.forges"), "forge preparation refused: {diagnostic}");
    assert!(!diagnostic.contains("workers[0].agent"), "agent preflight fixture refused: {diagnostic}");
    assert!(
        linked || diagnostic.contains("failed to lookup address information"),
        "unexpected startup refusal: {diagnostic}"
    );
    assert!(
        linked,
        "no-DNS start must reach its printed link; the current sign-in endpoint resolution refuses it: {diagnostic}"
    );
    assert_eq!(ended.exit, Some(Exit::Code(0)), "{diagnostic}");
    assert_eq!(ended.left, Some(Vec::new()), "no process outlived the box: {diagnostic}");
}

// domain/testing.md, 2.6: the person's App, one push and both services restarted.
#[test]
fn a_persons_github_app_connects_pushes_once_and_is_found_after_an_engine_restart() {
    support::github::restart();
}

// Shows the opener and timer cancellation on three processes. Leaves out pools,
// lending, host moves and the other omissions in domain/testing.md, 5.3's 4.1/4.3 rows;
// A and the reviewer leave no placement evidence in an ended document or workspace.
#[test]
fn the_opener_and_the_two_hour_check_run_on_three_workers_as_processes_without_pools_lending_or_host_moves() {
    use temper_real_loop_world::case::{self, acts, oracle};
    let run = support::case::run(
        &["ws", "s1", "s2"],
        case::fixture_check(),
        case::scripts::step_4_1_4_3,
        case::scripts::step_4_1_4_3_rules,
        acts::step_4_1_4_3(),
        &[case::DIRECTOR, case::CHECK, case::B, case::BENCHMARK],
    );
    for (at, call) in run.calls().iter().enumerate() {
        assert_eq!(
            call.exit,
            Some(skein_io::kernel::Exit::Code(0)),
            "call {at}: {}",
            String::from_utf8_lossy(&call.stderr)
        );
    }
    let observed = run.observed(&acts::reads_4_1_4_3());
    let mut misses = oracle::step_4_1(&observed);
    misses.extend(oracle::step_4_3(&observed));
    oracle::assert_met(&misses);
    assert_eq!(
        observed.documents[&acts::Read::BenchmarkWaiting]
            .pointer("/task/chip/phase")
            .and_then(serde_json::Value::as_str),
        Some("due")
    );
    for (task, worker) in [(case::DIRECTOR, "ws"), (case::CHECK, "ws"), (case::B, "s1"), (case::BENCHMARK, "s2")] {
        assert_eq!(run.placed().get(&task).map(String::as_str), Some(worker), "task {task}");
    }
}

// Leaves out the director as asker, plan pacing, B's model, repeated questions,
// and amending only A's model (domain/testing.md, 5.3's 4.4 row).
// On ws alone, no s2: a worker's own sources and sign-in are undesigned.
// Past 6.1's 2 s guide, as its real-loop twin is, by its seven activations.
#[test]
fn the_pace_question_reaches_the_person_and_its_answer_is_kept_as_processes_on_one_worker_without_plan_pacing() {
    use temper_real_loop_world::case::{self, acts, oracle};
    let run = support::case::run(
        &["ws"],
        case::fixture_4_4(),
        case::scripts::step_4_4,
        step_4_4_rules,
        acts::step_4_4(),
        &[],
    );
    oracle::assert_met(&oracle::step_4_4(&run.observed(acts::READS_4_4)));
}
fn step_4_4_rules() -> Box<[Box<[skein_fake_llm_domain::api::Rule]>]> {
    Box::new([Box::new([]), Box::new([]), Box::new([]), Box::new([])])
}

// Leaves out s2's sign-in, credentials never copied between machines, B's work,
// five hours and a session that never ends (domain/testing.md, 5.3's 4.5 row).
// On ws alone, no s2: a worker's own sources and sign-in are undesigned.
#[test]
fn a_refused_key_holds_its_work_until_it_is_replaced_as_processes_on_one_worker_without_a_sign_in() {
    use temper_real_loop_world::case::{self, acts, oracle};
    let run = support::case::run(
        &["ws"],
        case::fixture(),
        case::scripts::step_4_5_refused,
        step_4_5_refused_rules,
        acts::step_4_5_refused(),
        &[],
    );
    oracle::assert_met(&oracle::step_4_5_refused(&run.observed(acts::READS_4_5_REFUSED)));
}
fn step_4_5_refused_rules() -> Box<[Box<[skein_fake_llm_domain::api::Rule]>]> {
    Box::new([Box::new([]), Box::new([])])
}

// Leaves out s2's sign-in, credentials never copied between machines, B's work,
// five hours and a session that never ends (domain/testing.md, 5.3's 4.5 row).
// On ws alone, no s2: a worker's own sources and sign-in are undesigned.
// Past 6.1's 2 s guide, as its real-loop twin is, by its reset: one second,
// the shortest a provider's wire carries.
#[test]
fn a_limited_account_holds_its_work_until_its_reset_as_processes_on_one_worker() {
    use temper_real_loop_world::case::{self, acts, oracle};
    let run = support::case::run(
        &["ws"],
        case::fixture(),
        case::scripts::step_4_5_limited,
        step_4_5_limited_rules,
        acts::step_4_5_limited(),
        &[],
    );
    oracle::assert_met(&oracle::step_4_5_limited(&run.observed(acts::READS_4_5_LIMITED)));
}
fn step_4_5_limited_rules() -> Box<[Box<[skein_fake_llm_domain::api::Rule]>]> {
    Box::new([Box::new([]), Box::new([])])
}

// Shows the retained question and one answer across ws's two lives. Leaves out
// the hang past grace, three questions and the other limits of testing.md, 5.3's 4.6 row.
// The retry runs on s1 or s2 by that row's act order: ws stays ended and unlinked
// until after the ended chat's watch joins; the oracle checks both attempts.
#[test]
#[ignore = "waits on jig's fleet: a retry placed on a stopping worker's still-linked channel is rechecked and journaled as an extra refused attempt (3 in 85 runs since the waiting read, all under load); jig is to take a worker out of placement as it drains, before its runs end"]
fn a_restarted_workstation_keeps_the_directors_question_and_answers_it_once_as_processes_without_a_hang_past_the_grace()
{
    use temper_real_loop_world::case::{self, acts, oracle};
    let run = support::case::run(
        &["ws", "s1", "s2"],
        case::fixture_4_6(),
        case::scripts::step_4_6,
        case::scripts::step_4_6_rules,
        acts::step_4_6(0),
        &[case::HANG],
    );
    for (at, call) in run.calls().iter().enumerate() {
        assert_eq!(
            call.exit,
            Some(skein_io::kernel::Exit::Code(0)),
            "call {at}: {}",
            String::from_utf8_lossy(&call.stderr)
        );
    }
    oracle::assert_met(&oracle::step_4_6(&run.observed(acts::READS_4_6)));
    assert_eq!(run.first_placed().get(&case::HANG).map(String::as_str), Some("ws"));
}

// domain/testing.md, 2.6: the chat's connected GitHub change survives both services restarting.
#[test]
fn a_chats_change_lands_on_connected_github_and_is_found_after_a_restart() {
    support::github_land::restart();
}

#[test]
fn an_owned_forgejo_signs_its_member_in_through_temper() {
    let scratch = support::Scratch::new("forgejo_sign_in");
    support::forgejo_sign_in::run(scratch.path());
    scratch.remove();
}

#[test]
fn a_forgejo_elsewhere_connected_as_its_admin_makes_the_first_repository_lands_a_chats_change_and_is_found_after_a_restart()
 {
    support::forgejo_elsewhere::restart();
}

#[cfg(target_os = "linux")]
#[test]
fn a_story_box_and_its_steps_die_when_the_test_process_is_killed() {
    use std::{
        fs,
        process::{Command, Stdio},
    };
    use support::parent::{Guard, descendants, live, wait};
    const CHILD: &str = "TEMPER_BOX_PARENT_TEST";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::Path::new(&root);
        let command = support::boxed::dying_with_test(skein_world::end_to_end::Command {
            program: env!("CARGO_BIN_EXE_temper").into(),
            arguments: std::iter::once("box".into()).chain(support::configuration::flags(root)).collect(),
            directory: root.into(),
            environment: Vec::new(),
        });
        // The same test thread spawns and waits: PR_SET_PDEATHSIG follows it.
        let mut box_process = Command::new(command.program)
            .args(command.arguments)
            .current_dir(command.directory)
            .env_clear()
            .envs(command.environment)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("wrapped box");
        fs::write(root.join("box.pid"), box_process.id().to_string()).expect("box pid");
        let _ended = box_process.wait().expect("box wait");
        return;
    }
    let scratch = support::Scratch::new("box-dies-with-test");
    // Init before box.pid exists: a first run requires an empty configuration directory.
    let output = Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("init")
        .args(support::configuration::flags(scratch.path()))
        .output()
        .expect("init");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let child = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "a_story_box_and_its_steps_die_when_the_test_process_is_killed", "--nocapture"])
        .env(CHILD, scratch.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("child test");
    let mut guard = Guard::new(child);
    let pid_file = scratch.path().join("box.pid");
    wait(|| pid_file.exists());
    let box_pid = fs::read_to_string(pid_file).expect("box pid").parse().expect("pid number");
    guard.pids.push(box_pid);
    wait(|| {
        let mut steps = Vec::new();
        descendants(box_pid, &mut steps);
        if steps.is_empty() {
            return false;
        }
        guard.pids.extend(steps);
        true
    });
    guard.child.as_mut().expect("child test").kill().expect("SIGKILL test alone");
    guard.child.as_mut().expect("child test").wait().expect("reap test");
    wait(|| guard.pids.iter().all(|pid| !live(*pid)));
    guard.pids.clear();
    scratch.remove();
}
