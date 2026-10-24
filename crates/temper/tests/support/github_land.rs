//! A chat lands on a connected App and finds the same landing after restart (domain/testing.md, 2.6).
use super::{
    Scratch, WITHIN, cluster, ctl,
    github::{self, App, Scan, make_app, request_url},
};
use serde_json::Value;
use skein_io::kernel::Exit;
use skein_lib::Time;
use skein_shell::{Clock, Host};
use skein_world::{Referee, real};
use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

const INSTALLATION: u64 = 1;
const CHANGE: u64 = 2; // Fresh deployment: chat 1, change 2, coder 3, reviewer 4.
const EDIT: &[u8] = b"Changed by temper.\n";

pub fn restart() {
    let clock = Clock::new();
    let began = clock.now().now;
    let scratch = Scratch::new("github_land");
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/github");
    let mut description = cluster::Cluster::read_from(scratch.path(), &directory);
    let [clients, workers] = super::free_addresses(&[]);
    description.bind("story.clients", &clients.to_string());
    description.prepare("forge");
    let mut fixture: Value = serde_json::from_str(
        &fs::read_to_string(description.dir("forge").join("github.json")).expect("GitHub fixture"),
    )
    .expect("GitHub JSON");
    // The document already admits an account with no installation. Retain
    // acme/app, but make the person's App and its installation through HTTP.
    fixture["github"]["accounts"][0]["installation"] = Value::Null;
    fs::write(description.dir("forge").join("github.json"), serde_json::to_vec(&fixture).expect("fixture"))
        .expect("uninstalled fixture");
    let key = description.dir("llm").join("llm.key");
    let observations = description.dir("forge").join("observations");
    let engine = description.dir("engine").join("config/temper.json");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_temper"), description.dir("w1").join("git"))
        .expect("worker's fake git");
    let (mut members, hosts) = description.members();
    members.up(&["forge"]);
    let mut world = real::World::new(Judge {
        members,
        deadline: clock.now().now.saturating_add(WITHIN),
        stage: Stage::Forge,
        scratch: scratch.path().to_path_buf(),
        clients,
        workers,
        app: None,
        identity: None,
        restored: None,
        outputs: Vec::new(),
        restart_began: None,
        key,
        observations,
        engine,
        client: scratch.path().join("client"),
        began,
        ready: BTreeSet::new(),
        call_marks: 0,
        landed: None,
        tasks: None,
        life_tasks: Vec::new(),
        reply_snapshot: None,
        replied: false,
    });
    for host in hosts {
        world.spawn(|| host);
    }
    world.spawn(cluster::Root::client);
    let outcome = world.run(&clock, WITHIN);
    assert!(outcome.procs.iter().all(Host::is_empty), "every process and client settled");
    scratch.remove();
    eprintln!("github land: complete {:?}", clock.now().now.saturating_since(began));
}

