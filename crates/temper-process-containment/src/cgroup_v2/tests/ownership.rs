use super::*;

#[test]
fn startup_scavenging_reclaims_only_proven_stale_owners() {
    let (fs, processes, factory) = fake_factory(true);
    let dedicated = factory.capability().dedicated_subtree().expect("dedicated");
    let stale = create_owned_tree(fs.as_ref(), dedicated, "crashed", 8001, 42);
    let stale_descendant = stale.join("job-stale");
    fs.create_cgroup(&stale_descendant)
        .expect("stale descendant");
    processes.add_with_start(8001, 42);
    processes.remove(8001);
    processes.add(9001);
    fs.set_members(&stale_descendant, &[9001]);

    let active = create_owned_tree(fs.as_ref(), dedicated, "active", 8002, 84);
    let active_descendant = active.join("job-active");
    fs.create_cgroup(&active_descendant)
        .expect("active descendant");
    processes.add_with_start(8002, 84);
    processes.add(9002);
    fs.set_members(&active_descendant, &[9002]);

    let report = factory.scavenge_stale();
    assert!(report.removed().contains(&stale));
    assert_eq!(report.protected_count(), 1);
    assert!(!stale.exists());
    assert!(active_descendant.exists());
    assert_eq!(
        fs.kills.lock().expect("kills").as_slice(),
        &[stale],
        "startup must signal only the crashed owner's tree"
    );
}

#[test]
fn startup_scavenging_retains_unknown_and_uninspectable_ownership() {
    let (fs, _processes, factory) = fake_factory(true);
    let dedicated = factory.capability().dedicated_subtree().expect("dedicated");
    let legacy = dedicated.join("job-legacy");
    fs.create_cgroup(&legacy).expect("legacy root");
    let blocked = create_owned_tree(fs.as_ref(), dedicated, "blocked", 8100, 99);
    fs.fail_reads
        .lock()
        .expect("fail reads")
        .insert(blocked.join("cgroup.events"));
    let report = factory.scavenge_stale();
    assert!(
        report
            .retained()
            .iter()
            .any(|entry| entry.path() == blocked),
        "stale roots with incomplete empty proof are retained"
    );
    assert!(report.retained().iter().any(|entry| entry.path() == legacy));
    assert!(blocked.exists());
    assert!(legacy.exists());
    assert!(fs.kills.lock().expect("kills").is_empty());
}

#[test]
fn startup_of_second_live_owner_does_not_signal_first_owner() {
    let (fs, processes, factory) = fake_factory(true);
    let second = factory
        .prepare_cgroup(&test_spec("worker-b-job"))
        .expect("second worker ownership tree");
    let dedicated = factory.capability().dedicated_subtree().expect("dedicated");
    let first = create_owned_tree(fs.as_ref(), dedicated, "worker-a", 8201, 101);
    let descendant = first.join("active-job");
    fs.create_cgroup(&descendant).expect("active job");
    processes.add_with_start(8201, 101);
    processes.add(9201);
    fs.set_members(&descendant, &[9201]);

    let report = factory.scavenge_stale();
    assert!(report.removed().is_empty());
    assert_eq!(report.protected_count(), 2);
    assert!(report.retained().is_empty());
    assert!(descendant.exists());
    assert!(fs.kills.lock().expect("kills").is_empty());
    drop(second);
}

#[test]
fn reused_owner_pid_proves_old_boot_stale() {
    let (fs, processes, factory) = fake_factory(true);
    let dedicated = factory.capability().dedicated_subtree().expect("dedicated");
    let stale = create_owned_tree(fs.as_ref(), dedicated, "reused", 8301, 111);
    processes.add_with_start(8301, 222);

    let report = factory.scavenge_stale();
    assert_eq!(report.removed(), &[stale]);
    assert!(fs.kills.lock().expect("kills").is_empty());
}

#[test]
fn zero_process_boot_fences_are_retained_without_signaling() {
    let (fs, _processes, factory) = fake_factory(true);
    let dedicated = factory.capability().dedicated_subtree().expect("dedicated");
    let worker = dedicated.join("worker-invalid");
    fs.create_cgroup(&worker).expect("worker root");
    let invalid = worker.join("boot-0-0");
    fs.create_cgroup(&invalid).expect("invalid boot root");
    fs.set_members(&invalid, &[9_301]);

    let report = factory.scavenge_stale();

    assert!(
        report
            .retained()
            .iter()
            .any(|entry| entry.path() == invalid)
    );
    assert!(invalid.exists());
    assert!(fs.kills.lock().expect("kills").is_empty());
}
