//! The process tier's owned Forgejo, credentials and landing scripts.
use super::{Forge, Scratch, owned, write_private};
use serde_json::{Value, json};
use skein_throwaway_forgejo::{Instance, Outgoing, Token};
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use temper_real_loop_world::landing;
pub type BranchHead = (Box<[u8]>, Box<[u8]>);
// The landing story gathers its agent's brief before the person's next command.
const BRIEF_GATHER_MS: u64 = 5000;

/// The owned Forgejo of domain/testing.md 2.6, prepared, run and reconciled
/// by temper's commands with its runner, attached through skein's `Instance`,
/// with the person's token and repository.
pub struct Throwaway {
    instance: Instance,
    pid: u32,
    forgejo: PathBuf,
    runner: owned::Running,
    person: Option<Token>,
    repository: Option<u64>,
    directory: PathBuf,
    webhooks: Option<SocketAddr>,
}
impl Throwaway {
    #[must_use]
    pub fn start(scratch: &Scratch) -> Self {
        Self::start_with_job_sleep(scratch, 60)
    }

    #[must_use]
    pub fn start_with_job_sleep(scratch: &Scratch, sleep_seconds: u64) -> Self {
        let mut fixture = Self::start_bare(scratch);
        let began = skein_shell::Clock::new().now().now;
        let reconciled = owned::reconcile(scratch.path(), 1);
        assert_eq!(reconciled.status.code(), Some(0), "{}", String::from_utf8_lossy(&reconciled.stderr));
        assert!(
            skein_shell::Clock::new().now().now.saturating_since(began) < super::WITHIN,
            "reconcile within its bound"
        );
        stage("reconcile", began);
        let person = person_token(&fixture.instance);
        let repository = repository(&fixture.instance, &person, sleep_seconds);
        owned::adopt(scratch.path(), &fixture.instance, &person, fixture.webhooks);
        fixture.person = Some(person);
        fixture.repository = Some(repository);
        fixture
    }

    /// The pinned server and runner with their original administrator, before Reconcile or adoption.
    #[must_use]
    pub fn start_bare(scratch: &Scratch) -> Self {
        let address = owned::tier_configuration(scratch.path());
        let running = owned::start(scratch.path());
        let pid = running.id();
        let began = skein_shell::Clock::new().now().now;
        let instance = owned::attach(scratch.path(), address.port(), running);
        stage("attach", began);
        let began = skein_shell::Clock::new().now().now;
        let mut runner = owned::run(scratch.path(), "runner");
        runner.declared();
        stage("runner", began);
        let config: Value =
            serde_json::from_slice(&fs::read(scratch.path().join("temper.json")).expect("configuration"))
                .expect("JSON");
        let webhooks = config["application"]["webhooks"]["listener"]["socket"]
            .as_str()
            .map(|address| address.parse().expect("webhooks address"));
        Self {
            instance,
            pid,
            forgejo: owned::binaries().0,
            runner,
            person: None,
            repository: None,
            directory: scratch.path().into(),
            webhooks,
        }
    }

