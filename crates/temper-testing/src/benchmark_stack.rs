//! Private, stdin-owned Forgejo/runner process fixture for live benchmarks.

use crate::forgejo_server::{ForgejoRunner, download, start_cached_bare_admin_server};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Boots the pinned local fixture, then keeps it alive until stdin closes.
///
/// Bootstrap credentials are written only to a private file. The caller must
/// close stdin and wait for this process, allowing fixture destructors to run.
/// The retained log files contain local diagnostic evidence, not credentials.
pub fn run(output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    restrict_directory(output)?;
    let password = random_password()?;
    let cached = start_cached_bare_admin_server(
        "benchmark-admin",
        &password,
        "benchmark-admin@example.invalid",
    )?;
    let server = cached.server;
    let mut runner = ForgejoRunner::register(&server)?;
    if !runner.is_running() {
        return Err(format!("benchmark runner exited: {}", runner.log_tail()).into());
    }
    let token = server.run_cli(&[
        "admin",
        "user",
        "generate-access-token",
        "--username",
        "benchmark-admin",
        "--scopes",
        "all",
        "--raw",
    ])?;
    let bootstrap = output.join("bootstrap.json");
    write_private(
        &bootstrap,
        &serde_json::to_vec_pretty(&json!({
            "base_url": server.base_url(),
            "admin_user": "benchmark-admin",
            "admin_password": password,
            "admin_token": token.trim(),
            "forgejo_version": download::FORGEJO_VERSION,
            "runner_version": download::FORGEJO_RUNNER_VERSION,
            "cache_hit": cached.cache_hit,
            "cache_key": cached.cache_key,
            "server_data_dir": server.data_dir(),
            "runner_work_dir": runner.work_dir(),
        }))?,
    )?;
    println!("{}", json!({"ready": true, "bootstrap_file": bootstrap}));
    std::io::stdout().flush()?;

    let wait = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
    let server_log = fs::copy(
        server.data_dir().join("web.log"),
        output.join("forgejo.log"),
    );
    let runner_log = fs::copy(
        runner.work_dir().join("daemon.log"),
        output.join("runner.log"),
    );
    drop(runner);
    drop(server);
    fs::remove_file(bootstrap)?;
    wait?;
    server_log?;
    runner_log?;
    Ok(())
}

fn random_password() -> Result<String> {
    use std::fmt::Write as _;

    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).map_err(|error| format!("random password: {error}"))?;
    let mut password = String::from("Benchmark-A1!");
    for byte in bytes {
        write!(password, "{byte:02x}")?;
    }
    Ok(password)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn restrict_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
