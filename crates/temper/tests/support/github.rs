//! A person connects their App, pushes once and restarts (domain/testing.md, 2.6).
use super::{Scratch, WITHIN, cluster, ctl};
use serde_json::Value;
use skein_io::kernel::Exit;
use skein_lib::{Secret, Time, base64};
use skein_shell::{Clock, Host};
use skein_world::{Referee, real};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const INSTALLATION: u64 = 1;
const CODER: u64 = 3; // Fresh deployment: chat 1, change procedure 2, its coder 3.
const EDIT: &[u8] = b"Changed by temper.\n";

pub fn restart() {
    let clock = Clock::new();
    let began = clock.now().now;
    let scratch = Scratch::new("github_git");
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
    members.up(&["forge", "llm"]);
    let mut world = real::World::new(Judge {
        members,
        deadline: clock.now().now.saturating_add(WITHIN),
        stage: Stage::Fakes,
        scratch: scratch.path().to_path_buf(),
        clients,
        workers,
        app: None,
        identity: None,
        adoption: None,
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
    });
    for host in hosts {
        world.spawn(|| host);
    }
    world.spawn(cluster::Root::client);
    let outcome = world.run(&clock, WITHIN);
    assert!(outcome.procs.iter().all(Host::is_empty), "every process and client settled");
    scratch.remove();
    eprintln!("github git: complete {:?}", clock.now().now.saturating_since(began));
}

