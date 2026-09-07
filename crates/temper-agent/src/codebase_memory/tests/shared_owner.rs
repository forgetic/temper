//! Deterministic shared-owner failure boundaries; real provider proof is separate.
use super::*;
use std::path::Path;
use temper_codebase_memory_runtime::{LaunchConfig, ProviderBootstrap, ProviderOwnerManager};

pub(super) fn shared_owner_fixture(root: &Path, invalid: bool) -> LaunchConfig {
    let script = root.join("provider.py");
    fs::write(&script, r#"
import json, os, pathlib, subprocess, sys
root = pathlib.Path(sys.argv[1])
child = subprocess.Popen([sys.executable, '-c', '''import os,pathlib,sys,time
root=pathlib.Path(sys.argv[1])
(root/'descendant').write_text(str(os.getpid()))
while not (root/'release-descendant').exists(): time.sleep(.01)
''', str(root)], stdin=subprocess.DEVNULL)
for line in sys.stdin:
    request=json.loads(line)
    if request.get('method')=='initialize':
        name='invalid' if sys.argv[2]=='invalid' else 'codebase-memory-mcp'
        print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'serverInfo':{'name':name,'version':'0.10.8'}}}), flush=True)
(root/'frontend-exited').write_text('closed')
"#).unwrap();
    LaunchConfig {
        command: "python3".into(),
        args: vec![
            script.display().to_string(),
            root.display().to_string(),
            if invalid { "invalid" } else { "valid" }.into(),
        ],
        workspace: root.to_path_buf(),
        environment: Vec::new(),
        startup_timeout: Duration::from_secs(5),
        admission_timeout: Duration::from_secs(1),
    }
}

pub(super) fn helper() -> std::path::PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("temper-agent-containment-helper")
}

pub(super) fn wait_file(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "bounded fixture transition"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn serving_release_joins_reader_even_when_live_descendant_inherits_stdout() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manager = ProviderOwnerManager::default();
    let context = crate::containment_tests::containment_context();
    let bootstrap = ProviderBootstrap::start(
        &manager,
        shared_owner_fixture(root, false),
        context.factory(),
        &helper(),
    )
    .unwrap();
    wait_file(&root.join("descendant"));
    bootstrap.serving_admitted();
    wait_file(&root.join("frontend-exited"));
    assert!(
        bootstrap
            .completion()
            .wait(Duration::from_millis(100))
            .is_none()
    );
    fs::write(root.join("release-descendant"), "release").unwrap();
    let report = bootstrap.completion().wait(Duration::from_secs(4)).unwrap();
    assert_eq!(
        report.disposition(),
        temper_process_containment::CleanupDisposition::AlreadyEmpty
    );
    manager.reap_completed();
}

#[test]
fn failed_contract_releases_bootstrap_without_killing_already_shared_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manager = ProviderOwnerManager::default();
    let context = crate::containment_tests::containment_context();
    assert!(
        ProviderBootstrap::start(
            &manager,
            shared_owner_fixture(root, true),
            context.factory(),
            &helper()
        )
        .is_err()
    );
    wait_file(&root.join("frontend-exited"));
    let completions = manager.completions();
    assert_eq!(
        completions.len(),
        1,
        "failure keeps registered shared owner"
    );
    assert!(completions[0].wait(Duration::from_millis(100)).is_none());
    fs::write(root.join("release-descendant"), "release").unwrap();
    assert_eq!(
        completions[0]
            .wait(Duration::from_secs(4))
            .unwrap()
            .disposition(),
        temper_process_containment::CleanupDisposition::AlreadyEmpty
    );
    manager.reap_completed();
}

#[test]
fn expiration_fences_late_serving_release_until_parent_cleanup_drops_admission() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manager = ProviderOwnerManager::default();
    let context = crate::containment_tests::containment_context();
    let bootstrap = ProviderBootstrap::start(
        &manager,
        shared_owner_fixture(root, false),
        context.factory(),
        &helper(),
    )
    .unwrap();
    wait_file(&root.join("descendant"));
    let completion = bootstrap.completion();
    assert!(bootstrap.expire_admission());
    bootstrap.serving_admitted();
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !root.join("frontend-exited").exists(),
        "late event cannot remove keepalive while expired job drains"
    );
    drop(bootstrap);
    wait_file(&root.join("frontend-exited"));
    fs::write(root.join("release-descendant"), "release").unwrap();
    assert!(completion.wait(Duration::from_secs(4)).is_some());
    manager.reap_completed();
}
