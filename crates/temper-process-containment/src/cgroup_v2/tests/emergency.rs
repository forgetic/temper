use super::*;

#[test]
fn emergency_without_cgroup_kill_uses_dedicated_numeric_pidfd_owner() {
    let (fs, processes, factory) = fake_factory(false);
    let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(factory);
    let containment = ContainmentFactory::new(ContainmentBackendPolicy::RequireCgroupV2, backend);
    let registry = containment.emergency_termination_registry();
    let prepared = containment
        .prepare(test_spec("emergency-no-kill"))
        .expect("prepare");
    let root = PathBuf::from(prepared.root_identity().value());
    let nested = root.join("leaf");
    fs.create_cgroup(&nested).expect("nested");
    processes.add(7002);
    fs.set_members(&nested, &[7002]);
    let mut command = ContainmentCommand::new("sh");
    command.args(["-c", "exit 0"]);
    let process = prepared.spawn(command).expect("spawn");

    let receipt = registry.request_hard_kill();
    assert_eq!(receipt.requested_count(), 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while processes.signaled.lock().expect("signals").is_empty()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        processes.signaled.lock().expect("signals").as_slice(),
        &[(7002, ContainmentSignal::Kill)]
    );
    assert!(fs.kills.lock().expect("kills").is_empty());

    fs.set_members(&nested, &[]);
    let report = process.cleanup(CleanupTrigger::Shutdown);
    assert!(report.proves_quiescence());
    assert!(registry.snapshot().is_empty());
}

#[test]
fn emergency_cgroup_kill_bypasses_failed_member_discovery() {
    let (fs, _processes, factory) = fake_factory(true);
    let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(factory);
    let containment = ContainmentFactory::new(ContainmentBackendPolicy::RequireCgroupV2, backend);
    let registry = containment.emergency_termination_registry();
    let prepared = containment
        .prepare(test_spec("emergency-kill"))
        .expect("prepare");
    let root = PathBuf::from(prepared.root_identity().value());
    let nested = root.join("blocked-discovery");
    fs.create_cgroup(&nested).expect("nested");
    fs.set_members(&nested, &[7331]);
    fs.fail_reads
        .lock()
        .expect("fail reads")
        .insert(nested.join("cgroup.procs"));
    let mut command = ContainmentCommand::new("sh");
    command.args(["-c", "exit 0"]);
    let process = Arc::new(prepared.spawn(command).expect("spawn"));
    let (report_tx, report_rx) = std::sync::mpsc::channel();
    let cleanup_process = Arc::clone(&process);
    let cleanup = std::thread::spawn(move || {
        report_tx
            .send(cleanup_process.cleanup(CleanupTrigger::Shutdown))
            .expect("report receiver");
    });

    std::thread::sleep(Duration::from_millis(20));
    let receipt = registry.request_hard_kill();
    assert_eq!(receipt.requested_count(), 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while fs.kills.lock().expect("kills").is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(fs.kills.lock().expect("kills").as_slice(), &[root]);
    assert!(matches!(
        report_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(registry.snapshot().registered_count(), 1);

    fs.fail_reads.lock().expect("fail reads").clear();
    let report = report_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("cleanup proof after discovery recovers");
    assert!(report.proves_quiescence());
    assert!(registry.snapshot().is_empty());
    cleanup.join().expect("join cleanup");
    drop(process);
}
