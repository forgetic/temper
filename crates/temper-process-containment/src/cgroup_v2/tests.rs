use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Stdio;
use std::sync::Mutex;

use super::containment::PreparedControls;
use super::*;
use crate::{CleanupTrigger, ContainmentFactory, ContainmentIdentity};

struct FakeCgroupFs {
    root: PathBuf,
    kill: bool,
    fail_reads: Mutex<HashSet<PathBuf>>,
    fail_opens: Mutex<HashSet<PathBuf>>,
    fail_removes: Mutex<HashSet<PathBuf>>,
    event_reads: Mutex<HashMap<PathBuf, VecDeque<String>>>,
    removals: Mutex<Vec<PathBuf>>,
}

impl FakeCgroupFs {
    fn new(kill: bool) -> Arc<Self> {
        let root = std::env::temp_dir().join(format!(
            "temper-cgroup-v2-test-{}-{}",
            std::process::id(),
            TEST_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create fake cgroup root");
        let fake = Arc::new(Self {
            root,
            kill,
            fail_reads: Mutex::new(HashSet::new()),
            fail_opens: Mutex::new(HashSet::new()),
            fail_removes: Mutex::new(HashSet::new()),
            event_reads: Mutex::new(HashMap::new()),
            removals: Mutex::new(Vec::new()),
        });
        fake.make_controls(&fake.root);
        fake
    }

    fn make_controls(&self, path: &Path) {
        fs::write(path.join("cgroup.controllers"), "cpu memory\n").expect("controllers");
        fs::write(path.join("cgroup.procs"), "").expect("procs");
        fs::write(path.join("cgroup.events"), "populated 0\n").expect("events");
        if self.kill {
            fs::write(path.join("cgroup.kill"), "").expect("kill");
        }
    }

    fn set_members(&self, path: &Path, members: &[u32]) {
        let text = members
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path.join("cgroup.procs"), text).expect("set members");
        fs::write(
            path.join("cgroup.events"),
            format!("populated {}\n", usize::from(!members.is_empty())),
        )
        .expect("set events");
    }

    fn queue_events(&self, path: &Path, events: &[&str]) {
        self.event_reads.lock().expect("event reads").insert(
            path.to_path_buf(),
            events.iter().map(ToString::to_string).collect(),
        );
    }
}

impl Drop for FakeCgroupFs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl CgroupFileSystem for FakeCgroupFs {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn create_cgroup(&self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)?;
        self.make_controls(path);
        Ok(())
    }

    fn open_directory(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
    }

    fn open_read(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new().read(true).open(path)
    }

    fn open_write(&self, path: &Path) -> io::Result<File> {
        OpenOptions::new().write(true).open(path)
    }

    fn open_read_write(&self, path: &Path) -> io::Result<File> {
        if self.fail_opens.lock().expect("fail opens").contains(path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected open EACCES",
            ));
        }
        OpenOptions::new().read(true).write(true).open(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        if self.fail_reads.lock().expect("fail reads").contains(path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected EACCES",
            ));
        }
        fs::read_to_string(path)
    }

    fn read_events(&self, path: &Path, preopened: &mut File) -> io::Result<String> {
        if self.fail_reads.lock().expect("fail reads").contains(path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected EACCES",
            ));
        }
        let parent = path.parent().expect("events parent");
        let mut event_reads = self.event_reads.lock().expect("event reads");
        if let Some(queue) = event_reads.get_mut(parent) {
            if let Some(value) = queue.pop_front() {
                fs::write(path, &value)?;
                return Ok(value);
            }
        }
        read_preopened(preopened)
    }

    fn child_directories(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        RealCgroupFileSystem.child_directories(path)
    }

    fn remove_cgroup(&self, path: &Path) -> io::Result<()> {
        if self
            .fail_removes
            .lock()
            .expect("fail removes")
            .contains(path)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected remove EACCES",
            ));
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
            }
        }
        fs::remove_dir(path)?;
        self.removals
            .lock()
            .expect("removals")
            .push(path.to_path_buf());
        Ok(())
    }

    fn write_cgroup_kill(&self, root: &Path, control: Option<&mut File>) -> io::Result<()> {
        if !self.kill {
            return Err(io::Error::new(io::ErrorKind::NotFound, "cgroup.kill"));
        }
        if let Some(control) = control {
            control.write_all(b"1")?;
        }
        for path in descendant_directories(self, root)? {
            self.set_members(&path, &[]);
        }
        Ok(())
    }
}

#[derive(Default)]
struct FakeProcesses {
    identities: Mutex<HashMap<u32, ProcessIdentity>>,
    signaled: Mutex<Vec<(u32, ContainmentSignal)>>,
}

impl FakeProcesses {
    fn add(&self, pid: u32) {
        self.identities.lock().expect("identities").insert(
            pid,
            ProcessIdentity::new(pid, 1, pid, pid, u64::from(pid) * 7, "/fake/process"),
        );
    }
}