#[derive(Debug)]
enum Stage {
    Fakes,
    Worker,
    Install,
    Adopt,
    Chat,
    Tree,
    Coder,
    BeforeRestart,
    StopWorker,
    StopEngine,
    RestartEngine,
    RestartWorker,
    Restored,
    AdoptionRestored,
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
    adoption: Option<Value>,
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
}
impl Judge {
    fn mark(&self, step: &str) {
        eprintln!("github git: {step} {:?}", Clock::new().now().now.saturating_since(self.began));
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
        self.queue(host, vec![Self::adoption()]);
        self.stage = Stage::AdoptionRestored;
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
        for ended in calls.ends() {
            self.outputs.push(ended.stdout.clone());
            self.outputs.push(ended.stderr.clone());
            assert_eq!(
                ended.exit,
                Some(Exit::Code(0)),
                "ctl: {} {}",
                String::from_utf8_lossy(&ended.stderr),
                String::from_utf8_lossy(&ended.stdout)
            );
        }
        match self.stage {
            Stage::Install => {
                self.install(host, calls);
            }
            Stage::Adopt => {
                let packet = ctl::json(calls.ends().last().expect("adoption"));
                assert!(packet["adopted"].is_object(), "Must: first life adopts acme/app: {packet}");
                self.adoption = Some(packet["adopted"].clone());
                self.queue(
                    host,
                    vec![ctl::Call::new(&[
                        "chat",
                        "--project",
                        "1",
                        "--key",
                        "change",
                        "@forge_chat Update README",
                        "--json",
                    ])],
                );
                self.stage = Stage::Chat;
            }
            Stage::Chat => {
                self.mark("signed in, key usable, adopted, chat created");
                let packet = ctl::json(calls.ends().last().expect("chat creation"));
                assert_eq!(packet["outcome"]["done"]["owner"]["created"]["first"], 1, "{packet}");
                self.queue(host, vec![ctl::Call::new(&["task", "1", "--tree", "--json"])]);
                self.stage = Stage::Tree;
            }
            Stage::Tree => self.tree(host, &ctl::json(&calls.ends()[0])),
            Stage::Coder => {
                Self::coder(&ctl::json(&calls.ends()[0]));
                self.mark("coder delivered");
                self.queue(host, vec![ctl::Call::new(&["forges", "--json"])]);
                self.stage = Stage::BeforeRestart;
            }
            Stage::BeforeRestart => {
                let row = Self::row(&ctl::json(&calls.ends()[0]));
                assert_eq!(row["state"], "connected", "Must: connected before restart");
                assert_eq!(row["login"], "temper-person[bot]", "Must: connected as the person's App");
                assert!(row["id"].as_u64().is_some(), "Must: connected bot id");
                self.identity = Some(row);
                self.forge(true);
                self.mark("first life delivered; restarting");
                self.restart_began = Some(now);
                self.members.stop_one("w1");
                self.stage = Stage::StopWorker;
            }
            Stage::Restored => self.restored(host, &ctl::json(calls.ends().last().expect("restored forges"))),
            Stage::AdoptionRestored => {
                let packet = ctl::json(&calls.ends()[0]);
                assert_eq!(
                    packet["adopted"],
                    *self.adoption.as_ref().expect("first adoption"),
                    "Must: same adoption key answers its kept adopted outcome after restart"
                );
                self.mark("adoption found after restart");
                eprintln!(
                    "github restart: restored {:?}",
                    now.saturating_since(self.restart_began.expect("restart began"))
                );
                self.members.stop();
                self.stage = Stage::Stopping;
            }
            Stage::Fakes
            | Stage::Worker
            | Stage::StopWorker
            | Stage::StopEngine
            | Stage::RestartEngine
            | Stage::RestartWorker
            | Stage::Stopping => unreachable!("active calls"),
        }
    }
    fn tree(&mut self, host: &mut cluster::Root, packet: &Value) {
        let tree = &packet["tree"];
        // Watches carry their last digest forward, so discovering a new child never polls.
        if tree["rows"].as_array().expect("tree rows").iter().any(|row| row["chip"]["task"] == CODER) {
            self.mark("coder created");
            self.queue(host, vec![ctl::Call::new(&["wait", "task", &CODER.to_string(), "--until", "ended", "--json"])]);
            self.stage = Stage::Coder;
        } else {
            let digest = tree["digest"].as_str().unwrap_or_else(|| panic!("tree digest: {packet}"));
            self.queue(host, vec![ctl::Call::new(&["wait", "tree", "1", "--since", digest, "--json"])]);
        }
    }
    fn coder(task: &Value) {
        let task = &task["task"];
        assert_eq!(task["chip"]["task"], CODER, "{task}");
        let owner = &task["chip"]["details"]["owner"];
        assert!(owner["details"]["ending"]["done"].is_object(), "coder delivered: {task}");
        assert!(owner["details"]["ending"]["done"]["change"].is_object(), "change delivered: {task}");
        // begin() checks that this engine alone has exactly one worker position,
        // w1, and its linked line. Only that worker can have executed this coder.
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
        // Only the push's branch: once CI passes, the change may merge and move
        // main in either life; the landing's Musts are not this story's.
        let moved: Vec<_> =
            rows.iter().filter(|row| row["observation"] == "moved" && row["branch"] != "main").collect();
        assert_eq!(moved.len(), 1, "Must: one pushed branch across both lives");
        let identity = &self.identity.as_ref().expect("first identity")["id"];
        for row in rows.iter().filter(|row| row["observation"] == "moved" && row["branch"] == "main") {
            assert_eq!(&row["by"], identity, "Must: the base branch moves only by temper's own App");
        }
        assert_eq!(moved[0]["repository"], "acme/app");
        assert_ne!(moved[0]["branch"], "main");
        assert!(moved[0]["from"].is_null(), "push creates its branch");
        assert_eq!(moved[0]["by"], self.identity.as_ref().expect("first identity")["id"]);
        let commit = moved[0]["to"].as_u64().expect("pushed head");
        if !read_objects {
            return;
        }
        let address = self.members.capture("forge", "address").expect("forge address").parse().expect("socket");
        let objects = objects(address, commit);
        let object = objects["objects"]
            .as_array()
            .expect("objects")
            .iter()
            .find(|object| object["commit"] == commit)
            .expect("pushed commit");
        let tree = object["tree"].as_array().expect("pushed tree");
        let file = tree.iter().find(|file| decode(&file["path"]).as_ref() == b"README.md").expect("README");
        let expected = [temper_fake::forge::FILES[0].1, EDIT].concat();
        assert_eq!(decode(&file["content"]).as_ref(), expected, "the branch holds the coder's edit");
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
        if matches!(self.stage, Stage::Fakes) && self.members.is_up() {
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
            eprintln!("github restart: ready {:?}", now.saturating_since(self.restart_began.expect("restart began")));
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
                    | Stage::Tree
                    | Stage::Coder
                    | Stage::BeforeRestart
                    | Stage::Restored
                    | Stage::AdoptionRestored
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
            self.mark("Musts: adoption, one creation, no uninstall, one push, secrecy and teardown passed");
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
    base64::decode(base64::Alphabet::Standard, value.as_str().expect("base64 text").as_bytes(), 8192)
        .expect("file bytes")
}
// The fake's object route is credential-free and does not add a repository git call.
// Like support/webhooks.rs, the referee makes one bounded request to a separate process.
pub(super) fn objects(address: SocketAddr, commit: u64) -> Value {
    let within = std::time::Duration::from_secs(10);
    let mut socket = TcpStream::connect_timeout(&address, within).expect("object route");
    socket.set_read_timeout(Some(within)).expect("read bound");
    socket.set_write_timeout(Some(within)).expect("write bound");
    let body = format!("{{\"commits\":[{commit}]}}");
    write!(socket, "POST /.fake/git/objects HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("object request");
    let mut response = Vec::new();
    socket.take(65_536).read_to_end(&mut response).expect("object response");
    assert!(response.starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&response));
    let at = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").expect("HTTP head");
    serde_json::from_slice(&response[at + 4..]).expect("objects JSON")
}

pub(crate) struct App {
    pub(crate) id: u64,
    pub(crate) pem: Secret,
}
// These requests go to other processes' real listeners; unlike the one-ring
// tier, a bounded synchronous exchange cannot hold the server's loop up.
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) location: Option<String>,
    pub(crate) body: Vec<u8>,
}
pub(crate) fn request_url(address: SocketAddr, method: &str, url: &str, media: &str, body: &[u8]) -> Response {
    let origin = format!("http://{address}");
    let target = url.strip_prefix(&origin).expect("only the expected fake or engine origin");
    assert!(target.starts_with('/'), "local HTTP target");
    request(address, method, target, media, body)
}
fn request(address: SocketAddr, method: &str, target: &str, media: &str, body: &[u8]) -> Response {
    let within = std::time::Duration::from_secs(10);
    let mut socket = TcpStream::connect_timeout(&address, within).expect("local HTTP listener");
    socket.set_read_timeout(Some(within)).expect("read bound");
    socket.set_write_timeout(Some(within)).expect("write bound");
    let version = skein_forge::github::pin::API_VERSION;
    write!(socket, "{method} {target} HTTP/1.1\r\nHost: {address}\r\nUser-Agent: temper-test\r\nX-GitHub-Api-Version: {version}\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).expect("HTTP head");
    socket.write_all(body).expect("HTTP body");
    let mut bytes = Vec::new();
    socket.take(65_537).read_to_end(&mut bytes).expect("bounded HTTP response");
    assert!(bytes.len() <= 65_536, "HTTP response bound");
    let at = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n").expect("HTTP head");
    let head = std::str::from_utf8(&bytes[..at]).expect("HTTP header text");
    let status = head
        .lines()
        .next()
        .expect("status line")
        .split_whitespace()
        .nth(1)
        .expect("status")
        .parse()
        .expect("HTTP status");
    let location = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .map(|(_, value)| value.trim().to_owned());
    Response { status, location, body: bytes[at + 4..].to_vec() }
}
pub(crate) fn make_app(address: SocketAddr, clients: SocketAddr, scratch: &Path) -> App {
    use skein_forge::github::setup;
    use std::fmt::Write as _;
    let origin = format!("http://{clients}");
    let setup_url = format!("{origin}/forges/1/github/setup");
    let redirect = format!("{origin}/forges/1/github/redirect");
    let manifest = setup::manifest(
        &setup::Values {
            name: b"temper-person",
            url: origin.as_bytes(),
            hook_url: b"https://temper-test.invalid/",
            redirect_url: redirect.as_bytes(),
            setup_url: setup_url.as_bytes(),
        },
        &temper_fake::forge::git_limits().documents.writer,
    )
    .expect("temper manifest");
    let mut form = String::from("manifest=");
    for byte in &manifest {
        write!(form, "%{byte:02X}").expect("form byte");
    }
    let page = request(
        address,
        "POST",
        "/settings/apps/new?state=person",
        "application/x-www-form-urlencoded",
        form.as_bytes(),
    );
    assert_eq!(page.status, 200, "Must: manifest form makes the person's App");
    let html = std::str::from_utf8(&page.body).expect("App creation page");
    let code = html.split_once("?code=").expect("conversion link").1.split('&').next().expect("code");
    let converted = request(address, "POST", &format!("/app-manifests/{code}/conversions"), "application/json", b"");
    assert_eq!(converted.status, 201, "Must: manifest conversion over HTTP");
    let value: Value = serde_json::from_slice(&converted.body).expect("conversion JSON");
    let app = App {
        id: value["id"].as_u64().expect("converted App id"),
        pem: Secret::new(value["pem"].as_str().expect("converted PEM").as_bytes().into()),
    };
    let path = scratch.join("github-app.pem");
    let mut file =
        fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).expect("private App key");
    file.write_all(app.pem.expose()).expect("downloaded PEM");
    assert_eq!(fs::metadata(path).expect("PEM metadata").permissions().mode() & 0o777, 0o600);
    app
}

