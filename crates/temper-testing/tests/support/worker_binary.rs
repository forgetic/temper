use std::path::{Path, PathBuf};
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
    panic!(
        "{}",
        concat!(
            "temper-testing-worker binary path is unavailable; run the multiprocess tests ",
            "with Cargo for the temper-testing package so ",
            "CARGO_BIN_EXE_temper-testing-worker is set, or set ",
            "TEMPER_TESTING_WORKER_BIN to an existing binary"
        )
    );
}
