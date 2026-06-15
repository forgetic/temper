use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::DAEMON_BOOT_TIMEOUT;

pub(super) fn assert_daemon_boots(
    config_path: &Path,
    credentials_path: &Path,
    scratch_dir: &Path,
    bind_port: u16,
) {
    let log_path = scratch_dir.join("engine-boot.log");
    let log = std::fs::File::create(&log_path).expect("create engine boot log");
    let mut child = Command::new(env!("CARGO_BIN_EXE_temper"))
        .arg("daemon")
        .arg("--service")
        .arg("engine")
        .arg("--config")
        .arg(config_path)
        .arg("--credentials")
        .arg(credentials_path)
        .env_remove("FORGEJO_URL")
        .env_remove("FORGEJO_ACCESS_TOKEN")
        .env_remove("FORGEJO_DEFAULT_REPO")
        .env_remove("TEMPER_FORGE_URL")
        .env_remove("TEMPER_FORGE_TOKEN")
        .stdout(Stdio::from(log.try_clone().expect("clone log handle")))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("standalone engine spawns");

    let deadline = Instant::now() + DAEMON_BOOT_TIMEOUT;
    let bound = loop {
        if std::net::TcpStream::connect(("127.0.0.1", bind_port)).is_ok() {
            break true;
        }
        if let Some(status) = child.try_wait().expect("engine try_wait") {
            let _ = std::io::stderr().write_all(&read_log(&log_path));
            panic!(
                "engine exited during boot with {status:?}\n--- engine boot log ---\n{}",
                String::from_utf8_lossy(&read_log(&log_path))
            );
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        bound,
        "engine did not bind 127.0.0.1:{bind_port} within {DAEMON_BOOT_TIMEOUT:?}\n--- engine boot log ---\n{}",
        String::from_utf8_lossy(&read_log(&log_path))
    );
}

pub(super) fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral port binds")
        .local_addr()
        .expect("bound listener has an address")
        .port()
}

fn read_log(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}