pub(crate) struct Scan {
    pem: Vec<u8>,
    der: Box<[u8]>,
    encoded: Vec<u8>,
}
impl Scan {
    pub(crate) fn new(pem: &[u8]) -> Self {
        let compact: String =
            std::str::from_utf8(pem).expect("PEM text").lines().filter(|line| !line.starts_with("-----")).collect();
        let der = base64::decode(base64::Alphabet::Standard, compact.as_bytes(), 4096).expect("App DER");
        Self { pem: pem.to_vec(), der, encoded: compact.into_bytes() }
    }
    pub(crate) fn clean(&self, bytes: &[u8]) {
        assert!(
            !bytes.windows(40).any(|part| part.starts_with(b"ghs_") && part[4..].iter().all(u8::is_ascii_alphanumeric)),
            "Must: no installation token outside jig's secrets store"
        );
        for (name, key) in
            [("PEM", self.pem.as_slice()), ("DER", self.der.as_ref()), ("base64 DER", self.encoded.as_slice())]
        {
            assert!(
                skein_lib::bytes::find(bytes, key).is_none(),
                "Must: no App {name} outside jig's secrets store and downloaded PEM"
            );
        }
    }
    pub(crate) fn files(&self, root: &Path, secrets: &Path, downloaded: &Path) {
        assert!(secrets.is_dir(), "excluded directory is jig's secrets store");
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            if path == secrets || path == downloaded {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).expect("scratch metadata");
            if metadata.is_dir() {
                pending.extend(
                    fs::read_dir(path).expect("scratch directory").map(|entry| entry.expect("scratch entry").path()),
                );
            } else if metadata.is_file() {
                self.clean(&fs::read(path).expect("scratch file"));
            }
        }
    }
}