impl LinuxProcessApi for FakeProcesses {
    fn pidfd_supported(&self) -> bool {
        true
    }

    fn identity(&self, pid: u32) -> io::Result<ProcessIdentity> {
        self.identities
            .lock()
            .expect("identities")
            .get(&pid)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "gone"))
    }

    fn signal(
        &self,
        expected: &ProcessIdentity,
        signal: ContainmentSignal,
    ) -> io::Result<SignalAttemptOutcome> {
        self.signaled
            .lock()
            .expect("signals")
            .push((expected.pid(), signal));
        Ok(SignalAttemptOutcome::Succeeded)
    }
}

#[derive(Default)]
struct RecordingCapabilityObserver {
    capabilities: Mutex<Vec<CgroupV2Capability>>,
}

impl CgroupV2CapabilityObserver for RecordingCapabilityObserver {
    fn observe(&self, capability: &CgroupV2Capability) {
        self.capabilities
            .lock()
            .expect("capabilities")
            .push(capability.clone());
    }
}

#[derive(Default)]
struct RecordingFallback {
    policies: Mutex<Vec<ContainmentBackendPolicy>>,
}

impl ContainmentBackendFactory for RecordingFallback {
    fn prepare_backend(
        &self,
        policy: ContainmentBackendPolicy,
        _spec: &ContainmentSpec,
    ) -> io::Result<Box<dyn PreparedContainmentBackend>> {
        self.policies.lock().expect("policies").push(policy);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "recording fallback selected",
        ))
    }
}

static TEST_NONCE: AtomicU64 = AtomicU64::new(0);

fn fake_factory(
    kill: bool,
) -> (
    Arc<FakeCgroupFs>,
    Arc<FakeProcesses>,
    CgroupV2BackendFactory,
) {
    let fs = FakeCgroupFs::new(kill);
    let processes = Arc::new(FakeProcesses::default());
    let config = CgroupV2FactoryConfig::new("job-1", "attempt-2").expect("config");
    let capability = probe_delegated(
        &config,
        fs.as_ref(),
        Some(fs.root.clone()),
        fs.root.clone(),
        true,
    );
    let erased_fs: Arc<dyn CgroupFileSystem> = fs.clone();
    let erased_processes: Arc<dyn LinuxProcessApi> = processes.clone();
    let mut factory =
        CgroupV2BackendFactory::from_parts(config, capability, erased_fs, erased_processes);
    factory.nonce_base = 42;
    (fs, processes, factory)
}

fn test_spec(name: &str) -> ContainmentSpec {
    ContainmentSpec::new(
        ContainmentIdentity::new(name).expect("identity"),
        ContainmentScope::Tool,
    )
    .with_timing(Duration::from_millis(1), Duration::from_millis(1))
}