    /// Forgejo's pid, which `temper forge run` became.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    #[must_use]
    pub fn forge(&self) -> Forge<'_> {
        Forge::Throwaway {
            forgejo: &self.forgejo,
            api: SocketAddr::from(([127, 0, 0, 1], self.instance.port())),
            git: self.instance.base(),
        }
    }
    fn get(&self, suffix: &str) -> skein_throwaway_forgejo::Exchange {
        self.instance
            .exchange(&Outgoing::get(
                &format!("/api/v1/repos/acme/widget/{suffix}"),
                Some(self.person.as_ref().expect("fixture person")),
            ))
            .expect("throwaway forge answer")
    }
    #[must_use]
    pub fn merged_pull(&self) -> Option<u64> {
        let response = self.get("pulls?state=closed&limit=2");
        assert_eq!(response.status, 200, "closed pull list");
        let value: Value = serde_json::from_slice(&response.body).expect("pull JSON");
        value.as_array().expect("pull list").iter().find_map(|pull| {
            (pull["merged"].as_bool() == Some(true)).then(|| pull["number"].as_u64().expect("pull number"))
        })
    }
    #[must_use]
    pub fn job_state(&self, pull: u64) -> String {
        let response = self.get(&format!("pulls/{pull}"));
        assert_eq!(response.status, 200, "pull head");
        let pull: Value = serde_json::from_slice(&response.body).expect("pull JSON");
        let head = pull["head"]["sha"].as_str().expect("pull head");
        let response = self.get(&format!("commits/{head}/status"));
        assert_eq!(response.status, 200, "job status");
        let status: Value = serde_json::from_slice(&response.body).expect("status JSON");
        status["state"].as_str().expect("job state").into()
    }
    #[must_use]
    pub fn main_readme(&self) -> Vec<u8> {
        let response = self.get("raw/README.md?ref=main");
        assert_eq!(response.status, 200, "main README");
        response.body
    }
    #[must_use]
    pub fn branch_head(&self, branch: &[u8]) -> Option<Box<[u8]>> {
        let branch = std::str::from_utf8(branch).expect("branch words");
        let response = self.get(&format!("branches/{branch}"));
        if response.status == 404 {
            return None;
        }
        assert_eq!(response.status, 200, "branch answer");
        let value: Value = serde_json::from_slice(&response.body).expect("branch JSON");
        Some(value["commit"]["id"].as_str().expect("branch commit").as_bytes().into())
    }
    #[must_use]
    pub fn branch_heads(&self) -> Vec<BranchHead> {
        let response = self.get("branches?limit=8");
        assert_eq!(response.status, 200, "branch list");
        let value: Value = serde_json::from_slice(&response.body).expect("branches JSON");
        value
            .as_array()
            .expect("branches")
            .iter()
            .map(|branch| {
                (
                    branch["name"].as_str().expect("branch name").as_bytes().into(),
                    branch["commit"]["id"].as_str().expect("branch commit").as_bytes().into(),
                )
            })
            .collect()
    }
    #[must_use]
    pub fn failure_state(&self) -> String {
        let response = self.get("pulls?state=all&limit=8");
        assert_eq!(response.status, 200, "all pull list");
        let pulls: Value = serde_json::from_slice(&response.body).expect("pull JSON");
        let response = self.get("branches?limit=8");
        assert_eq!(response.status, 200, "branch list");
        let branches: Value = serde_json::from_slice(&response.body).expect("branches JSON");
        let names: Vec<_> = branches
            .as_array()
            .expect("branches")
            .iter()
            .map(|branch| branch["name"].as_str().expect("branch name"))
            .collect();
        format!(
            "{} pulls, merged={}, branches={names:?}, main edited={}",
            pulls.as_array().expect("pulls").len(),
            self.merged_pull().is_some(),
            self.main_readme().ends_with(super::EDIT)
        )
    }
    pub fn stop(self) {
        self.runner.stop();
        self.instance.stop().expect("owned Forgejo stopped");
    }
    #[must_use]
    pub fn repository(&self) -> u64 {
        self.repository.expect("fixture repository")
    }
    /// The configured address, held only by the test or the running service.
    #[must_use]
    pub fn webhooks(&self) -> SocketAddr {
        self.webhooks.expect("webhooks configured")
    }
    /// The reconciled secret, without its optional final line feed.
    #[must_use]
    pub fn secret(&self) -> Vec<u8> {
        let secret = fs::read(self.directory.join("forge-1/webhook_secret")).expect("webhook secret");
        secret.strip_suffix(b"\n").unwrap_or(&secret).to_vec()
    }
    /// Commits one file to main through the person's contents API.
    pub fn push(&self, file: &str) {
        let path = format!("/api/v1/repos/acme/widget/contents/{file}");
        let body = serde_json::to_vec(&json!({"branch":"main","message":format!("Add {file}"),"content":"aG9vawo="}))
            .expect("contents JSON");
        let response = self
            .instance
            .exchange(&Outgoing::json("POST", &path, Some(self.person.as_ref().expect("fixture person")), &body))
            .expect("contents answer");
        assert_eq!(response.status, 201, "contents commit");
    }
}

