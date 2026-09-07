use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use temper_process_containment::{
    CleanupReport, CleanupTrigger, ContainedProcess, ContainmentCommand, ContainmentFactory,
    ContainmentIdentity, ContainmentScope, ContainmentSpec,
};

use crate::config::{BootstrapReply, HELPER_MODE, MAX_CONTROL_BYTES};
use crate::{LaunchConfig, ProviderBootstrapFailure};

type CompletionState = Arc<(Mutex<Option<CleanupReport>>, Condvar)>;

/// Observation of the shared owner's natural completion, independent of a
/// particular job's short-lived bootstrap admission handle.
#[derive(Clone)]
pub struct ProviderCompletion(CompletionState);

impl ProviderCompletion {
    pub fn wait(&self, timeout: Duration) -> Option<CleanupReport> {
        let (state, changed) = &*self.0;
        let state = state.lock().unwrap_or_else(|error| error.into_inner());
        let (state, _) = changed
            .wait_timeout_while(state, timeout, |state| state.is_none())
            .unwrap_or_else(|error| error.into_inner());
        state.clone()
    }
}

enum AdmissionState {
    Pending(UnixStream),
    Expired(UnixStream),
    Released,
}

struct Admission {
    control: Mutex<AdmissionState>,
    deadline: Instant,
}

/// Releasing or dropping this handle closes only the temporary bootstrap
/// session. The separately registered owner retains shared descendants until
/// upstream naturally ends the generation, including on startup-failure paths.
#[derive(Clone)]
pub struct ProviderBootstrap {
    admission: Arc<Admission>,
    completion: ProviderCompletion,
}

impl ProviderBootstrap {
    /// Must be called at the worker/composition boundary, outside every job's
    /// containment and emergency registry. Only operator-resolved configuration
    /// is accepted; the agent protocol can release admission but cannot launch.
    pub fn start(
        manager: &ProviderOwnerManager,
        config: LaunchConfig,
        factory: &ContainmentFactory,
        helper_executable: &Path,
    ) -> Result<Self, ProviderBootstrapFailure> {
        config.validate()?;
        if !cfg!(target_os = "linux") {
            return Err(ProviderBootstrapFailure::UnsupportedPlatform);
        }
        let mut bytes =
            serde_json::to_vec(&config).map_err(|_| ProviderBootstrapFailure::Configuration)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_CONTROL_BYTES {
            return Err(ProviderBootstrapFailure::Configuration);
        }
        let (stream, helper_stream) =
            UnixStream::pair().map_err(|_| ProviderBootstrapFailure::Transport)?;
        let token = uuid::Uuid::new_v4().to_string();
        let spec = ContainmentSpec::new(
            ContainmentIdentity::new(format!("cbm-shared-{token}"))
                .map_err(|_| ProviderBootstrapFailure::Configuration)?,
            ContainmentScope::Custom("shared_codebase_memory".into()),
        );
        let mut command = ContainmentCommand::new(helper_executable);
        command
            .arg(HELPER_MODE)
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(helper_stream)))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let process = factory
            .prepare(spec)
            .map_err(|_| ProviderBootstrapFailure::Spawn)?
            .spawn(command)
            .map_err(|_| ProviderBootstrapFailure::Spawn)?;
        // Register BEFORE the helper receives the executable configuration.
        let completion = manager.observe(process)?;
        stream
            .set_read_timeout(Some(config.startup_timeout))
            .map_err(|_| ProviderBootstrapFailure::Transport)?;
        stream
            .set_write_timeout(Some(config.startup_timeout))
            .map_err(|_| ProviderBootstrapFailure::Transport)?;
        let mut reader = BufReader::new(stream);
        reader
            .get_mut()
            .write_all(&bytes)
            .map_err(|_| ProviderBootstrapFailure::Transport)?;
        let reply: BootstrapReply = serde_json::from_slice(&read_control(&mut reader)?)
            .map_err(|_| ProviderBootstrapFailure::ProviderContract)?;
        match reply {
            BootstrapReply::Admitted => {
                observe("admitted");
                Ok(Self {
                    admission: Arc::new(Admission {
                        control: Mutex::new(AdmissionState::Pending(reader.into_inner())),
                        deadline: Instant::now() + config.admission_timeout,
                    }),
                    completion,
                })
            }
            BootstrapReply::Failed { category } => Err(category),
        }
    }

    pub fn serving_admitted(&self) {
        let mut state = self
            .admission
            .control
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !matches!(*state, AdmissionState::Pending(_)) {
            return;
        }
        if let AdmissionState::Pending(mut stream) =
            std::mem::replace(&mut *state, AdmissionState::Released)
        {
            let _ = stream.write_all(b"R");
            let _ = stream.shutdown(Shutdown::Both);
            observe("released");
        }
    }

    pub fn admission_remaining(&self) -> Duration {
        self.admission
            .deadline
            .saturating_duration_since(Instant::now())
    }

    pub fn admission_pending(&self) -> bool {
        !self.admission_remaining().is_zero()
            && matches!(
                *self
                    .admission
                    .control
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
                AdmissionState::Pending(_)
            )
    }

    /// Atomically fence a missing serving admission. The parent MUST cancel
    /// and join its job before dropping this handle; a late serving event can
    /// no longer release the bootstrap while that job can still cold-activate.
    pub fn expire_admission(&self) -> bool {
        let mut state = self
            .admission
            .control
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if matches!(*state, AdmissionState::Pending(_)) {
            if let AdmissionState::Pending(stream) =
                std::mem::replace(&mut *state, AdmissionState::Released)
            {
                *state = AdmissionState::Expired(stream);
            }
            observe("admission_expired");
            true
        } else {
            match &*state {
                AdmissionState::Expired(stream) => {
                    let _ = stream.peer_addr();
                    true
                }
                _ => false,
            }
        }
    }

    pub fn completion(&self) -> ProviderCompletion {
        self.completion.clone()
    }
}

