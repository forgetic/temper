// SPDX-License-Identifier: MPL-2.0

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

use crate::support::{WORKFLOW_JSON, temper};

#[derive(Debug, Clone)]
struct RequestRecord {
    path: String,
    authorization: Option<String>,
}

type Handler = dyn Fn(&RequestRecord) -> (u16, String) + Send + Sync;

struct FakeForge {
    base_url: String,
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RequestRecord>>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeForge {
    fn start(handler: impl Fn(&RequestRecord) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake forge");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let thread_requests = Arc::clone(&requests);
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else {
                    break;
                };
                handle_connection(stream, &thread_requests, &handler);
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            addr,
            requests,
            shutdown,
            handle: Some(handle),
        }
    }

    fn requests(&self) -> Vec<RequestRecord> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl Drop for FakeForge {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    requests: &Arc<Mutex<Vec<RequestRecord>>>,
    handler: &Arc<Handler>,
) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let mut raw = Vec::new();
    let mut buf = [0_u8; 1024];
    while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
        let Ok(read) = stream.read(&mut buf) else {
            return;
        };
        if read == 0 {
            return;
        }
        raw.extend_from_slice(&buf[..read]);
        if raw.len() > 16 * 1024 {
            return;
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let authorization = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim().to_string())
    });
    let record = RequestRecord {
        path,
        authorization,
    };
    requests.lock().expect("requests lock").push(record.clone());
    let (status, body) = handler(&record);
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn write_engine_bundle(root: &std::path::Path, forge_url: &str) -> std::path::PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(bundle.join("state")).expect("create state");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace");
    std::fs::write(
        bundle.join("config.toml"),
        format!(
            "schema_version = 1\n\
             [deployment]\n\
             name = \"local-dev\"\n\
             topology = \"standalone\"\n\
             [workflow]\n\
             file = \"workflow.json\"\n\
             [paths]\n\
             state_dir = \"state\"\n\
             workspace_dir = \"workspace\"\n\
             [forge]\n\
             url = \"{forge_url}\"\n\
             admin = \"engineer\"\n\
             ci_user = \"engineer\"\n\
             [engine]\n\
             repos = [\"ai/temper\"]\n\
             roles = [\"engineer\"]\n"
        ),
    )
    .expect("write config");
    std::fs::write(bundle.join("workflow.json"), WORKFLOW_JSON).expect("write workflow");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"forge-token\"\n\
         password = \"forge-password\"\n\
         [agent.providers.anthropic]\n\
         type = \"api-key\"\n\
         key = \"provider-key\"\n",
    )
    .expect("write credentials");
    bundle
}

#[test]
fn online_forge_success_checks_user_and_repos() {
    let forge = FakeForge::start(|request| {
        if request.authorization.as_deref() != Some("token forge-token") {
            return (401, "{}".to_string());
        }
        match request.path.as_str() {
            "/api/v1/user" => (200, r#"{"login":"engineer"}"#.to_string()),
            "/api/v1/repos/ai/temper" => (200, r#"{"full_name":"ai/temper"}"#.to_string()),
            _ => (404, "{}".to_string()),
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_engine_bundle(dir.path(), &forge.base_url);
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--online",
        ],
        dir.path(),
    );

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["status"], "ok");
    assert_eq!(value["online"], true);
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("online checks are not implemented"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let paths = forge
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect::<Vec<_>>();
    assert!(paths.contains(&"/api/v1/user".to_string()), "{paths:?}");
    assert!(
        paths.contains(&"/api/v1/repos/ai/temper".to_string()),
        "{paths:?}"
    );
}

#[test]
fn online_forge_auth_failure_is_distinct_and_redacted() {
    let forge = FakeForge::start(|request| match request.path.as_str() {
        "/api/v1/user" => (401, "{}".to_string()),
        _ => (404, "{}".to_string()),
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_engine_bundle(dir.path(), &forge.base_url);
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--online",
        ],
        dir.path(),
    );

    assert!(!output.status.success(), "auth failure should fail");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(!stdout.contains("forge-token"), "{stdout}");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["check"] == "online"
            && finding["category"] == "auth"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("authentication failed"))),
        "{value}"
    );
}

#[test]
fn online_forge_reports_missing_repo_visibility() {
    let forge = FakeForge::start(|request| {
        if request.authorization.as_deref() != Some("token forge-token") {
            return (401, "{}".to_string());
        }
        match request.path.as_str() {
            "/api/v1/user" => (200, "{}".to_string()),
            "/api/v1/repos/ai/temper" => (404, "{}".to_string()),
            _ => (404, "{}".to_string()),
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_engine_bundle(dir.path(), &forge.base_url);
    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--online",
        ],
        dir.path(),
    );

    assert!(!output.status.success(), "missing repo should fail");
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["check"] == "online"
            && finding["category"] == "repo"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("ai/temper"))),
        "{value}"
    );
}

#[test]
fn worker_online_uses_role_token_without_engine_credentials() {
    let forge = FakeForge::start(|request| {
        if request.authorization.as_deref() != Some("token role-token") {
            return (401, "{}".to_string());
        }
        match request.path.as_str() {
            "/api/v1/user" | "/api/v1/repos/ai/temper" => (200, "{}".to_string()),
            _ => (404, "{}".to_string()),
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace");
    std::fs::write(
        bundle.join("config.toml"),
        format!(
            "schema_version = 1\n\
             [paths]\n\
             workspace_dir = \"workspace\"\n\
             [forge]\n\
             url = \"{}\"\n\
             [[worker.pools]]\n\
             name = \"engineers\"\n\
             roles = [\"engineer\"]\n\
             repos = [\"ai/temper\"]\n\
             agent_profile = \"coding\"\n\
             [agent.profiles.coding]\n\
             provider = \"anthropic\"\n\
             provider_url = \"https://provider.example\"\n\
             credential = \"profile-secret\"\n",
            forge.base_url
        ),
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"role-token\"\n\
         [secrets]\n\
         profile-secret = \"provider-secret\"\n",
    )
    .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--component",
            "worker",
            "--pool",
            "engineers",
            "--online",
        ],
        dir.path(),
    );

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["status"], "ok");
    assert!(
        value["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .all(|finding| !finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("engine.forge_token"))),
        "{value}"
    );
}

#[test]
fn provider_profile_online_validation_redacts_secret_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("bundle");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [paths]\n\
         workspace_dir = \"workspace\"\n\
         [worker]\n\
         git_base_url = \"https://git.example\"\n\
         [[worker.pools]]\n\
         name = \"engineers\"\n\
         roles = [\"engineer\"]\n\
         repos = [\"ai/temper\"]\n\
         agent_profile = \"coding\"\n\
         [agent.profiles.coding]\n\
         provider = \"deepseek\"\n\
         provider_url = \"ftp://provider.example\"\n\
         credential = \"profile-secret\"\n",
    )
    .expect("write config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"role-token\"\n\
         [secrets]\n\
         profile-secret = \"super-secret-value\"\n",
    )
    .expect("write credentials");

    let bundle_arg = bundle.to_string_lossy();
    let output = temper(
        &[
            "--config",
            &bundle_arg,
            "--format",
            "json",
            "check",
            "--component",
            "worker",
            "--pool",
            "engineers",
            "--online",
        ],
        dir.path(),
    );

    assert!(!output.status.success(), "invalid provider URL should fail");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(!stdout.contains("super-secret-value"), "{stdout}");
    let value: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = value["findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|finding| finding["check"] == "online"
            && finding["category"] == "provider"
            && finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("provider URL"))),
        "{value}"
    );
}