pub fn configuration(directory: &Path, forge: &Forge<'_>, value: &mut Value) {
    let Forge::Throwaway { api, forgejo, .. } = forge else { return };
    let mut owned = owned::Owned::beside(*api);
    owned.forgejo_binary = forgejo.into();
    let path = directory.join("temper.json");
    if let Ok(bytes) = fs::read(&path) {
        let previous: Value = serde_json::from_slice(&bytes).expect("previous configuration");
        for (name, listener) in [("clients", &mut owned.clients), ("webhooks", &mut owned.webhooks)] {
            if let Some(address) = previous["application"][name]["listener"]["socket"].as_str() {
                *listener = address.parse().expect("listener address");
            }
        }
    }
    let limits = value["application"]["limits"].take();
    value["application"] = owned::application(&owned);
    owned::old_form(&mut value["application"]);
    value["application"]["limits"] = limits;
    value["application"]["limits"]["overrides"]["root.brief.gather_ms"] = BRIEF_GATHER_MS.into();
    let path = directory.join("agent.json");
    let mut agent: Value = serde_json::from_slice(&fs::read(&path).expect("agent configuration")).expect("agent JSON");
    landing::configure(value, &mut agent, *api);
    super::configuration::turn_bounds::apply(value, super::configuration::turn_bounds::turn_body(&agent));
    write_private(&path, &serde_json::to_vec(&agent).expect("agent JSON"));
}

#[must_use]
pub fn landing_scripts(answer: &[u8], forgejo: &Throwaway) -> Box<[skein_fake_llm_domain::api::Script]> {
    let Forge::Throwaway { api, .. } = forgejo.forge() else { panic!("throwaway forge") };
    let mut scripts = landing::scripts(api, forgejo.repository(), &forgejo.main_readme()).into_vec();
    let mut initial = super::chat_script(answer);
    initial.cue = b"Hello".as_slice().into();
    scripts.push(initial);
    scripts.into_boxed_slice()
}

fn stage(name: &str, began: skein_lib::Time) {
    eprintln!(
        "stage {name}: {} ms",
        skein_shell::Clock::new().now().now.saturating_since(began).as_nanos() / 1_000_000
    );
}
fn request(instance: &Instance, token: &Token, method: &str, path: &str, body: &Value, status: u16) -> Value {
    let body = serde_json::to_vec(body).expect("request JSON");
    let response = instance.exchange(&Outgoing::json(method, path, Some(token), &body)).expect("Forgejo answer");
    assert_eq!(response.status, status, "{method} {path}");
    serde_json::from_slice(&response.body).expect("answer JSON")
}
fn person_token(instance: &Instance) -> Token {
    let began = skein_shell::Clock::new().now().now;
    let admin = &instance.admin().token;
    request(
        instance,
        admin,
        "PATCH",
        "/api/v1/admin/users/ada",
        &json!({"login_name":"ada","source_id":0,"must_change_password":false}),
        200,
    );
    let value = request(
        instance,
        admin,
        "POST",
        "/api/v1/admin/users/ada/tokens",
        &json!({"name":"person","scopes":["write:repository","write:organization"]}),
        201,
    );
    stage("person", began);
    Token::new(value["sha1"].as_str().expect("person token").into())
}
fn repository(instance: &Instance, person: &Token, sleep_seconds: u64) -> u64 {
    let began = skein_shell::Clock::new().now().now;
    request(instance, person, "POST", "/api/v1/orgs", &json!({"username":"acme","visibility":"private"}), 201);
    let value = request(
        instance,
        person,
        "POST",
        "/api/v1/orgs/acme/repos",
        &json!({"name":"widget","auto_init":true,"default_branch":"main","private":true}),
        201,
    );
    request(instance, person, "PATCH", "/api/v1/repos/acme/widget", &json!({"has_actions":true}), 200);
    // The hinted job sleeps 5 s, outlasting its overridden 3 s settle window
    // (protocol/setup.md 13.4).
    let check = format!("test ! -e slow || sleep {sleep_seconds}\ntest -s README.md\n");
    for (path, content) in [("check.sh", check.as_str()), (".forgejo/workflows/check.yaml", WORKFLOW)] {
        let length = skein_lib::base64::encoded_len(
            skein_lib::base64::Alphabet::Standard,
            u32::try_from(content.len()).expect("workflow length"),
        )
        .expect("base64 length");
        let mut encoded = skein_lib::Writer::new(usize::try_from(length).expect("bounded length"));
        skein_lib::base64::encode(skein_lib::base64::Alphabet::Standard, content.as_bytes(), &mut encoded)
            .expect("bounded workflow");
        let encoded = encoded.finish();
        let content = std::str::from_utf8(&encoded).expect("base64 text");
        request(
            instance,
            person,
            "POST",
            &format!("/api/v1/repos/acme/widget/contents/{path}"),
            &json!({"branch":"main","message":"Add check","content":content}),
            201,
        );
    }
    stage("repository", began);
    value["id"].as_u64().expect("repository id")
}
const WORKFLOW: &str = r#"on: { push: { branches: ['temper/**'] } }
jobs:
  check:
    runs-on: temper-host
    steps:
      - run: |
          auth=$(printf 'x-access-token:%s' "${{ github.token }}" | base64 -w0)
          git -c "http.extraHeader=Authorization: basic $auth" clone "${{ github.server_url }}/${{ github.repository }}.git" .
          git checkout "${{ github.sha }}" && sh check.sh
