// SPDX-License-Identifier: MPL-2.0

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const WORKFLOW_JSON: &str =
    include_str!("../../crates/temper-workflow/fixtures/reference-delivery.json");

pub fn temper(args: &[&str], env_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .output()
        .expect("run temper")
}

#[derive(Debug, Clone)]
pub struct RequestRecord {
    pub path: String,
    pub authorization: Option<String>,
}

type Handler = dyn Fn(&RequestRecord) -> (u16, String) + Send + Sync;

pub struct FakeForge {
    base_url: String,
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RequestRecord>>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeForge {
    pub fn start(
        handler: impl Fn(&RequestRecord) -> (u16, String) + Send + Sync + 'static,
    ) -> Self {
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

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn requests(&self) -> Vec<RequestRecord> {
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

pub fn write_online_engine_bundle(root: &Path, forge_url: &str) -> PathBuf {
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

pub fn write_valid_bundle(root: &Path) -> PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
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
         url = \"http://localhost:3000\"\n\
         admin = \"engineer\"\n\
         ci_user = \"engineer\"\n\
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("write config");
    std::fs::write(bundle.join("workflow.json"), WORKFLOW_JSON).expect("write workflow");
    std::fs::create_dir_all(bundle.join("state")).expect("create state dir");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace dir");
    write_valid_credentials(&bundle);
    bundle
}

pub fn write_valid_credentials(bundle: &Path) {
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
}
