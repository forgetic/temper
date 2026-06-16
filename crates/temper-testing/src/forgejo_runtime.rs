//! Test-only runtime resources for local Forgejo integration tests.
//!
//! Ignored Forgejo tests run concurrently under libtest. This module gives each
//! test a unique workspace for mutable files and starts webhook triggers from an
//! already-bound listener so there is no free-port handoff gap.

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use temper_trigger_forgejo::trigger_args::TriggerArgs;

static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory owned by one test run.
///
/// The name includes the process id plus an atomic counter and is created with
/// `create_dir`, so parallel calls cannot receive the same path. The directory
/// is removed recursively on drop.
pub struct RunWorkspace {
    path: PathBuf,
}

impl RunWorkspace {
    /// Creates a new workspace under the OS temp directory.
    pub fn new(prefix: impl AsRef<str>) -> Self {
        let prefix = safe_component(prefix.as_ref());
        for _ in 0..1024 {
            let id = NEXT_WORKSPACE.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!("{prefix}-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self { path },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("creating run workspace {} failed: {error}", path.display()),
            }
        }
        panic!("could not allocate a unique run workspace for prefix {prefix}")
    }

    /// Returns the workspace root path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns a path inside the workspace without creating it.
    pub fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.path.join(relative)
    }

    /// Creates and returns a directory inside the workspace.
    pub fn dir(&self, relative: impl AsRef<Path>) -> PathBuf {
        let path = self.join(relative);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("creating {} failed: {error}", path.display()));
        path
    }

    /// Writes a file inside the workspace, creating parent directories.
    pub fn write_file(&self, relative: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("creating {} failed: {error}", parent.display()));
        }
        std::fs::write(&path, contents)
            .unwrap_or_else(|error| panic!("writing {} failed: {error}", path.display()));
        path
    }
}

impl Drop for RunWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A detached local webhook trigger started from an already-bound listener.
pub struct TriggerServer {
    addr: SocketAddr,
}

impl TriggerServer {
    /// Starts the trigger on `127.0.0.1:0`, returning the actual bound address.
    ///
    /// The listener is allocated before the thread is spawned and then moved
    /// into `run_with_listener`, so no other process can steal the selected port
    /// between address allocation and the serving loop.
    pub fn start(
        webhook_secret_file: PathBuf,
        wake_secret_file: Option<PathBuf>,
        wake_dir: PathBuf,
    ) -> Self {
        // Validate the files before spawning so an immediate typo does not look
        // like a listener that started successfully and then disappeared.
        std::fs::read_to_string(&webhook_secret_file).unwrap_or_else(|error| {
            panic!(
                "webhook secret file {} is not readable: {error}",
                webhook_secret_file.display()
            )
        });
        if let Some(path) = &wake_secret_file {
            std::fs::read_to_string(path).unwrap_or_else(|error| {
                panic!(
                    "wake secret file {} is not readable: {error}",
                    path.display()
                )
            });
        }
        std::fs::create_dir_all(&wake_dir)
            .unwrap_or_else(|error| panic!("creating {} failed: {error}", wake_dir.display()));

        let listener = TcpListener::bind("127.0.0.1:0").expect("trigger listener binds");
        let addr = listener
            .local_addr()
            .expect("trigger listener has an address");
        let args = TriggerArgs {
            bind: addr,
            webhook_secret_file,
            wake_secret_file,
            wake_dir: Some(wake_dir),
            wake_sockets: Vec::new(),
        };
        // `TcpListener::bind` has already put the socket into the listening
        // state. Moving that bound listener into the serving thread keeps the
        // reported address reserved and reachable without a free-port gap.
        std::thread::spawn(move || {
            if let Err(error) = temper_trigger_forgejo::trigger::run_with_listener(&args, listener)
            {
                tracing::warn!(%error, "Forgejo test trigger exited");
            }
        });
        Self { addr }
    }

    /// Returns the actual address the trigger listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns the Forgejo webhook URL for this trigger.
    pub fn webhook_url(&self) -> String {
        format!("http://{}/forgejo/webhook", self.addr)
    }
}

fn safe_component(raw: &str) -> String {
    let safe = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = safe.trim_matches('-');
    if trimmed.is_empty() {
        "temper-run".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, Barrier, Mutex};

    #[test]
    fn workspaces_are_unique_under_parallel_generation() {
        let threads = 8;
        let per_thread = 8;
        let barrier = Arc::new(Barrier::new(threads));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let handles = (0..threads)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let paths = Arc::clone(&paths);
                std::thread::spawn(move || {
                    barrier.wait();
                    let workspaces = (0..per_thread)
                        .map(|_| RunWorkspace::new("temper-forgejo-runtime-parallel"))
                        .collect::<Vec<_>>();
                    paths.lock().expect("paths lock").extend(
                        workspaces
                            .iter()
                            .map(|workspace| workspace.path().to_path_buf()),
                    );
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("workspace thread joins");
        }
        let paths = paths.lock().expect("paths lock");
        let unique = paths.iter().cloned().collect::<HashSet<_>>();
        assert_eq!(unique.len(), threads * per_thread);
    }

    #[test]
    fn trigger_server_reports_reachable_bound_addr() {
        let workspace = RunWorkspace::new("temper-forgejo-trigger-helper");
        let webhook_secret = workspace.write_file("secrets/webhook", "webhook-secret\n");
        let wake_secret = workspace.write_file("secrets/wake", "wake-secret\n");
        let wake_dir = workspace.dir("wake");

        let trigger = TriggerServer::start(webhook_secret, Some(wake_secret), wake_dir);
        assert_ne!(trigger.addr().port(), 0);

        let mut stream =
            TcpStream::connect(trigger.addr()).expect("reported trigger addr connects");
        stream
            .write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\ncontent-length: 0\r\n\r\n")
            .expect("request writes");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("response reads");
        assert!(
            response.starts_with("HTTP/1.1 401 Unauthorized"),
            "unexpected response: {response:?}"
        );
    }
}
