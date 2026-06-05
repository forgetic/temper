use std::path::PathBuf;
use std::time::Instant;

pub(crate) struct StopSignal {
    stop_file: Option<PathBuf>,
    started: Instant,
    run_secs: Option<u64>,
}

impl StopSignal {
    pub(crate) fn new(stop_file: Option<PathBuf>, run_secs: Option<u64>) -> Self {
        Self {
            stop_file,
            started: Instant::now(),
            run_secs,
        }
    }

    pub(crate) fn should_stop(&self) -> bool {
        self.stop_file.as_ref().is_some_and(|path| path.exists())
            || self
                .run_secs
                .is_some_and(|seconds| self.started.elapsed().as_secs() >= seconds)
    }
}