#[derive(Debug)]
enum Stage {
    Forge,
    Fakes,
    Worker,
    Install,
    Adopt,
    Chat,
    Change,
    ReplySnapshot,
    ReplyPages,
    ChatEnded,
    BeforeRestart,
    StopWorker,
    StopEngine,
    RestartEngine,
    RestartWorker,
    Restored,
    LandingRestored,
    TasksPage { restored: bool },
    Stopping,
}
struct Judge {
    members: cluster::Members,
    deadline: Time,
    stage: Stage,
    scratch: PathBuf,
    clients: SocketAddr,
    workers: SocketAddr,
    app: Option<App>,
    identity: Option<Value>,
    restored: Option<Value>,
    outputs: Vec<Vec<u8>>,
    restart_began: Option<Time>,
    key: PathBuf,
    observations: PathBuf,
    engine: PathBuf,
    client: PathBuf,
    began: Time,
    ready: BTreeSet<&'static str>,
    call_marks: usize,
    landed: Option<Value>,
    tasks: Option<Vec<u64>>,
    life_tasks: Vec<u64>,
    reply_snapshot: Option<String>,
    replied: bool,
}
impl Judge {
    fn mark(&self, step: &str) {
        eprintln!("github land: {step} {:?}", Clock::new().now().now.saturating_since(self.began));
    }
    fn queue(&mut self, host: &mut cluster::Root, calls: Vec<ctl::Call>) {
        self.call_marks = 0;
        let link = format!("http://{}", self.members.capture("engine", "link").expect("engine link"));
        let mut batch = ctl::Calls::linked(&link, &self.client);
        for call in calls {
            batch.push(ctl::Step::Run(call));
        }
        let cluster::Root::Client { calls, .. } = host else { panic!("client root") };
        *calls = Some(Box::new(batch));
    }
    fn begin(&mut self, host: &mut cluster::Root) {
        use ctl::Call;
        let engine: Value =
            serde_json::from_slice(&fs::read(&self.engine).expect("engine document")).expect("engine JSON");
        assert_eq!(engine["workers"], serde_json::json!([]), "engine alone");
        assert_eq!(engine["application"]["workers"], serde_json::json!([]));
        let workers = engine["engine"]["hosts"]["workers"].as_array().expect("worker positions");
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0]["name"], "w1");
        assert_eq!(self.members.linked("w1"), 1, "the sole worker linked");
        self.queue(
            host,
            vec![
                Call::sign_in(),
                Call::new(&[
                    "add-key",
                    "1",
                    self.key.to_str().expect("key path"),
                    "--deployment",
                    "--key",
                    "github-llm",
                ]),
                Call::new(&["wait", "accounts", "--until", "usable"]),
                Call::new(&[
                    "forge-connect",
                    "1",
                    "github-app",
                    &self.app.as_ref().expect("made App").id.to_string(),
                    self.scratch.join("github-app.pem").to_str().expect("PEM path"),
                    "--key",
                    "github-app",
                ]),
                Call::new(&["wait", "forges", "--until", "connected:1"]),
                Call::new(&["forge-install", "1", "--key", "github-install"]),
            ],
        );
        self.stage = Stage::Install;
    }
    fn adoption() -> ctl::Call {
        ctl::Call::new(&[
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
            "adopt-app",
            "--json",
        ])
    }
    fn row(packet: &Value) -> Value {
        packet["forges"]
            .as_array()
            .expect("forges rows")
            .iter()
            .find(|row| row["number"] == 1)
            .expect("Must: forge 1 is present")
            .clone()
    }
    fn restored(&mut self, host: &mut cluster::Root, packet: &Value) {
        let row = Self::row(packet);
        // The account is learned from the token minted after startup. Wait on
        // the document's digest when T1 exposes a row still learning it.
        if row["installations"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty() && rows.iter().any(|row| row["account"].is_null()))
        {
            let digest = packet["digest"].as_str().expect("forges digest");
            self.queue(host, vec![ctl::Call::new(&["wait", "forges", "--since", digest, "--json"])]);
            return;
        }
        self.restored = Some(row);
        self.queue(host, landing_reads());
        self.stage = Stage::LandingRestored;
    }
    fn install(&mut self, host: &mut cluster::Root, calls: &ctl::Calls) {
        let output = std::str::from_utf8(&calls.ends().last().expect("install link").stdout).expect("install output");
        let url = output.lines().find(|line| line.starts_with("http://")).expect("printed install link");
        let address = self.members.capture("forge", "address").expect("forge address").parse().expect("socket");
        let page = request_url(address, "GET", url, "", b"");
        assert_eq!(page.status, 302, "Must: fake install redirects to setup");
        let location = page.location.expect("setup redirect");
        assert!(location.starts_with(&format!("http://{}/forges/1/github/setup?", self.clients)), "setup URL");
        assert!(
            location.contains(&format!("installation_id={INSTALLATION}")),
            "Must: accepted visited installation id"
        );
        let accepted = request_url(self.clients, "GET", &location, "", b"");
        assert_eq!(accepted.status, 200, "Must: setup accepted installation");
        assert!(
            skein_lib::bytes::find(&accepted.body, b"temper has the installation; you may close this tab.").is_some(),
            "the setup page says so"
        );
        self.outputs.push(accepted.body);
        self.queue(host, vec![ctl::Call::new(&["wait", "forges", "--until", "settled"]), Self::adoption()]);
        self.stage = Stage::Adopt;
    }
    fn called(&mut self, now: Time, host: &mut cluster::Root, calls: &ctl::Calls) {
        for (index, ended) in calls.ends().iter().enumerate() {
            self.outputs.push(ended.stdout.clone());
            self.outputs.push(ended.stderr.clone());
            assert_eq!(
                ended.exit,
                Some(Exit::Code(
                    if index == 6 && matches!(self.stage, Stage::BeforeRestart | Stage::LandingRestored) {
                        2
                    } else {
                        0
                    }
                )),
                "ctl: {} {}",
                String::from_utf8_lossy(&ended.stderr),
                String::from_utf8_lossy(&ended.stdout)
            );
        }
        match self.stage {
            Stage::Install => {
                self.install(host, calls);
            }
            Stage::Adopt => self.adopted(host, calls),
            Stage::Chat => {
                self.mark("signed in, key usable, adopted, chat created");
                let packet = ctl::json(calls.ends().last().expect("chat creation"));
                assert_eq!(packet["outcome"]["done"]["owner"]["created"]["first"], 1, "{packet}");
                // The chat's first reply acknowledges its already committed change task.
                self.queue(
                    host,
                    vec![
                        ctl::Call::new(&["wait", "task", "1", "--until", "replied"]),
                        ctl::Call::new(&["wait", "task", "2", "--until", "ended", "--json"]),
                    ],
                );
                self.stage = Stage::Change;
            }
            Stage::Change => {
                let packet = ctl::json(calls.ends().last().expect("landed change"));
                assert_eq!(packet["task"]["chip"]["phase"], "ended");
                assert_eq!(packet["task"]["result"]["value"]["change"]["kind"], 1);
                self.landed = Some(packet["task"]["result"].clone());
                self.mark("change landed");
                self.queue(host, vec![ctl::Call::new(&["task", "1", "--json"])]);
                self.stage = Stage::ReplySnapshot;
            }
            Stage::ReplySnapshot => {
                let packet = ctl::json(calls.ends().last().expect("reply snapshot"));
                self.reply_snapshot = Some(packet["digest"].as_str().expect("task digest").to_owned());
                self.queue(host, vec![ctl::Call::new(&["chat", "1", "--json"])]);
                self.stage = Stage::ReplyPages;
            }
            Stage::ReplyPages => self.reply_page(host, calls),
            Stage::ChatEnded => {
                assert_eq!(ctl::json(calls.ends().last().expect("chat ended"))["task"]["chip"]["phase"], "ended");
                self.queue(host, landing_reads());
                self.stage = Stage::BeforeRestart;
            }
            Stage::BeforeRestart => self.before_restart(now, host, calls),
            Stage::Restored => self.restored(host, &ctl::json(calls.ends().last().expect("restored forges"))),
            Stage::LandingRestored => {
                let cursor = self.check_landing(calls);
                self.read_tasks(now, host, true, cursor);
            }
            Stage::TasksPage { restored } => {
                let packet = ctl::json(&calls.ends()[0]);
                let rows = &packet["ended"]["rows"];
                self.collect_tasks(rows);
                let cursor = rows["after"]["value"].as_str().map(str::to_owned);
                self.read_tasks(now, host, restored, cursor);
            }
            Stage::Forge
            | Stage::Fakes
            | Stage::Worker
            | Stage::StopWorker
            | Stage::StopEngine
            | Stage::RestartEngine
            | Stage::RestartWorker
            | Stage::Stopping => unreachable!("active calls"),
        }
    }
    fn adopted(&mut self, host: &mut cluster::Root, calls: &ctl::Calls) {
        let packet = ctl::json(calls.ends().last().expect("adoption"));
        assert!(packet["adopted"].is_object(), "Must: first life adopts acme/app: {packet}");
        self.queue(
            host,
            vec![ctl::Call::new(&["chat", "--project", "1", "--key", "change", "@forge_chat Update README", "--json"])],
        );
        self.stage = Stage::Chat;
    }
    fn reply_page(&mut self, host: &mut cluster::Root, calls: &ctl::Calls) {
        let page = ctl::json(&calls.ends()[0]);
        self.replied |= page["activity"]["rows"].to_string().contains("The change landed.");
        if let Some(before) = page["activity"]["before"]["value"].as_str() {
            self.queue(host, vec![ctl::Call::new(&["chat", "1", "--before", before, "--json"])]);
        } else if !self.replied {
            self.queue(
                host,
                vec![
                    ctl::Call::new(&[
                        "wait",
                        "task",
                        "1",
                        "--until",
                        "replied",
                        "--since",
                        self.reply_snapshot.as_deref().expect("reply snapshot"),
                        "--json",
                    ]),
                    ctl::Call::new(&["task", "1", "--json"]),
                ],
            );
            self.stage = Stage::ReplySnapshot;
        } else {
            self.queue(
                host,
                vec![
                    ctl::Call::new(&["close", "1", "--key", "close-land", "--json"]),
                    ctl::Call::new(&["wait", "task", "1", "--until", "ended", "--json"]),
                ],
            );
            self.stage = Stage::ChatEnded;
        }
    }
    fn before_restart(&mut self, now: Time, host: &mut cluster::Root, calls: &ctl::Calls) {
        let cursor = self.check_landing(calls);
        let row = Self::row(&ctl::json(&calls.ends()[4]));
        assert_eq!(row["state"], "connected", "Must: connected before restart");
        assert_eq!(row["login"], "temper-person[bot]", "Must: connected as the person's App");
        assert!(row["id"].as_u64().is_some(), "Must: connected bot id");
        self.identity = Some(row);
        self.forge(true);
        self.read_tasks(now, host, false, cursor);
    }
    fn collect_tasks(&mut self, rows: &Value) {
        assert_eq!(rows["cut"], false, "whole task page");
        self.life_tasks.extend(rows["rows"].as_array().expect("ended rows").iter().map(|row| {
            assert_eq!(row["chip"]["phase"], "ended");
            row["chip"]["task"].as_u64().expect("task id")
        }));
    }
    fn read_tasks(&mut self, now: Time, host: &mut cluster::Root, restored: bool, cursor: Option<String>) {
        if let Some(cursor) = cursor {
            self.queue(
                host,
                vec![ctl::Call::new(&[
                    "tasks",
                    "--project",
                    "1",
                    "--parent",
                    "2",
                    "--ended",
                    "--after",
                    &cursor,
                    "--json",
                ])],
            );
            self.stage = Stage::TasksPage { restored };
            return;
        }
        self.life_tasks.sort_unstable();
        assert_eq!(self.life_tasks, [1, 2, 3, 4], "the complete task set");
        if restored {
            assert_eq!(Some(&self.life_tasks), self.tasks.as_ref(), "no task made after restart");
            self.mark("same landing and task set found after restart");
            eprintln!(
                "github land restart: restored {:?}",
                now.saturating_since(self.restart_began.expect("restart began"))
            );
            self.members.stop();
            self.stage = Stage::Stopping;
        } else {
            self.tasks = Some(self.life_tasks.clone());
            self.mark("first life landed and replied; restarting");
            self.restart_began = Some(now);
            self.members.stop_one("w1");
            self.stage = Stage::StopWorker;
        }
    }
    fn check_landing(&mut self, calls: &ctl::Calls) -> Option<String> {
        let task = ctl::json(&calls.ends()[0]);
        assert_eq!(task["chip"]["phase"], "ended");
        assert_eq!(task["result"], *self.landed.as_ref().expect("first landed result"), "same PR and merge commit");
        assert_eq!(ctl::json(&calls.ends()[1])["chip"]["phase"], "ended", "the Open chat was closed before restart");
        let unknown = &calls.ends()[6];
        assert_eq!(unknown.exit, Some(Exit::Code(2)));
        assert_eq!(
            unknown.stderr.trim_ascii_end(),
            br#"{"error":"unknown"}"#,
            "no task beyond the four first-life tasks"
        );
        self.life_tasks = vec![1];
        for index in [2, 3] {
            let packet = ctl::json(&calls.ends()[index]);
            super::forge_land::assert_no_live_tasks(&packet);
            let rows = &packet["ended"]["rows"];
            if index == 2 {
                assert!(rows["after"].is_null(), "complete single change delegate");
                assert_eq!(rows["rows"].as_array().expect("ended rows").len(), 1);
                assert_eq!(rows["rows"][0]["chip"]["task"], CHANGE);
            }
            self.collect_tasks(rows);
        }
        super::forge_land::assert_no_live_roots(&ctl::json(&calls.ends()[5]));
        let packet = ctl::json(&calls.ends()[3]);
        packet["ended"]["rows"]["after"]["value"].as_str().map(str::to_owned)
    }
    fn forge(&self, read_objects: bool) {
        let lines = fs::read_to_string(&self.observations).expect("forge observations");
        let rows: Vec<Value> =
            lines.lines().map(|line| serde_json::from_str(line).expect("observation JSON")).collect();
        assert_eq!(rows.last().expect("checkpoint")["observations_lost"], 0);
        let calls: Vec<_> = rows.iter().filter(|row| row["observation"] == "git_call").collect();
        assert!(calls.iter().any(|row| row["writes"] == false && row["status"] == 200), "clone/read observed");
        assert_eq!(
            calls.iter().filter(|row| row["writes"] == true).count(),
            1,
            "Must: exactly one push across both lives"
        );
        for call in calls {
            // The record's closed schema is the token-secrecy proof: no string/value field.
            let decoded =
                temper_fake::forge::observations::decode(call.to_string().as_bytes()).expect("closed git record");
            assert!(matches!(decoded, Some(temper_fake::forge::observations::Observation::GitCall(_))));
            assert_eq!(call["installation"], INSTALLATION);
            assert!(call["mint"].as_u64().is_some_and(|mint| mint > 0));
            if call["writes"] == true {
                assert_eq!(call["status"], 200, "push succeeds");
            } else {
                // Before its first push git checks whether the new branch exists.
                assert!(call["status"] == 200 || call["status"] == 404, "git read: {call}");
            }
        }
        assert!(
            !rows
                .iter()
                .any(|row| row["observation"] == "git_calls_full" || row["observation"] == "installations_full"),
            "Must: observation stream complete"
        );
        let created: Vec<_> = rows.iter().filter(|row| row["observation"] == "installation_created").collect();
        assert_eq!(created.len(), 1, "Must: one installation creation across both lives, no second acceptance");
        assert_eq!(created[0]["installation"], INSTALLATION, "Must: the visited installation was created");
        assert!(
            !rows.iter().any(|row| row["observation"] == "installation_deleted"),
            "Must: no uninstall across both lives"
        );
        for row in rows
            .iter()
            .filter(|row| matches!(row["observation"].as_str(), Some("installation_created" | "installation_deleted")))
        {
            temper_fake::forge::observations::decode(row.to_string().as_bytes())
                .expect("closed installation record")
                .expect("installation observation");
        }
        let opened: Vec<_> =
            rows.iter().filter(|row| row["observation"] == "opened" && row["kind"] == "pull").collect();
        let merged: Vec<_> = rows.iter().filter(|row| row["observation"] == "merged").collect();
        assert_eq!(opened.len(), 1, "one pull request across both lives");
        assert_eq!(merged.len(), 1, "one merge across both lives");
        let merge = merged[0];
        assert_eq!(opened[0]["repository"], "acme/app");
        assert_eq!(merge["repository"], "acme/app");
        assert_eq!(merge["number"], opened[0]["number"]);
        assert_eq!(merge["base"], "main");
        let bot = &self.identity.as_ref().expect("first bot")["id"];
        assert_eq!(merge["by"], *bot);
        assert_eq!(opened[0]["by"], *bot);
        let pushed: Vec<_> =
            rows.iter().filter(|row| row["observation"] == "moved" && row["branch"] != "main").collect();
        assert_eq!(pushed.len(), 1);
        assert_eq!(merge["head"], pushed[0]["to"], "merge at the coder's pushed head");
        let main: Vec<_> = rows.iter().filter(|row| row["observation"] == "moved" && row["branch"] == "main").collect();
        assert_eq!(main.len(), 1);
        assert_eq!(main[0]["to"], merge["commit"], "merge commit is main's tip");
        let commit = merge["commit"].as_u64().expect("merge commit");
        let mut hex = String::new();
        for byte in skein_fake_forge_domain::object_bytes(commit) {
            write!(hex, "{byte:02x}").expect("commit hex");
        }
        let result = &self.landed.as_ref().expect("landed result")["value"]["change"];
        assert_eq!(result["words"]["bytes"], hex);
        assert_eq!(result["part"]["key"]["number"], merge["number"]);
        if read_objects {
            let address = self.members.capture("forge", "address").expect("forge address").parse().expect("socket");
            let objects = github::objects(address, commit);
            let object = objects["objects"]
                .as_array()
                .expect("objects")
                .iter()
                .find(|object| object["commit"] == commit)
                .expect("merge object");
            let file = object["tree"]
                .as_array()
                .expect("tree")
                .iter()
                .find(|file| decode(&file["path"]).as_ref() == b"README.md")
                .expect("README");
            assert_eq!(decode(&file["content"]).as_ref(), [temper_fake::forge::FILES[0].1, EDIT].concat());
        }
    }
}
impl Referee<cluster::Root> for Judge {
    fn act(&mut self, now: Time, hosts: &mut [cluster::Root]) {
        self.members.act(now, hosts);
        if self.members.failed() {
            self.stage = Stage::Stopping;
            return;
        }
        let host = hosts.last_mut().expect("client root");
        if matches!(self.stage, Stage::Forge) && self.members.is_up() {
            self.members.prepare("llm");
            configure_llm(&self.members.dir("llm").join("llm.json"));
            self.members.up(&["llm"]);
            self.stage = Stage::Fakes;
        } else if matches!(self.stage, Stage::Fakes) && self.members.is_up() {
            self.mark("fakes ready");
            let api = self.members.capture("forge", "address").expect("forge address").parse().expect("API socket");
            self.app = Some(make_app(api, self.clients, &self.scratch));
            self.mark("App made by manifest and conversion");
            assert!(!self.members.dir("engine").join("data/state").exists(), "Must: no seeded forge records");
            self.members.prepare("engine");
            let mut document: Value =
                serde_json::from_slice(&fs::read(&self.engine).expect("engine configuration")).expect("engine JSON");
            // Restart retains the worker's files. Reserve its engine endpoint,
            // as the clients' endpoint is reserved for the App's setup URL.
            document["engine"]["hosts"]["listener"][0]["socket"] = self.workers.to_string().into();
            let address = self.members.capture("forge", "address").expect("forge address").parse().expect("socket");
            configure_engine(&mut document, address);
            fs::write(&self.engine, serde_json::to_vec(&document).expect("engine JSON"))
                .expect("fixed workers listener");
            self.members.up(&["w1"]);
            self.stage = Stage::Worker;
            self.members.act(now, hosts);
        } else if matches!(self.stage, Stage::Worker) && self.members.is_up() {
            self.mark("engine and worker ready");
            self.begin(host);
        } else if matches!(self.stage, Stage::StopWorker) && self.members.ended("w1") {
            self.members.stop_one("engine");
            self.stage = Stage::StopEngine;
        } else if matches!(self.stage, Stage::StopEngine) && self.members.ended("engine") {
            self.members.restart("engine");
            self.stage = Stage::RestartEngine;
        } else if matches!(self.stage, Stage::RestartEngine) && self.members.capture("engine", "link").is_some() {
            self.members.restart("w1");
            self.stage = Stage::RestartWorker;
        } else if matches!(self.stage, Stage::RestartWorker) && self.members.is_up() {
            self.mark("engine and worker restarted on ready lines");
            eprintln!(
                "github land restart: ready {:?}",
                now.saturating_since(self.restart_began.expect("restart began"))
            );
            assert_eq!(self.members.linked("w1"), 2, "Must: worker linked in both lives");
            self.queue(
                host,
                vec![
                    ctl::Call::new(&["wait", "forges", "--until", "connected:1"]),
                    ctl::Call::new(&["forges", "--json"]),
                ],
            );
            self.stage = Stage::Restored;
        } else if host.is_empty()
            && matches!(
                self.stage,
                Stage::Install
                    | Stage::Adopt
                    | Stage::Chat
                    | Stage::Change
                    | Stage::ReplySnapshot
                    | Stage::ReplyPages
                    | Stage::ChatEnded
                    | Stage::BeforeRestart
                    | Stage::Restored
                    | Stage::LandingRestored
                    | Stage::TasksPage { .. }
            )
        {
            let cluster::Root::Client { calls, .. } = host else { panic!("client root") };
            let calls = calls.take().expect("completed calls");
            self.called(now, host, &calls);
        }
    }
    fn observe(&mut self, now: Time, hosts: &[cluster::Root]) {
        self.members.observe(now, &hosts[..hosts.len() - 1]);
        for (process, capture) in [("forge", "address"), ("llm", "address"), ("engine", "link"), ("w1", "engine")] {
            if self.members.capture(process, capture).is_some() && self.ready.insert(process) {
                self.mark(&format!("{process} ready"));
            }
        }
        if let Some(cluster::Root::Client { calls: Some(calls), .. }) = hosts.last() {
            for index in self.call_marks..calls.ends().len() {
                self.mark(&format!("ctl {:?} call {} completed", self.stage, index + 1));
            }
            self.call_marks = calls.ends().len();
        }
        if let Some(stopped) = self.members.stopped() {
            assert!(hosts.iter().all(Host::is_empty), "Must: no process or ctl client outlives the story");
            assert_eq!(
                stopped.iter().map(|member| member.name.as_str()).collect::<Vec<_>>(),
                ["w1", "engine", "w1", "engine", "llm", "forge"]
            );
            for (index, member) in stopped.iter().enumerate() {
                assert_eq!(
                    member.exit,
                    Some(Exit::Code(0)),
                    "{}: {}",
                    member.name,
                    String::from_utf8_lossy(&member.stderr)
                );
                assert_eq!(member.ending, if index < 2 { cluster::Ending::Alone } else { cluster::Ending::Cluster });
                assert!(!member.killed && member.survivors.is_empty(), "no surviving process");
            }
            self.forge(false);
            let app = self.app.as_ref().expect("made App");
            let scan = Scan::new(app.pem.expose());
            scan.files(
                &self.scratch,
                &self.members.dir("engine").join("data/state/generations/1/secrets"),
                &self.scratch.join("github-app.pem"),
            );
            for member in stopped {
                scan.clean(&member.stdout);
                scan.clean(&member.stderr);
            }
            for output in &self.outputs {
                scan.clean(output);
            }
            self.mark("Musts: same landing, one PR, one merge, no new tasks, secrecy and teardown passed");
            let row = self.restored.as_ref().expect("restored forges row");
            let prior = self.identity.as_ref().expect("first life identity");
            assert_eq!(row["state"], "connected", "Must: forge 1 connected after restart");
            assert_eq!(row["id"], prior["id"], "Must: same bot id after restart");
            assert_eq!(row["login"], prior["login"], "Must: same SLUG[bot] after restart");
            let installations =
                row["installations"].as_array().expect("Must (T1): restored forge row exposes installations");
            assert_eq!(installations.len(), 1, "Must (T1): exactly one accepted installation after restart");
            let installation = &installations[0];
            assert_eq!(installation["id"], INSTALLATION, "Must (T1): same visited installation after restart");
            assert_eq!(
                installation["account"]["login"], "acme",
                "Must (T1): installation account filled after startup mint"
            );
            assert_eq!(installation["stray"], false, "Must (T1): accepted installation is not stray");
            assert_eq!(installation["suspended"], false, "Must (T1): installation is not suspended");
        }
    }
    fn next_deadline(&self) -> Option<Time> {
        Some(self.members.next_deadline().map_or(self.deadline, |at| at.min(self.deadline)))
    }
    fn overdue(&self, now: Time) -> Option<String> {
        (now >= self.deadline).then(|| format!("GitHub process story waited in {:?}", self.stage))
    }
    fn passed(&self) -> bool {
        self.members.passed()
    }
}

