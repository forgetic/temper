//! Design-only process QA: protocol/setup.md 6.4–7.3, 8.4 and 13.4;
//! draft/first-run.md 2.3 and step 4; client/forge.md 5.4.
#[expect(
    dead_code,
    clippy::unused_self,
    clippy::struct_field_names,
    reason = "Unchanged shared process harness; this binary exercises the first run only."
)]
mod support;

use serde_json::Value;
use skein_io::kernel::Exit;
use skein_lib::Duration;
use std::{fs, net::SocketAddr, path::Path};
use support::{
    Scratch, boxed, ctl,
    ctl::{Call, Step},
    forge_land, parent,
};

fn paths(root: &Path) -> [std::path::PathBuf; 3] {
    [root.join("config"), root.join("data"), root.join("cache")]
}

fn forge_address(config: &Path) -> SocketAddr {
    let document: Value =
        serde_json::from_slice(&fs::read(config.join("temper.json")).expect("init document")).expect("document JSON");
    let forge = document["application"]["forges"]
        .as_array()
        .expect("configured forges")
        .iter()
        .find(|forge| forge["number"] == 3)
        .expect("owned forge 3");
    forge["api"]["socket"].as_str().expect("forge socket").parse().expect("socket address")
}

fn row(call: &support::Ended, number: u64) -> Value {
    ctl::json(call)["forges"]
        .as_array()
        .expect("forge rows")
        .iter()
        .find(|row| row["number"] == number)
        .cloned()
        .expect("configured forge row")
}

fn drained<T>(outcome: &boxed::CtlOutcome<T>, expected: &[(usize, u8)], count: usize) {
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    eprintln!("box stderr:\n{stderr}");
    for (index, call) in outcome.calls.iter().enumerate() {
        eprintln!(
            "ctl {index}: {:?}\n{}\n{}",
            call.exit,
            String::from_utf8_lossy(&call.stdout),
            String::from_utf8_lossy(&call.stderr)
        );
        let code = expected.iter().find(|(at, _)| *at == index).map_or(0, |(_, code)| *code);
        assert_eq!(call.exit, Some(Exit::Code(code)), "ctl {index}; box: {stderr}");
    }
    assert_eq!(outcome.calls.len(), count, "all story calls completed; box: {stderr}");
    assert!(outcome.checked.is_some(), "live checkpoint reached; box: {stderr}");
    assert_eq!(outcome.exit, Some(Exit::Code(0)), "healthy box stop; box: {stderr}");
    assert!(outcome.left.as_ref().is_some_and(Vec::is_empty), "no process at box exit: {:?}; {stderr}", outcome.left);
}

fn descendants_with(arguments: &[&str]) -> Vec<u32> {
    let mut pids = Vec::new();
    // Rust's test body runs on a worker thread; its children belong to that
    // thread, rather than the test process's leader.
    for pid in fs::read_to_string("/proc/thread-self/children")
        .expect("test thread's children")
        .split_whitespace()
        .map(|pid| pid.parse().expect("child pid"))
    {
        pids.push(pid);
        parent::descendants(pid, &mut pids);
    }
    pids.into_iter()
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|bytes| {
                let words: Vec<_> = bytes.split(|byte| *byte == 0).collect();
                words
                    .windows(arguments.len())
                    .any(|window| window.iter().zip(arguments).all(|(actual, expected)| *actual == expected.as_bytes()))
            })
        })
        .collect()
}

