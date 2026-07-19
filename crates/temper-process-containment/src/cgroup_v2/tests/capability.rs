use super::*;

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
    assert!(capability.ownership_fence());
    assert!(capability.probe_rollback_complete());
}

#[test]
fn missing_owner_fence_disables_cgroup_and_preserves_auto_fallback() {
    let fs = FakeCgroupFs::new(true);
    let processes = Arc::new(FakeProcesses::default());
    let mut config = CgroupV2FactoryConfig::new("job", "attempt").expect("config");
    let capability = probe_owned_system(&mut config, fs.as_ref(), processes.as_ref());
    assert!(!capability.ownership_fence());
    assert!(!capability.delegation_available());
    assert!(
        capability
            .diagnostic()
            .is_some_and(|diagnostic| diagnostic.contains("ownership fence"))
    );

    let erased_fs: Arc<dyn CgroupFileSystem> = fs;
    let erased_processes: Arc<dyn LinuxProcessApi> = processes;
    let fallback = Arc::new(RecordingFallback::default());
    let factory =
        CgroupV2BackendFactory::from_parts(config, capability, erased_fs, erased_processes)
            .with_fallback(fallback.clone());

    let result = factory.prepare_backend(ContainmentBackendPolicy::Auto, &test_spec("fallback"));
    assert!(result.is_err(), "recording fallback deliberately fails");
    assert_eq!(
        fallback.policies.lock().expect("policies").as_slice(),
        &[ContainmentBackendPolicy::ForceLinuxSupervisor]
    );
}