fn decode(value: &Value) -> Box<[u8]> {
    skein_lib::base64::decode(
        skein_lib::base64::Alphabet::Standard,
        value.as_str().expect("base64 text").as_bytes(),
        8192,
    )
    .expect("file bytes")
}
pub(crate) fn configure_llm(path: &Path) {
    use serde_json::json;
    let mut value: Value = serde_json::from_slice(&fs::read(path).expect("LLM fixture")).expect("LLM JSON");
    value["cued"][0]["turns"][0]["lines"] =
        json!([{"call":"change","arguments":{"words":"@coding Fix","repository":"acme/app"}}]);
    // This fixture's chat is Open, as in forge_land: text replies, then Close ends it.
    value["cued"][0]["turns"].as_array_mut().expect("chat turns").extend([
        json!({"lines":[{"text":"The change landed."}],"finish":"stop","tokens":20}),
        json!({"lines":[{"text":"The change chat is closed."}],"finish":"stop","tokens":20}),
    ]);
    value["cued"].as_array_mut().expect("scripts").push(json!({"cue":"Review this change at the named head","turns":[{"lines":[{"call":"finish","arguments":{"form":"verdict","label":"1","text":"Checked","fields":{},"items":[]}}],"finish":"tool_calls","tokens":20}]}));
    super::write_private(path, &serde_json::to_vec(&value).expect("LLM JSON"));
}
fn configure_engine(value: &mut Value, forge: SocketAddr) {
    configure_landing(value, forge, "acme", "app");
    let limits = &mut value["application"]["limits"]["overrides"];
    for (name, millis) in [("poll_ms", 10), ("poll_max_ms", 50), ("slow_ms", 100), ("hinted_ms", 10)] {
        limits[format!("forge.client.{name}")] = millis.into();
    }
    limits["forge.client.window_ms"] = 100.into();
    limits["forge.client.pending"] = 8.into();
    limits["forge.client.calls"] = 4.into();
    limits["forge_component.connections"] = 4.into();
    limits["forge_component.log_poll_ms"] = 10.into();
}

pub(crate) fn landing_reads() -> Vec<ctl::Call> {
    [
        &["task", "2", "--json"][..],
        &["task", "1", "--json"],
        &["tasks", "--project", "1", "--parent", "1", "--ended", "--json"],
        &["tasks", "--project", "1", "--parent", "2", "--ended", "--json"],
        &["forges", "--json"],
        &["tasks", "--project", "1", "--json"],
        &["task", "5", "--json"],
    ]
    .into_iter()
    .map(ctl::Call::new)
    .collect()
}

pub(crate) fn configure_landing(value: &mut Value, forge: SocketAddr, owner: &str, repository: &str) {
    use serde_json::json;
    value["application"]["landing"]["projects"] = json!([{"project":1,"rules":[{"connector":1,"kind":4,"pattern":{"segments":["forge",forge.to_string(),owner,repository,"branch"],"last":"exact","value":"main"},"enforce":true,"ci":true,"up_to_date":true,"gates":[{"number":1,"blocking":true,"freshness":"exact"}],"approvals":[]}]}]);
}
