use super::*;

#[test]
fn blocked_inspection_cannot_complete_cleanup() {
    let fake = Arc::new(FakeBackendFactory::new(
        ContainmentBackendKind::LinuxSupervisor,
        vec![process_identity(201)],
    ));
    fake.state.inspection_blocked.store(true, Ordering::Release);
    let observer = Arc::new(RecordingObserver::default());
    let scoped_observer: Arc<dyn CleanupObserver> = observer.clone();
    let factory = factory_with_observer(
        &fake,
        ContainmentBackendPolicy::ForceLinuxSupervisor,
        Some(scoped_observer),
    );
    let emergency_registry = factory.emergency_termination_registry();
    let process = Arc::new(
        factory
            .prepare(spec("blocked"))
            .expect("prepare fake containment")
            .spawn(exited_command())
            .expect("spawn fake containment"),
    );
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    let cleanup_process = Arc::clone(&process);
    let cleanup_thread = thread::spawn(move || {
        let report = cleanup_process.cleanup(CleanupTrigger::Watchdog);
        completed_tx.send(report).expect("publish cleanup report");
    });

    assert!(observer.wait_for_blocked(Duration::from_secs(2)));
    let (waiter_tx, waiter_rx) = std::sync::mpsc::channel();
    let waiting_process = Arc::clone(&process);
    let waiting_thread = thread::spawn(move || {
        let report = waiting_process.cleanup(CleanupTrigger::Shutdown);
        waiter_tx.send(report).expect("publish waiter report");
    });
    assert!(matches!(
        completed_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        waiter_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    let snapshot = emergency_registry.snapshot();
    assert_eq!(snapshot.registered_count(), 1);
    assert_eq!(snapshot.boundaries()[0].root_pid(), process.id());
    let receipt = emergency_registry.request_hard_kill();
    assert_eq!(receipt.escalation(), EmergencyEscalation::HardKill);
    assert_eq!(receipt.requested_count(), 1);
    assert_eq!(
        receipt.dispatched()[0].outcome(),
        EmergencyDispatchOutcome::Dispatched
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while fake.state.emergency_kill_calls.load(Ordering::Acquire) == 0 && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(fake.state.emergency_kill_calls.load(Ordering::Acquire), 1);
    assert!(
        fake.state
            .members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty(),
        "emergency KILL must bypass failed ordinary discovery"
    );
    assert!(matches!(
        completed_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        waiter_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        emergency_registry.snapshot().registered_count(),
        1,
        "dispatch is not ordinary cleanup proof"
    );
    assert!(
        observer
            .snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .all(|snapshot| !matches!(snapshot, CleanupSnapshot::Completed { .. }))
    );

    fake.state
        .inspection_blocked
        .store(false, Ordering::Release);
    let report = completed_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("cleanup completes after inspection recovers");
    let waiter_report = waiter_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("waiting caller receives the same cleanup report");
    assert_eq!(report, waiter_report);
    assert_eq!(report.trigger(), CleanupTrigger::Watchdog);
    assert!(matches!(
        report.recursive_empty(),
        RecursiveEmptyProof::Proven { .. }
    ));
    assert!(!report.blocked_diagnostics().is_empty());
    assert!(emergency_registry.snapshot().is_empty());
    assert!(
        observer
            .observations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .all(|observation| observation.root_pid() == process.id()),
        "discovery failures must not erase the spawned root PID"
    );
    cleanup_thread.join().expect("join cleanup caller");
    waiting_thread.join().expect("join waiting cleanup caller");
    drop(process);
}

#[test]
fn failed_recursive_empty_verification_cannot_prevent_emergency_kill() {
    let fake = Arc::new(FakeBackendFactory::new(
        ContainmentBackendKind::LinuxSupervisor,
        vec![process_identity(211)],
    ));
    fake.state.verify_blocked.store(true, Ordering::Release);
    let observer = Arc::new(RecordingObserver::default());
    let factory = factory_with_observer(
        &fake,
        ContainmentBackendPolicy::ForceLinuxSupervisor,
        Some(observer.clone()),
    );
    let registry = factory.emergency_termination_registry();
    let process = Arc::new(
        factory
            .prepare(spec("verify-blocked"))
            .expect("prepare fake containment")
            .spawn(exited_command())
            .expect("spawn fake containment"),
    );
    let (report_tx, report_rx) = std::sync::mpsc::channel();
    let cleanup_process = Arc::clone(&process);
    let cleanup_thread = thread::spawn(move || {
        report_tx
            .send(cleanup_process.cleanup(CleanupTrigger::Shutdown))
            .expect("publish cleanup report");
    });

    assert!(observer.wait_for_blocked(Duration::from_secs(2)));
    let receipt = registry.request_hard_kill();
    assert_eq!(receipt.requested_count(), 1);
    let deadline = Instant::now() + Duration::from_secs(2);
    while fake.state.emergency_kill_calls.load(Ordering::Acquire) == 0 && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(fake.state.emergency_kill_calls.load(Ordering::Acquire), 1);
    assert!(matches!(
        report_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(registry.snapshot().registered_count(), 1);

    fake.state.verify_blocked.store(false, Ordering::Release);
    let report = report_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("ordinary proof completes after verification recovers");
    assert!(report.proves_quiescence());
    assert!(registry.snapshot().is_empty());
    cleanup_thread.join().expect("join cleanup");
    drop(process);
}