#[test]
fn qa_dormant_owned_forge_holds_its_port_before_any_setup() {
    let scratch = Scratch::new("qa-first-run-dormant");
    let dirs = paths(scratch.path());
    let outcome = boxed::run_ctl(
        scratch.path(),
        forge_land::deployment_flags(&dirs),
        scratch.path(),
        vec![Step::Run(Call::sign_in()), Step::Run(Call::new(&["forges", "--json"]))],
        Duration::from_secs(30),
        || {
            let address = forge_address(&dirs[0]);
            let reservation = std::net::TcpListener::bind(address).err().map(|error| error.kind());
            (address, reservation, descendants_with(&["forge", "run"]))
        },
    );
    drained(&outcome, &[], 2);
    let (address, reservation, running) = outcome.checked.expect("dormant checkpoint");
    assert_eq!(reservation, Some(std::io::ErrorKind::AddrInUse), "box holds dormant forge's {address}");
    assert!(running.is_empty(), "no owned program before asked: {running:?}");
    let owned = row(&outcome.calls[1], 3);
    assert_eq!(owned["state"], "not_set_up");
    assert_eq!(owned["offer"], "offered");
    assert!(!dirs[1].join("box/started/3").exists());
    assert!(!dirs[1].join("forge-3").exists());
    assert!(!String::from_utf8_lossy(&outcome.stderr).contains("Playground: starting"));
    let released = std::net::TcpListener::bind(address).expect("box releases dormant port at stop");
    drop(released);
    scratch.remove();
}

#[test]
fn qa_running_playground_ends_whole_when_owned_forge_connects_without_restart() {
    let scratch = Scratch::new("qa-first-run-going-real");
    let dirs = paths(scratch.path());
    let (cached, fingerprints) = forge_land::cache_forgejo_binaries(&dirs[2]);
    let steps = vec![
        Step::Run(Call::sign_in()),
        Step::Run(Call::new(&["demo-start"])),
        Step::Run(Call::new(&["chat", "--project", "1", "--key", "playground-chat", "Add CSV export to widget"])),
        Step::Run(Call::new(&["wait", "task", "1", "--until", "replied", "--timeout", "20s"])),
        Step::Run(Call::new(&["wait", "task", "2", "--until", "ended", "--timeout", "60s"])),
        Step::Until(Call::new(&["chat", "1"]), b"CSV export is available"),
        Step::Run(Call::new(&["forges", "--json"])),
        Step::Run(Call::new(&["forge-setup", "3"])),
        Step::Run(Call::new(&["wait", "forges", "--until", "settled", "--timeout", "180s"])),
        Step::Run(Call::new(&["forges", "--json"])),
        Step::Run(Call::new(&["accounts", "--json"])),
        Step::Run(Call::new(&["wait", "task", "1", "--until", "ended", "--timeout", "20s"])),
        Step::Run(Call::new(&["task", "1"])),
        Step::Run(Call::new(&["demo-start"])),
        Step::Run(Call::new(&["forges"])),
        Step::Run(Call::new(&["task", "2"])),
    ];
    let expected = [(13, 2)];
    let outcome = boxed::run_ctl_with_exits(
        scratch.path(),
        forge_land::deployment_flags(&dirs),
        scratch.path(),
        steps,
        Duration::from_secs(210),
        || {
            support::playground::retired_reload(&dirs[0], scratch.path());
            let turns = support::first_repository::turns(scratch.path(), "1");
            let address = forge_address(&dirs[0]);
            (turns, boxed::status(address, "/api/v1/version"))
        },
        &expected,
    );
    drained(&outcome, &expected, 16);
    assert_eq!(row(&outcome.calls[6], 1)["state"], "connected");
    let final_forges = ctl::json(&outcome.calls[9]);
    assert!(final_forges["forges"].as_array().expect("rows").iter().all(|row| row["number"] != 1));
    let owned = row(&outcome.calls[9], 3);
    assert_eq!(owned["state"], "connected");
    assert_eq!(owned["mode"], "owned");
    assert_eq!(owned["first"]["state"], "made");
    assert_eq!(owned["first"]["org"], "work");
    assert_eq!(owned["first"]["repository"], "hello");
    assert_eq!(owned["first"]["project"]["new"], "hello");
    let adoption = String::from_utf8_lossy(&outcome.calls[7].stdout);
    assert!(adoption.contains("Adopted work/hello on forge 3 as owned."), "{adoption}");
    let task = String::from_utf8_lossy(&outcome.calls[12].stdout);
    assert!(task.contains("Ended with the playground") && task.to_lowercase().contains("cancelled"), "{task}");
    assert!(String::from_utf8_lossy(&outcome.calls[13].stderr).contains("not offered"));
    let accounts = ctl::json(&outcome.calls[10]);
    assert_eq!(accounts["accounts"]["deployment"], serde_json::json!([]), "demo account removed: {accounts}");
    let (turns, status) = outcome.checked.expect("live checkpoint");
    assert!(turns.contains("CSV export is available"), "the demo's committed reply remains readable: {turns}");
    let before = String::from_utf8_lossy(&outcome.calls[4].stdout);
    let after = String::from_utf8_lossy(&outcome.calls[15].stdout);
    let landed = before.lines().find(|line| line.contains("Landed · merge commit "));
    assert!(landed.is_some(), "the demo's change landed before going real: {before}");
    assert_eq!(after.lines().find(|line| line.contains("Landed · merge commit ")), landed);
    assert_eq!(status, Some(200));
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert_eq!(
        String::from_utf8_lossy(&outcome.stdout).lines().filter(|line| line.starts_with("http://")).count(),
        1,
        "one engine's sign-in link across the whole runtime transition"
    );
    assert!(String::from_utf8_lossy(&outcome.calls[14].stdout).contains("Connected as temper"));
    assert!(!stderr.contains("restarting"), "no restart while going real: {stderr}");
    assert!(dirs[1].join("box/started/3").is_file());
    for name in ["admin_token", "api_token", "git_token", "user", "webhook_secret"] {
        assert!(!dirs[1].join("forge-3").join(name).exists(), "no legacy/handover {name}");
    }
    for (path, before) in cached.iter().zip(fingerprints) {
        assert_eq!(forge_land::cached_binary_fingerprint(path), before, "cached release untouched");
    }
    // setup.md 7.2's `Playground: stopped` is the labelled terminal's, not built:
    // qa_an_asked_groups_failure_is_named_by_its_label keeps that gap.
    assert!(stderr.contains("box: forge 1's group stopped"), "box reports the demo group's stop: {stderr}");
    scratch.remove();
}