"#;

// E4 observes a distinct managed deployment beside this owned fixture.
impl Throwaway {
    /// Mint a disposable admin paste, keeping its bytes out of argv and the environment.
    pub fn admin_token(&self, path: &Path) {
        let output = std::process::Command::new(&self.forgejo)
            .args([
                "--work-path",
                self.directory.join("forge-1/forgejo").to_str().expect("work path"),
                "--config",
                self.directory.join("forge-1/forgejo/app.ini").to_str().expect("config path"),
                "admin",
                "user",
                "generate-access-token",
                "--username",
                "temper-admin",
                "--token-name",
                "elsewhere-disposable",
                "--raw",
                "--scopes",
                "write:admin,read:user,write:organization,write:repository",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("TZ", "UTC")
            .env("HOME", self.directory.join("forge-1/home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.directory.join("forge-1/home/gitconfig"))
            .current_dir(self.directory.join("forge-1/forgejo"))
            .output()
            .expect("mint disposable admin token");
        assert!(output.status.success(), "disposable token command failed (output redacted)");
        let token = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        assert!(!token.is_empty(), "nonempty disposable token");
        assert!(!output.stderr.windows(token.len()).any(|part| part == token), "token in CLI stderr (redacted)");
        write_private(path, token);
    }

    /// The fixture's original admin credential survives Reconcile, never `TakeOver`.
    pub fn witness_authenticates(&self) {
        let response = self
            .instance
            .exchange(&Outgoing::get("/api/v1/user", Some(&self.instance.admin().token)))
            .expect("witness API answer");
        assert_eq!(response.status, 200, "original admin still authenticates");
    }

    #[must_use]
    pub fn repository_observation(&self, org: &str, name: &str, suffix: &str) -> Value {
        let response = self
            .instance
            .exchange(&Outgoing::get(
                &format!("/api/v1/repos/{org}/{name}/{suffix}"),
                Some(&self.instance.admin().token),
            ))
            .expect("elsewhere repository answer");
        assert_eq!(response.status, 200, "elsewhere observation");
        serde_json::from_slice(&response.body).expect("elsewhere JSON")
    }

    #[must_use]
    pub fn merged_pull_in(&self, org: &str, name: &str) -> Option<u64> {
        self.repository_observation(org, name, "pulls?state=closed&limit=50")
            .as_array()
            .expect("pull list")
            .iter()
            .find_map(|pull| (pull["merged"] == true).then(|| pull["number"].as_u64().expect("merged pull number")))
    }

    #[must_use]
    pub fn job_state_in(&self, org: &str, name: &str, pull: u64) -> String {
        let pull = self.repository_observation(org, name, &format!("pulls/{pull}"));
        let head = pull["head"]["sha"].as_str().expect("pull head");
        self.repository_observation(org, name, &format!("commits/{head}/status"))["state"]
            .as_str()
            .expect("job state")
            .into()
    }

    #[must_use]
    pub fn main_readme_in(&self, org: &str, name: &str) -> Vec<u8> {
        let response = self
            .instance
            .exchange(&Outgoing::get(
                &format!("/api/v1/repos/{org}/{name}/raw/README.md?ref=main"),
                Some(&self.instance.admin().token),
            ))
            .expect("elsewhere README answer");
        assert_eq!(response.status, 200, "elsewhere main README");
        response.body
    }

    #[must_use]
    pub fn repositories_in(&self, org: &str) -> Value {
        let response = self
            .instance
            .exchange(&Outgoing::get(&format!("/api/v1/orgs/{org}/repos?limit=50"), Some(&self.instance.admin().token)))
            .expect("elsewhere repositories answer");
        assert_eq!(response.status, 200, "elsewhere repository list");
        serde_json::from_slice(&response.body).expect("repositories JSON")
    }
}