/// Shared worker/composition owner registry. Individual jobs hold admission
/// only. Completed monitor threads are joined at the next admission or audit.
#[derive(Clone, Default)]
pub struct ProviderOwnerManager {
    owners: Arc<Mutex<Vec<(ProviderCompletion, thread::JoinHandle<()>)>>>,
}

impl ProviderOwnerManager {
    pub fn reap_completed(&self) {
        let mut owners = self
            .owners
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut index = 0;
        while index < owners.len() {
            if owners[index].1.is_finished() {
                let (_, owner) = owners.swap_remove(index);
                let _ = owner.join();
            } else {
                index += 1;
            }
        }
    }

    pub fn completions(&self) -> Vec<ProviderCompletion> {
        self.owners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|(completion, _)| completion.clone())
            .collect()
    }

    fn observe(
        &self,
        process: ContainedProcess,
    ) -> Result<ProviderCompletion, ProviderBootstrapFailure> {
        self.reap_completed();
        let completion: CompletionState = Arc::new((Mutex::new(None), Condvar::new()));
        let result = Arc::clone(&completion);
        let owner = thread::Builder::new()
            .name("cbm-shared-owner".into())
            .spawn(move || {
                observe("started");
                while matches!(process.try_wait_root(), Ok(None)) {
                    thread::sleep(Duration::from_millis(20));
                }
                let report = process.cleanup(CleanupTrigger::NormalRootExit);
                *result.0.lock().unwrap_or_else(|error| error.into_inner()) = Some(report);
                result.1.notify_all();
                observe("completed");
            })
            .map_err(|_| ProviderBootstrapFailure::Spawn)?;
        let completion = ProviderCompletion(completion);
        self.owners
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((completion.clone(), owner));
        Ok(completion)
    }
}

fn read_control(reader: &mut BufReader<UnixStream>) -> Result<Vec<u8>, ProviderBootstrapFailure> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(257)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) {
                ProviderBootstrapFailure::Timeout
            } else {
                ProviderBootstrapFailure::Transport
            }
        })?;
    if bytes.len() > 256 || bytes.pop() != Some(b'\n') {
        return Err(ProviderBootstrapFailure::ProviderContract);
    }
    Ok(bytes)
}

fn observe(stage: &'static str) {
    tracing::debug!(target: "temper::codebase_memory", event = "codebase_memory.shared_owner", lifecycle.stage = stage,
        "shared provider ownership transition");
}