#[test]
fn qa_bad_cached_release_fails_setup_but_keeps_the_playground_working() {
    bad_cached_release();
}

#[test]
#[ignore = "backlog: the box's terminal names its groups by label (setup.md 7.2, 7.3: `Forge 3 (Forgejo): failed, WHY`, `Playground: stopped`), not built: the quiet terminal's, finding F1's work (STATUS, protocol/setup.md's row)"]
fn qa_an_asked_groups_failure_is_named_by_its_label() {
    let stderr = bad_cached_release();
    assert!(stderr.contains("Forge 3 (Forgejo): failed, "), "{stderr}");
}

/// Returns the box's stderr once the failed set up's other promises hold.
fn bad_cached_release() -> String {
    let scratch = Scratch::new("qa-first-run-bad-cache");
    let dirs = paths(scratch.path());
    let (cached, _) = forge_land::cache_forgejo_binaries(&dirs[2]);
    fs::write(&cached[0], b"invalid cached release\n").expect("corrupt this scratch copy only");
    let expected = [(2, 1)];
    let outcome = boxed::run_ctl_with_exits(
        scratch.path(),
        forge_land::deployment_flags(&dirs),
        scratch.path(),
        vec![
            Step::Run(Call::sign_in()),
            Step::Run(Call::new(&["demo-start"])),
            Step::Run(Call::new(&["forge-setup", "3"])),
            Step::Run(Call::new(&["forges", "--json"])),
            Step::Run(Call::new(&["demo-start"])),
            Step::Run(Call::new(&[
                "chat",
                "--project",
                "1",
                "--key",
                "after-failed-setup",
                "Add a dark theme to widget",
            ])),
            Step::Run(Call::new(&["wait", "task", "1", "--until", "replied", "--timeout", "20s"])),
        ],
        Duration::from_secs(60),
        || {
            let address = forge_address(&dirs[0]);
            (
                std::net::TcpListener::bind(address).err().map(|error| error.kind()),
                support::first_repository::turns(scratch.path(), "1"),
            )
        },
        &expected,
    );
    drained(&outcome, &expected, 7);
    let owned = row(&outcome.calls[3], 3);
    assert_eq!(owned["state"], "set_up_failed");
    assert_eq!(owned["offer"], "offered");
    assert_eq!(row(&outcome.calls[3], 1)["state"], "connected");
    let (reservation, turns) = outcome.checked.expect("failed setup checkpoint");
    assert_eq!(reservation, Some(std::io::ErrorKind::AddrInUse));
    assert!(turns.contains("theme"), "demo still answers: {turns}");
    assert!(!dirs[1].join("box/started/3").exists());
    assert_eq!(fs::read(&cached[0]).expect("retained corrupt binary"), b"invalid cached release\n");
    let stderr = String::from_utf8_lossy(&outcome.stderr).into_owned();
    assert!(!stderr.contains("box: forge 1's group stopped"), "the playground runs on: {stderr}");
    scratch.remove();
    stderr
}