#[test]
fn prepare_preopens_controls_and_preexec_membership_precedes_payload() {
    let (_fs, _processes, factory) = fake_factory(true);
    let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(factory);
    let containment = ContainmentFactory::new(ContainmentBackendPolicy::RequireCgroupV2, backend);
    let prepared = containment.prepare(test_spec("ordering")).expect("prepare");
    let root = PathBuf::from(prepared.root_identity().value());
    let procs = root.join("cgroup.procs");
    let mut command = ContainmentCommand::new("sh");
    command
            .args([
                "-c",
                &format!(
                    "test \"$(cat '{}')\" = 0 && test /proc/self/fd/{INHERITED_CGROUP_SCOPE_FD} -ef '{}'",
                    procs.display(),
                    root.display()
                ),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    let process = prepared.spawn(command).expect("spawn");
    let status = process.wait_root().expect("wait payload");
    assert!(
        status.success(),
        "payload observed pre-exec membership write"
    );
    let report = process.cleanup(CleanupTrigger::NormalRootExit);
    assert!(matches!(
        report.recursive_empty(),
        RecursiveEmptyProof::Proven { .. }
    ));
}

#[test]
fn nested_cleanup_removes_directories_deepest_first() {
    let (fs, _processes, factory) = fake_factory(true);
    let prepared = factory
        .prepare_cgroup(&test_spec("nested"))
        .expect("prepare");
    let root = PathBuf::from(prepared.root_identity().value());
    let child = root.join("nested-tool");
    fs.create_cgroup(&child).expect("nested child");
    let grandchild = child.join("leaf");
    fs.create_cgroup(&grandchild).expect("nested grandchild");
    fs.removals.lock().expect("removals").clear();
    drop(prepared);
    assert!(
        !root.exists(),
        "prepared rollback removed the recursive tree"
    );
    assert_eq!(
        fs.removals.lock().expect("removals").as_slice(),
        &[grandchild, child, root],
        "recursive cgroups must be removed deepest-first"
    );
}

#[test]
fn auto_selection_emits_capabilities_and_uses_the_supplied_fallback() {
    let (_fs, _processes, mut factory) = fake_factory(true);
    factory.capability.pidfd = false;
    factory.capability.diagnostic = Some("pidfd_open/pidfd_send_signal are unavailable".to_owned());
    let observer = Arc::new(RecordingCapabilityObserver::default());
    let fallback = Arc::new(RecordingFallback::default());
    let factory = factory
        .with_capability_observer(observer.clone())
        .with_fallback(fallback.clone());

    let result = factory.prepare_backend(ContainmentBackendPolicy::Auto, &test_spec("auto"));
    assert!(result.is_err(), "recording fallback deliberately fails");
    assert_eq!(
        fallback.policies.lock().expect("policies").as_slice(),
        &[ContainmentBackendPolicy::ForceLinuxSupervisor]
    );
    let capabilities = observer.capabilities.lock().expect("capabilities");
    assert_eq!(capabilities.len(), 1);
    let capability = &capabilities[0];
    assert!(capability.unified_mount().is_some());
    assert!(capability.delegation());
    assert!(capability.writable_subtree());
    assert!(capability.cgroup_kill());
    assert!(!capability.pidfd());
    assert!(capability.probe_rollback_complete());
}

#[test]
fn partial_prepare_is_rolled_back_before_auto_fallback() {
    let (fs, _processes, factory) = fake_factory(true);
    let root = factory
        .capability()
        .dedicated_subtree()
        .expect("dedicated")
        .join("job-job-1")
        .join("attempt-attempt-2")
        .join("owner-kind-tool")
        .join("owner-id-rollback")
        .join("nonce-000000000000002a-0000000000000000");
    fs.fail_opens
        .lock()
        .expect("fail opens")
        .insert(root.join("cgroup.procs"));
    let fallback = Arc::new(RecordingFallback::default());
    let factory = factory.with_fallback(fallback.clone());

    let result = factory.prepare_backend(ContainmentBackendPolicy::Auto, &test_spec("rollback"));
    assert!(result.is_err(), "recording fallback deliberately fails");
    assert!(!root.exists(), "partial cgroup must be rolled back");
    assert_eq!(
        fallback.policies.lock().expect("policies").as_slice(),
        &[ContainmentBackendPolicy::ForceLinuxSupervisor],
        "fallback is attempted only after rollback"
    );
}

#[test]
fn incomplete_partial_prepare_rollback_blocks_auto_fallback() {
    let (fs, _processes, factory) = fake_factory(true);
    let root = factory
        .capability()
        .dedicated_subtree()
        .expect("dedicated")
        .join("job-job-1")
        .join("attempt-attempt-2")
        .join("owner-kind-tool")
        .join("owner-id-blocked-rollback")
        .join("nonce-000000000000002a-0000000000000000");
    fs.fail_opens
        .lock()
        .expect("fail opens")
        .insert(root.join("cgroup.procs"));
    fs.fail_removes
        .lock()
        .expect("fail removes")
        .insert(root.clone());
    let fallback = Arc::new(RecordingFallback::default());
    let factory = factory.with_fallback(fallback.clone());

    let result = factory.prepare_backend(
        ContainmentBackendPolicy::Auto,
        &test_spec("blocked-rollback"),
    );
    let error = match result {
        Ok(_) => panic!("incomplete rollback unexpectedly prepared a backend"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("partial cgroup rollback failed"));
    assert!(
        root.exists(),
        "failed rollback remains available to scavenging"
    );
    assert!(fallback.policies.lock().expect("policies").is_empty());
}

#[test]
fn startup_scavenging_kills_stale_members_and_retains_inspection_errors() {
    let (fs, processes, factory) = fake_factory(true);
    let dedicated = factory.capability().dedicated_subtree().expect("dedicated");
    let stale = dedicated.join("stale");
    fs.create_cgroup(&stale).expect("stale");
    processes.add(9001);
    fs.set_members(&stale, &[9001]);
    let report = factory.scavenge_stale();
    assert!(report.removed().contains(&stale));

    let blocked = dedicated.join("blocked");
    fs.create_cgroup(&blocked).expect("blocked");
    fs.fail_reads
        .lock()
        .expect("fail reads")
        .insert(blocked.join("cgroup.events"));
    let report = factory.scavenge_stale();
    assert!(
        report
            .retained()
            .iter()
            .any(|entry| entry.path() == blocked)
    );
    assert!(blocked.exists());
}

#[test]
fn missing_cgroup_kill_enumerates_nested_members_with_pidfds() {
    let (fs, processes, factory) = fake_factory(false);
    let prepared = factory
        .prepare_cgroup(&test_spec("no-kill"))
        .expect("prepare");
    let root = PathBuf::from(prepared.root_identity().value());
    let nested = root.join("leaf");
    fs.create_cgroup(&nested).expect("nested");
    processes.add(7001);
    fs.set_members(&nested, &[7001]);
    let controls = PreparedControls::open(fs.as_ref(), root).expect("controls");
    let mut kernel = CgroupV2Containment {
        root: prepared.root_identity(),
        controls,
        fs: fs.clone(),
        processes: processes.clone(),
        inspections: 0,
        removed: false,
        direct_child_reaped: None,
    };
    let batch = kernel
        .signal_members(ContainmentSignal::Kill)
        .expect("kill");
    assert_eq!(batch.attempts().len(), 1);
    assert_eq!(
        processes.signaled.lock().expect("signals").as_slice(),
        &[(7001, ContainmentSignal::Kill)]
    );
    fs.set_members(&nested, &[]);
    drop(kernel);
    drop(prepared);
}

#[test]
fn populated_event_race_cannot_produce_an_early_empty_proof() {
    let (fs, _processes, factory) = fake_factory(true);
    let prepared = factory
        .prepare_cgroup(&test_spec("events-race"))
        .expect("prepare");
    let root = PathBuf::from(prepared.root_identity().value());
    fs.queue_events(&root, &["populated 1\n", "populated 0\n"]);
    let controls = PreparedControls::open(fs.as_ref(), root.clone()).expect("controls");
    let mut kernel = CgroupV2Containment {
        root: prepared.root_identity(),
        controls,
        fs: fs.clone(),
        processes: Arc::new(FakeProcesses::default()),
        inspections: 0,
        removed: false,
        direct_child_reaped: None,
    };
    assert!(matches!(
        kernel.verify_recursive_empty().expect("first events read"),
        RecursiveEmptyProof::NotEmpty { .. }
    ));
    assert!(matches!(
        kernel.verify_recursive_empty().expect("second events read"),
        RecursiveEmptyProof::Proven { .. }
    ));
    drop(prepared);
}

#[test]
fn membership_inspection_errors_are_not_reported_as_empty() {
    let (fs, _processes, factory) = fake_factory(true);
    let prepared = factory
        .prepare_cgroup(&test_spec("inspect-error"))
        .expect("prepare");
    let root = PathBuf::from(prepared.root_identity().value());
    fs.fail_reads
        .lock()
        .expect("fail reads")
        .insert(root.join("cgroup.procs"));
    let controls = PreparedControls::open(fs.as_ref(), root).expect("controls");
    let mut kernel = CgroupV2Containment {
        root: prepared.root_identity(),
        controls,
        fs: fs.clone(),
        processes: Arc::new(FakeProcesses::default()),
        inspections: 0,
        removed: false,
        direct_child_reaped: None,
    };
    // Root membership uses its pre-opened descriptor; fail a nested read to
    // prove traversal errors propagate instead.
    let nested = kernel.controls.path.join("nested");
    fs.create_cgroup(&nested).expect("nested");
    fs.fail_reads
        .lock()
        .expect("fail reads")
        .insert(nested.join("cgroup.procs"));
    assert!(kernel.discover_members().is_err());
    drop(kernel);
    fs.fail_reads.lock().expect("fail reads").clear();
    drop(prepared);
}

#[test]
fn real_delegated_cgroup_contains_and_removes_setsid_descendant_when_available() {
    let config =
        CgroupV2FactoryConfig::new(format!("test-{}", std::process::id()), "real-delegation")
            .expect("config");
    let factory = CgroupV2BackendFactory::system(config);
    if !factory.capability().delegation_available() {
        return;
    }
    let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(factory);
    let containment = ContainmentFactory::new(ContainmentBackendPolicy::RequireCgroupV2, backend);
    let pid_file = std::env::temp_dir().join(format!(
        "temper-cgroup-descendant-{}-{}",
        std::process::id(),
        TEST_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut command = ContainmentCommand::new("sh");
    command.args([
        "-c",
        &format!(
            "setsid sh -c 'sleep 30' >/dev/null 2>&1 & echo $! > '{}'",
            pid_file.display()
        ),
    ]);
    let process = containment
        .prepare(test_spec("real-setsid"))
        .expect("prepare real cgroup")
        .spawn(command)
        .expect("spawn real cgroup");
    process.wait_root().expect("wait direct child");
    let descendant: u32 = fs::read_to_string(&pid_file)
        .expect("descendant pid")
        .trim()
        .parse()
        .expect("numeric descendant pid");
    let report = process.cleanup(CleanupTrigger::NormalRootExit);
    assert!(matches!(
        report.recursive_empty(),
        RecursiveEmptyProof::Proven { .. }
    ));
    assert!(
        proc_identity(descendant).is_err(),
        "setsid descendant survived cleanup"
    );
    let _ = fs::remove_file(pid_file);
}
