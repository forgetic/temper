//! A second deployment connects to the owned fixture solely as a Forgejo elsewhere.
use super::{Forge, Process, Scratch, ctl, forgejo::Throwaway, write_private};
use serde_json::{Value, json};
use skein_fake_llm_domain::api::{Finish, Line, Script, Turn};
use skein_io::kernel::{Exit, Signal};
use skein_lib::{Duration, Time};
use skein_shell::{Clock, Host};
use skein_world::{
    Referee,
    end_to_end::{Binary, Command, Mode},
    real,
};
use std::{
    fs,
    io::{Read, Write},
    os::{
        linux::net::SocketAddrExt,
        unix::{
            ffi::OsStrExt,
            fs::PermissionsExt,
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
};

const STORY: &str = "a_forgejo_elsewhere_connected_as_its_admin_makes_the_first_repository_lands_a_chats_change_and_is_found_after_a_restart";
const CHILD: &str = "TEMPER_E4_LIFE";
const EDIT: &[u8] = b"Changed by the elsewhere chat.\n";

pub fn restart() {
    if let Some(root) = std::env::var_os(CHILD) {
        life(Path::new(&root), std::env::var_os("TEMPER_E4_RESTART").is_some());
        return;
    }
    let Some(capture) = std::env::var_os("TEMPER_E4_CAPTURE") else {
        let scratch = Scratch::new("forgejo_elsewhere_capture");
        let name = format!("temper-e4-{}", std::process::id());
        let address = SocketAddr::from_abstract_name(name.as_bytes()).expect("short capture address");
        let listener = UnixListener::bind_addr(&address).expect("capture socket");
        let child = std::process::Command::new(std::env::current_exe().expect("process test binary"))
            .args(["--exact", STORY, "--nocapture"])
            .env("TEMPER_E4_CAPTURE", &name)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("captured complete story");
        // The child opens this before setup: even setup failure closes the stream.
        let (mut stream, _) = listener.accept().expect("capture child");
        let mut token = Vec::new();
        stream.read_to_end(&mut token).expect("private scan witness");
        let output = child.wait_with_output().expect("complete story output");
        if token.is_empty() {
            assert!(output.status.success(), "story failed before minting a credential (output redacted)");
        } else {
            no_token(&output.stdout, &token);
            no_token(&output.stderr, &token);
            std::io::stdout().write_all(&output.stdout).expect("scanned support stdout");
            std::io::stderr().write_all(&output.stderr).expect("scanned support stderr");
            assert!(output.status.success(), "complete elsewhere story failed");
        }
        scratch.remove();
        return;
    };
    let address = SocketAddr::from_abstract_name(capture.as_bytes()).expect("capture address");
    let mut capture = UnixStream::connect_addr(&address).expect("private capture witness");
    let clock = Clock::new();
    let began = clock.now().now;
    let elsewhere = Scratch::new("forgejo_elsewhere_host");
    let forge = Throwaway::start(&elsewhere);
    let scratch = Scratch::new("forgejo_elsewhere");
    for name in ["config", "data", "cache"] {
        fs::create_dir(scratch.path().join(name)).expect("fresh root");
        fs::set_permissions(scratch.path().join(name), fs::Permissions::from_mode(0o700)).expect("private root");
    }
    configure(scratch.path(), &forge.forge());
    let paste = scratch.path().join("admin-paste");
    forge.admin_token(&paste);
    let token = fs::read(&paste).expect("private disposable token");
    capture.write_all(&token).expect("private capture witness");
    drop(capture);
    assert_eq!(fs::metadata(&paste).expect("paste metadata").permissions().mode() & 0o777, 0o600);
    // Re-exec captures the support's logs and stage timings too. Neither the
    // child arguments nor its environment contains credential bytes.
    child(scratch.path(), false, &token);
    inspect(scratch.path(), &token);
    forge.witness_authenticates();
    let pull = forge.merged_pull_in("elsewhere", "first").expect("merged elsewhere pull");
    assert_eq!(forge.job_state_in("elsewhere", "first", pull), "success");
    assert!(
        forge.main_readme_in("elsewhere", "first").windows(EDIT.len()).any(|part| part == EDIT),
        "main holds the chat's edit"
    );
    let commits = forge.repository_observation("elsewhere", "first", "commits?sha=main&limit=50");
    assert_eq!(forge.repositories_in("elsewhere").as_array().expect("repositories").len(), 1);
    assert_eq!(
        forge.repository_observation("elsewhere", "first", "pulls?state=all&limit=50").as_array().expect("pulls").len(),
        1
    );
    fs::remove_file(&paste).expect("delete paste before restart");
    child(scratch.path(), true, &token);
    inspect(scratch.path(), &token);
    forge.witness_authenticates();
    assert!(!paste.exists(), "restart has no paste");
    assert_eq!(forge.merged_pull_in("elsewhere", "first"), Some(pull));
    assert_eq!(forge.repositories_in("elsewhere").as_array().expect("repositories").len(), 1);
    assert_eq!(
        forge.repository_observation("elsewhere", "first", "commits?sha=main&limit=50"),
        commits,
        "restart creates no workflow commit"
    );
    assert_eq!(
        forge.repository_observation("elsewhere", "first", "pulls?state=all&limit=50").as_array().expect("pulls").len(),
        1
    );
    forge.stop();
    scratch.remove();
    elsewhere.remove();
    eprintln!("elsewhere story: {:?}", clock.now().now.saturating_since(began));
}

fn child(root: &Path, restarted: bool, token: &[u8]) {
    let mut command = std::process::Command::new(std::env::current_exe().expect("process test binary"));
    command.args(["--exact", STORY, "--nocapture"]).env(CHILD, root);
    if restarted {
        command.env("TEMPER_E4_RESTART", "1");
    }
    let output = command.output().expect("captured story life");
    no_token(&output.stdout, token);
    no_token(&output.stderr, token);
    // Only release diagnostics after the redacted byte scan.
    std::io::stdout().write_all(&output.stdout).expect("support stdout");
    std::io::stderr().write_all(&output.stderr).expect("support stderr");
    assert!(output.status.success(), "elsewhere engine life failed");
}

fn configure(root: &Path, forge: &Forge<'_>) {
    let config = root.join("config");
    super::configuration(&config, forge);
    let Forge::Throwaway { api, git, .. } = forge else { panic!("elsewhere fixture") };
    let path = config.join("temper.json");
    let bytes = fs::read_to_string(&path).expect("configuration").replace("\"acme\"", "\"elsewhere\"");
    let mut value: Value = serde_json::from_str(&bytes).expect("JSON");
    value["engine"]["deployment"]["directory"] = json!(root.join("data"));
    let digest = value["engine"]["hosts"]["workers"][0]["digest"].as_str().expect("worker digest");
    write_private(&root.join("data/worker.digest"), &fs::read(config.join(digest)).expect("fixture digest"));
    value["engine"]["hosts"]["workers"][0]["digest"] = "worker.digest".into();
    fs::rename(config.join("worker.secret"), root.join("data/worker.secret")).expect("data worker secret");
    fs::remove_dir_all(config.join("secrets")).expect("remove obsolete file credentials");
    let entry = value["application"]["forges"][0].as_object_mut().expect("forge");
    for member in ["api_token", "user", "webhook_secret", "identity"] {
        entry.remove(member);
    }
    entry.insert("api".into(), json!({"socket":api.to_string()}));
    entry.insert("git".into(), git.clone().into());
    let identity = value["application"]["identities"][0].as_object_mut().expect("identity");
    identity.remove("token");
    identity.insert("forge".into(), 1.into());
    identity.insert("login".into(), "elsewhere-bot".into());
    value["application"]["setup"] =
        json!([{"forge":1,"bot":{"login":"elsewhere-bot","email":"elsewhere-bot@example.invalid"},"mode":"managed"}]);
    write_private(&path, &serde_json::to_vec(&value).expect("managed configuration"));
}

fn scripts(repository: &str, readme: &[u8]) -> Box<[Script]> {
    let call = |name: &[u8], arguments: Value| Turn {
        lines: Box::new([Line::Call {
            name: name.into(),
            arguments: serde_json::to_vec(&arguments).expect("tool JSON").into(),
        }]),
        finish: Finish::ToolCalls,
        tokens: 20,
    };
    let coding = format!("@elsewhere-coder Edit {repository} README.md");
    let original = std::str::from_utf8(readme).expect("README words");
    let edited = format!("{original}\n\n{}", std::str::from_utf8(EDIT).expect("edit words"));
    Box::new([
        Script {
            cue: b"@elsewhere-chat".as_slice().into(),
            turns: Box::new([
                call(b"change", json!({"words":coding,"repository":repository})),
                call(b"wait", json!({})),
                Turn {
                    lines: Box::new([Line::Text { text: b"The change landed.".as_slice().into() }]),
                    finish: Finish::Stop,
                    tokens: 20,
                },
            ]),
        },
        Script {
            cue: coding.into_bytes().into(),
            turns: Box::new([
                call(b"read", json!({"path":"README.md"})),
                call(b"read", json!({"path":".forgejo/workflows/ci.yml"})),
                call(b"edit", json!({"path":"README.md","edits":[{"old":original,"new":edited}]})),
                call(
                    b"finish",
                    json!({"form":"change","fields":{"title":"Update elsewhere README","body":"Changed by the elsewhere chat."}}),
                ),
            ]),
        },
    ])
}

fn life(root: &Path, restarted: bool) {
    let clock = Clock::new();
    let began = clock.now().now;
    let config = root.join("config");
    let peer = (!restarted).then(|| super::fake_llm(scripts("elsewhere/first", b"# first")));
    if let Some(peer) = &peer {
        peer.configure(&config);
    }
    let command = Command {
        program: env!("CARGO_BIN_EXE_temper").into(),
        arguments: vec![
            "serve".into(),
            "--config".into(),
            config.into(),
            "--data".into(),
            root.join("data").into(),
            "--cache".into(),
            root.join("cache").into(),
        ],
        directory: root.into(),
        environment: Vec::new(),
    };
    let mut engine = Binary::start(command, Mode::Pipes, 65_536).expect("elsewhere engine");
    let streams = engine.take_streams().expect("engine pipes");
    let fd = skein_shell::open_root(root).expect("story root");
    let mut fds = streams.descriptors();
    fds.push(fd);
    let mut steps = vec![ctl::Step::Run(ctl::Call::sign_in())];
    if !restarted {
        steps.push(ctl::Step::Run(ctl::Call::new(&[
            "forge-connect",
            "1",
            "forgejo-admin",
            root.join("admin-paste").to_str().expect("paste path"),
            "--first",
            "elsewhere/first",
            "--label",
            "temper-host",
            "--key",
            "elsewhere-connect",
            "--json",
        ])));
        steps.push(ctl::Step::Run(ctl::Call::new(&["wait", "forges", "--until", "made:1", "--timeout", "60s"])));
    }
    steps.push(ctl::Step::Run(ctl::Call::new(&["wait", "forges", "--until", "connected:1", "--timeout", "30s"])));
    steps.push(ctl::Step::Run(ctl::Call::new(&["forges", "--json"])));
    let mut world = real::World::new(Judge {
        root: root.into(),
        restarted,
        stage: 0,
        stopped: false,
        settled: false,
        logged: 0,
        deadline: clock.now().now.saturating_add(Duration::from_secs(240)),
    });
    world.spawn_with_fds(engine.descriptors(), || Process::Binary(Box::new(engine)));
    world.spawn_with_fds(fds, || {
        let mut calls = ctl::Calls::new(fd, steps, Some(&streams), None);
        calls.hold();
        Process::Calls(Box::new(calls))
    });
    if let Some(peer) = peer {
        peer.spawn(&mut world);
    }
    let ended = world.run(&clock, Duration::from_secs(250));
    assert!(ended.procs.iter().all(Host::is_empty), "engine, ctl and fake drained");
    let Process::Binary(engine) = &ended.procs[0] else { panic!("engine") };
    let Process::Calls(calls) = &ended.procs[1] else { panic!("ctl") };
    // The parent captures these raw bytes before releasing any diagnostic.
    let mut output = std::io::stdout();
    output.write_all(engine.stderr()).expect("engine log");
    output.write_all(calls.stdout()).expect("engine stdout");
    for call in calls.ends() {
        output.write_all(&call.stdout).expect("ctl stdout");
        output.write_all(&call.stderr).expect("ctl stderr");
        assert_eq!(call.exit, Some(Exit::Code(0)), "ctl call failed");
    }
    assert_eq!(engine.exit_status(), Some(Exit::Code(0)), "engine stopped");
    let task = calls.ends().last().expect("landed task read");
    assert!(
        String::from_utf8_lossy(&task.stdout).contains("Landed · merge commit "),
        "same landing is visible through ctl"
    );
    eprintln!("elsewhere life restarted={restarted}: {:?}", clock.now().now.saturating_since(began));
}

struct Judge {
    root: PathBuf,
    restarted: bool,
    stage: u8,
    stopped: bool,
    settled: bool,
    logged: usize,
    deadline: Time,
}
impl Referee<Process> for Judge {
    fn act(&mut self, now: Time, procs: &mut [Process]) {
        let Process::Binary(engine) = &procs[0] else { panic!("engine") };
        if engine.exit_status().is_some() && !self.stopped || now >= self.deadline {
            eprintln!("engine stderr: {}", String::from_utf8_lossy(engine.stderr()));
        }
        assert!(self.stopped || engine.exit_status().is_none(), "engine ended before ctl completed");
        let Process::Calls(calls) = &mut procs[1] else { panic!("ctl") };
        while self.logged < calls.ends().len() {
            let call = &calls.ends()[self.logged];
            eprintln!(
                "ctl {} exit {:?}: {} {}",
                self.logged,
                call.exit,
                String::from_utf8_lossy(&call.stdout),
                String::from_utf8_lossy(&call.stderr)
            );
            self.logged += 1;
        }
        if !calls.foreground_idle() {
            return;
        }
        if self.stage == 0 {
            let ends = calls.ends();
            let forges = ctl::json(ends.last().expect("forges read"));
            let row = &forges["forges"][0];
            assert_eq!(row["state"], "connected");
            assert_eq!(row["mode"], "managed");
            if self.restarted {
                assert!(row["first"].is_null(), "restart does not repeat Create");
                calls.push(ctl::Step::Run(ctl::Call::new(&["task", "2"])));
            } else {
                assert_eq!(row["first"]["state"], "made");
                // Progress and the terminal answer are separate JSON lines, as for setup.
                let answer = ends[1]
                    .stdout
                    .split(|byte| *byte == b'\n')
                    .rfind(|line| !line.is_empty())
                    .expect("adoption answer");
                let connected: Value = serde_json::from_slice(answer).expect("adoption JSON");
                let project = connected["project"].as_u64().expect("adoption's project number");
                assert_eq!(connected["outcome"]["adopted"]["owner"], "elsewhere");
                assert_eq!(connected["outcome"]["adopted"]["name"], "first");
                let key = self.root.join("llm-key");
                write_private(&key, jig_channel_world::fixtures::KEY);
                for call in [
                    ctl::Call::new(&["add-key", "1", key.to_str().expect("key path"), "--key", "elsewhere-model"]),
                    ctl::Call::new(&["wait", "accounts", "--until", "usable", "--timeout", "10s"]),
                    ctl::Call::new(&[
                        "chat",
                        "--project",
                        &project.to_string(),
                        "--key",
                        "elsewhere-chat",
                        "@elsewhere-chat Land a README change in elsewhere/first",
                        "--json",
                    ]),
                    ctl::Call::new(&["wait", "task", "1", "--until", "replied", "--timeout", "120s", "--json"]),
                    ctl::Call::new(&["wait", "task", "2", "--until", "ended", "--timeout", "120s", "--json"]),
                    ctl::Call::new(&["task", "2"]),
                ] {
                    calls.push(ctl::Step::Run(call));
                }
            }
            self.stage = 1;
        } else if !self.stopped {
            calls.release();
            let Process::Binary(engine) = &mut procs[0] else { panic!("engine") };
            engine.signal(Signal::Terminate);
            if let Some(Process::Peer(peer)) = procs.get_mut(2) {
                peer.shutdown();
            }
            self.stopped = true;
        }
    }
    fn observe(&mut self, _now: Time, procs: &[Process]) {
        self.settled = procs.iter().all(Host::is_empty);
    }
    fn next_deadline(&self) -> Option<Time> {
        (!self.settled).then_some(self.deadline)
    }
    fn overdue(&self, now: Time) -> Option<String> {
        (!self.settled && now >= self.deadline).then(|| "elsewhere life deadline".into())
    }
    fn passed(&self) -> bool {
        self.settled
    }
}

fn no_token(bytes: &[u8], token: &[u8]) {
    assert!(!bytes.windows(token.len()).any(|part| part == token), "disposable credential escaped (redacted)");
}
fn inspect(root: &Path, token: &[u8]) {
    // jig's initial generation keeps its secrets separately from its store.
    let excluded = root.join("data/generations/1/secrets");
    let mut pending = vec![root.join("config"), root.join("data"), root.join("cache")];
    while let Some(path) = pending.pop() {
        if path == excluded {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).expect("deployment metadata");
        if metadata.is_dir() {
            pending.extend(fs::read_dir(path).expect("deployment entries").map(|entry| entry.expect("entry").path()));
        } else if metadata.is_file() {
            no_token(&fs::read(path).expect("deployment file"), token);
        }
    }
    let mut pending = vec![root.join("data/generations/1/store")];
    let mut files = 0;
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).expect("store metadata");
        assert!(!metadata.file_type().is_symlink(), "store has no symlinks");
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.is_dir() {
            assert_eq!(mode, 0o700, "private store directory");
            pending.extend(fs::read_dir(path).expect("store entries").map(|entry| entry.expect("entry").path()));
        } else {
            assert!(metadata.is_file(), "regular store file");
            assert_eq!(mode, 0o600, "private store file");
            files += 1;
        }
    }
    assert!(files > 0, "observed store files");
}