#[cfg(target_os = "linux")]
fn parent_shutdown(signal: &str) {
    let scratch = Scratch::new(&format!("qa-first-run-parent-{signal}"));
    let dirs = paths(scratch.path());
    let _cached = forge_land::cache_forgejo_binaries(&dirs[2]);
    let mut steps = forge_land::forgejo_setup_steps("first");
    steps.push(Step::Run(Call::new(&["demo-start"])));
    let outcome = boxed::run_ctl_with_exits(
        scratch.path(),
        forge_land::deployment_flags(&dirs),
        scratch.path(),
        steps,
        Duration::from_secs(210),
        || {
            let runners = descendants_with(&["forge", "run"]);
            let forge_parent = runners
                .into_iter()
                .find(|pid| fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|bytes| bytes.ends_with(b"forgejo\0")));
            let mut children = Vec::new();
            if let Some(pid) = forge_parent {
                parent::descendants(pid, &mut children);
            }
            let child = children.iter().copied().find(|pid| {
                fs::read_link(format!("/proc/{pid}/exe"))
                    .is_ok_and(|path| path.file_name().is_some_and(|name| name == "forgejo"))
            });
            let healthy = boxed::status(forge_address(&dirs[0]), "/api/v1/version");
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(dirs[1].join("forge-3/forgejo.lock"))
                .expect("Forgejo lock file");
            let locked = lock.try_lock().is_err();
            let stopped = match (forge_parent, child) {
                (Some(pid), Some(child)) => {
                    parent::signal(pid, signal);
                    parent::wait(|| !parent::live(child));
                    parent::wait(|| !parent::live(pid));
                    true
                }
                _ => false,
            };
            (forge_parent, child, healthy, locked, stopped)
        },
        &[(7, 2)],
    );
    drained(&outcome, &[(7, 2)], 8);
    let (forge_parent, child, healthy, locked, stopped) = outcome.checked.expect("parent checkpoint");
    assert!(forge_parent.is_some() && child.is_some(), "forge run stays Forgejo's parent: {forge_parent:?}, {child:?}");
    assert_eq!(healthy, Some(200), "Forgejo's startup SIGTERM did not stop its parent or box");
    assert!(locked, "forge run retains its lock");
    assert!(stopped, "{signal} to forge run stops Forgejo too");
    assert!(String::from_utf8_lossy(&outcome.calls[7].stderr).contains("not offered"));
    assert!(!String::from_utf8_lossy(&outcome.stderr).contains("Playground: starting"));
    scratch.remove();
}

#[cfg(target_os = "linux")]
#[test]
fn qa_forge_parent_forwards_external_terminate() {
    parent_shutdown("TERM");
}

#[cfg(target_os = "linux")]
#[test]
fn qa_forge_parent_forwards_external_interrupt() {
    parent_shutdown("INT");
}

#[cfg(target_os = "linux")]
#[test]
fn qa_forgejo_dies_when_its_forge_run_parent_is_killed() {
    parent_shutdown("KILL");
}
