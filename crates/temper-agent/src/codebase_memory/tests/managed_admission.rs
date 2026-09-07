//! A dropped startup future must retain demand until its MCP owner has drained.
use super::*;
use shared_owner::{helper, shared_owner_fixture, wait_file};
use temper_codebase_memory_runtime::{ProviderBootstrap, ProviderOwnerManager};

#[test]
fn unmanaged_builders_never_launch_without_parent_admission() {
    let server = fake_server_script();
    let workspace = tempfile::tempdir().unwrap();
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let log = workspace.path().join("unmanaged.log");
    temper_agent_io::block_on(async move {
        for mode in [CodebaseMemoryMode::Auto, CodebaseMemoryMode::Required] {
            let config = config(
                &server,
                mode,
                CodebaseMemoryIndex::Off,
                "normal",
                &log,
                json!({}),
            );
            let result = crate::codebase_memory::build_codebase_memory_toolset(
                Some(&config),
                "engineer",
                &context,
                workspace.path(),
            )
            .await;
            if mode == CodebaseMemoryMode::Required {
                assert!(result.is_err());
            } else {
                let result = result.unwrap();
                assert!(matches!(
                    result.status(),
                    CodebaseMemoryToolsetStatus::AutoUnavailable { .. }
                ));
                assert!(result.registered_tool_names().is_empty());
            }
        }
        assert!(
            !log.exists(),
            "no provider admission or graph call was attempted"
        );
    });
}

#[test]
fn expiry_and_dropped_future_retain_bootstrap_through_mcp_cleanup() {
    for expire in [false, true] {
        for mode in [CodebaseMemoryMode::Auto, CodebaseMemoryMode::Required] {
            check_startup_cleanup(expire, mode);
        }
    }
}

fn check_startup_cleanup(expire: bool, mode: CodebaseMemoryMode) {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let context = workspace_context(root, &[("acme", "demo", "demo")]);
    let manager = ProviderOwnerManager::default();
    let containment = crate::containment_tests::containment_context()
        .with_cleanup_timing(Duration::from_millis(200), Duration::from_millis(10));
    let bootstrap = ProviderBootstrap::start(
        &manager,
        shared_owner_fixture(root, false),
        containment.factory(),
        &helper(),
    )
    .unwrap();
    let completion = bootstrap.completion();
    wait_file(&root.join("descendant"));
    let script = root.join("blocked-serving.py");
    fs::write(
        &script,
        r#"
import json, pathlib, signal, sys, time
root = pathlib.Path(sys.argv[1])
def terminate(_signal, _frame):
    (root/'cleanup-held-admission').write_text(str(not (root/'frontend-exited').exists()))
signal.signal(signal.SIGTERM, terminate)
for line in sys.stdin:
    request = json.loads(line)
    if request.get('method') == 'initialize':
        (root/'serving-started').write_text('started')
        time.sleep(60)
"#,
    )
    .unwrap();
    let mut config = bad_command_config(mode);
    let provider = config.codebase_memory.as_mut().unwrap();
    provider.command = "python3".into();
    provider.args = vec![script.display().to_string(), root.display().to_string()];
    provider.startup_timeout_secs = 10;
    let started = Instant::now();
    let run_root = root.to_path_buf();
    temper_agent_io::block_on(async move {
        let root = run_root.as_path();
        let mut build = Box::pin(
            crate::codebase_memory::build_managed_codebase_memory_toolset(
                Some(&config),
                "engineer",
                &context,
                root,
                Duration::from_secs(10),
                &containment,
                bootstrap,
            ),
        );
        let admitted = async {
            while !root.join("serving-started").exists() {
                temper_agent_io::sleep_for(Duration::from_millis(5)).await;
            }
        };
        match futures::future::select(build.as_mut(), Box::pin(admitted)).await {
            futures::future::Either::Left(_) => panic!("startup must remain pending"),
            futures::future::Either::Right(_) => {}
        }
        if expire {
            let result = build.await;
            assert_eq!(result.is_err(), mode == CodebaseMemoryMode::Required);
        } else {
            drop(build);
        }
    });
    wait_file(&root.join("cleanup-held-admission"));
    assert_eq!(
        fs::read_to_string(root.join("cleanup-held-admission")).unwrap(),
        "True"
    );
    wait_file(&root.join("frontend-exited"));
    fs::write(root.join("release-descendant"), "release").unwrap();
    let report = completion.wait(Duration::from_secs(4)).unwrap();
    assert_eq!(
        report.disposition(),
        temper_process_containment::CleanupDisposition::AlreadyEmpty
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    manager.reap_completed();
}
