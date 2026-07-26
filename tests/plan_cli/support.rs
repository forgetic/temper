use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct RequestRecord {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
}

pub struct RecordingForge {
    base_url: String,
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RequestRecord>>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl RecordingForge {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake forge");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_shutdown = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { break };
                handle_connection(stream, &thread_requests);
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

impl Drop for RecordingForge {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(mut stream: TcpStream, requests: &Arc<Mutex<Vec<RequestRecord>>>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let mut raw = Vec::new();
    let mut buf = [0_u8; 1024];
    while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
        let Ok(read) = stream.read(&mut buf) else {
            return;
        };
        if read == 0 || raw.len() > 16 * 1024 {
            return;
        }
        raw.extend_from_slice(&buf[..read]);
    }
    let text = String::from_utf8_lossy(&raw);
    let mut lines = text.split("\r\n");
    let mut request = lines.next().unwrap_or_default().split_whitespace();
    let method = request.next().unwrap_or_default().to_string();
    let path = request.next().unwrap_or_default().to_string();
    let authorization = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim().to_string())
    });
    requests.lock().expect("requests lock").push(RequestRecord {
        method,
        path: path.clone(),
        authorization,
    });

    let route = path.split('?').next().unwrap_or(&path);
    let body = if let Some(login) = route.strip_prefix("/api/v1/users/") {
        format!(r#"{{"login":"{login}","email":"{login}@example.invalid"}}"#)
    } else if route.ends_with("/labels")
        || route.ends_with("/hooks")
        || route.ends_with("/issues")
        || route.ends_with("/pulls")
    {
        "[]".to_string()
    } else if let Some(repo) = route.strip_prefix("/api/v1/repos/") {
        let mut parts = repo.split('/');
        let owner = parts.next().unwrap_or("acme");
        let name = parts.next().unwrap_or("repo");
        format!(
            r#"{{"owner":{{"login":"{owner}"}},"name":"{name}","full_name":"{owner}/{name}","default_branch":"main","created_at":"1970-01-01T00:00:00Z","updated_at":"1970-01-01T00:00:00Z","has_actions":true}}"#
        )
    } else {
        "[]".to_string()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).expect("response");
}

#[derive(Debug, Eq, PartialEq)]
pub enum SnapshotEntry {
    Directory,
    File(Vec<u8>),
}

pub fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, SnapshotEntry> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, SnapshotEntry>) {
        let mut children = std::fs::read_dir(path)
            .expect("read snapshot directory")
            .map(|entry| entry.expect("directory entry").path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            let relative = child
                .strip_prefix(root)
                .expect("relative path")
                .to_path_buf();
            if child.is_dir() {
                entries.insert(relative, SnapshotEntry::Directory);
                visit(root, &child, entries);
            } else {
                entries.insert(
                    relative,
                    SnapshotEntry::File(std::fs::read(&child).expect("file")),
                );
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

pub fn write_bundle(
    root: &Path,
    forge_url: &str,
    repos: &[&str],
    auth: &str,
    workflow_file: Option<&str>,
) -> PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle");
    std::fs::write(bundle.join("webhook-secret"), "webhook-secret-value").expect("webhook");
    let repos = repos
        .iter()
        .map(|repo| format!("\"{repo}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let workflow = workflow_file
        .map(|file| format!("[workflow]\nfile = \"{file}\"\n"))
        .unwrap_or_default();
    std::fs::write(
        bundle.join("config.toml"),
        format!(
            "schema_version = 1\n[deployment]\nname = \"local-dev\"\ntopology = \"standalone\"\n{workflow}[forge]\nurl = \"{forge_url}\"\nadmin = \"root\"\n[engine]\nbind = \"127.0.0.1:38100\"\nrepos = [{repos}]\nroles = [\"architect\", \"engineer\"]\nwebhook_secret_file = \"webhook-secret\"\n"
        ),
    )
    .expect("config");
    std::fs::write(
        bundle.join("credentials.toml"),
        format!(
            "schema_version = 1\n[forge.users.root]\npassword = \"admin-pass\"\n{auth}[agent.providers.deepseek]\ntype = \"api-key\"\nkey = \"provider-key\"\n"
        ),
    )
    .expect("credentials");
    bundle
}
