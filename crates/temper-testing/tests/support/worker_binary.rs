use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub fn temper_testing_worker() -> &'static Path {
    static WORKER: OnceLock<PathBuf> = OnceLock::new();
    WORKER.get_or_init(resolve_worker).as_path()
}

fn resolve_worker() -> PathBuf {
    if let Some(path) = std::env::var_os("TEMPER_TESTING_WORKER_BIN") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_temper-testing-worker") {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_temper-testing-worker") {
        return PathBuf::from(path);
    }
    build_root_worker()
}

fn build_root_worker() -> PathBuf {
    let workspace_root = workspace_root();
    let profile = option_env!("PROFILE").unwrap_or("debug");
    let mut command = Command::new(cargo());
    command
        .current_dir(&workspace_root)
        .arg("build")
        .arg("-p")
        .arg("temper")
        .arg("--bin")
        .arg("temper-testing-worker")
        .arg("--features")
        .arg("testing-worker");
    if profile != "debug" {
        command.arg("--profile").arg(profile);
    }
    let status = command
        .status()
        .expect("cargo can be spawned to build temper-testing-worker");
    assert!(
        status.success(),
        "cargo build for temper-testing-worker failed"
    );

    let mut file_name = String::from("temper-testing-worker");
    file_name.push_str(std::env::consts::EXE_SUFFIX);
    let path = target_dir(&workspace_root).join(profile).join(file_name);
    assert!(
        path.is_file(),
        "cargo build succeeded but temper-testing-worker was not found at {}",
        path.display()
    );
    path
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/ in the workspace")
        .to_path_buf()
}

fn target_dir(workspace_root: &Path) -> PathBuf {
    let path = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target"));
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| option_env!("CARGO").unwrap_or("cargo").into())
}
