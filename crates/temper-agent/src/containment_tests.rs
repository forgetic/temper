#[cfg(target_os = "linux")]
use std::sync::Arc;

use temper_agent_core::AgentContainmentContext;

/// Linux supervisor context whose helper is entered through this crate's unit
/// test harness. Backend selection is explicit in the context; the per-process
/// environment value is only the private helper protocol transport.
pub(crate) fn containment_context() -> AgentContainmentContext {
    #[cfg(target_os = "linux")]
    {
        use temper_agent_core::ContainmentFactory;
        use temper_process_containment::{
            ContainmentBackendFactory, ContainmentBackendPolicy, LinuxSupervisorBackendFactory,
        };

        let executable = std::env::current_exe().expect("current unit-test executable");
        let backend: Arc<dyn ContainmentBackendFactory> =
            Arc::new(LinuxSupervisorBackendFactory::with_helper_invocation(
                executable,
                [
                    "--exact",
                    "containment_tests::linux_supervisor_helper_entrypoint",
                    "--nocapture",
                ],
            ));
        AgentContainmentContext::new(
            ContainmentFactory::new(ContainmentBackendPolicy::ForceLinuxSupervisor, backend),
            None,
        )
        .with_cleanup_timing(
            std::time::Duration::from_millis(25),
            std::time::Duration::from_millis(5),
        )
    }

    #[cfg(not(target_os = "linux"))]
    AgentContainmentContext::production(None)
}

#[test]
#[cfg(target_os = "linux")]
fn linux_supervisor_helper_entrypoint() {
    use temper_process_containment::{
        LINUX_SUPERVISOR_TEST_HELPER_ENV, close_linux_supervisor_test_payload_stdout,
        dispatch_linux_supervisor_helper, restore_linux_supervisor_test_payload_stdout,
    };

    let hidden = std::env::var(LINUX_SUPERVISOR_TEST_HELPER_ENV);
    let Ok(hidden) = hidden else {
        // Ordinary execution as part of the unit-test suite is a no-op.
        return;
    };
    let mut arguments = hidden
        .split_whitespace()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    let process_arguments = std::env::args_os().collect::<Vec<_>>();
    if let Some(separator) = process_arguments
        .iter()
        .position(|argument| argument == "--")
    {
        arguments.extend(process_arguments[separator..].iter().cloned());
    }
    restore_linux_supervisor_test_payload_stdout().expect("restore test payload stdout");
    let status = dispatch_linux_supervisor_helper(arguments)
        .expect("test helper transport contains Linux supervisor mode");
    close_linux_supervisor_test_payload_stdout();
    assert_eq!(status, std::process::ExitCode::SUCCESS);
}
