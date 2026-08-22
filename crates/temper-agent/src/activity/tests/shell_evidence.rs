use temper_agent_core::{
    DiagnosticToolArguments, ShellDiscoveryDispositionStatusV1, ShellDiscoveryDispositionV1,
};

use super::*;

#[derive(Default)]
struct HumanPreviewRecorder(Mutex<Vec<Option<String>>>);

impl ActivityProjection for HumanPreviewRecorder {
    fn emit(&self, _record: &AgentActivityChildRecordV1) {}

    fn emit_tool_started(
        &self,
        _record: &AgentActivityChildRecordV1,
        human_arg_preview: Option<&str>,
    ) {
        self.0
            .lock()
            .expect("human previews")
            .push(human_arg_preview.map(str::to_string));
    }
}

#[test]
fn shell_preview_and_complete_diagnostic_evidence_follow_capture_mode() {
    const PREVIEW: &str = "`cargo test -p temper-agent --test very-long-…`";
    const COMPLETE: &str =
        r#"{"command":"cargo test -p temper-agent --test very-long-integration-name"}"#;

    for mode in [
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(Recorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                ..Default::default()
            },
            Arc::new(FakeClock::new(0..10)),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("provider", "model"));
        run.observability.events.emit(AgentEvent::ToolStart {
            id: "bash-1".to_string(),
            name: "bash".to_string(),
            arg_preview: Some(PREVIEW.to_string()),
            diagnostic_arguments: Some(DiagnosticToolArguments::new(COMPLETE.to_string())),
            shell_discovery_disposition: None,
        });

        let frames = recorder.0.lock().expect("frames");
        let started = frames
            .iter()
            .find_map(|frame| match &frame.event {
                AgentActivityEventV1::ToolStarted(started) => Some(started),
                _ => None,
            })
            .expect("tool.started boundary");
        let arguments = started
            .arguments
            .as_ref()
            .and_then(|content| match content {
                CapturedContentV1::Inline(inline) => Some(inline),
                CapturedContentV1::Blob { .. } => None,
            });
        match mode {
            CaptureModeV1::Metadata => assert!(arguments.is_none()),
            CaptureModeV1::Transcript => {
                assert_eq!(arguments.map(|value| value.text.as_str()), Some(PREVIEW));
            }
            CaptureModeV1::Diagnostic => {
                let arguments = arguments.expect("complete diagnostic arguments");
                assert_eq!(arguments.text, COMPLETE);
                assert!(!arguments.truncated);
            }
            CaptureModeV1::Off => unreachable!(),
        }
    }
}

#[test]
fn denied_shell_disposition_survives_every_capture_mode_without_private_arguments() {
    const PRIVATE: [&str; 7] = [
        "DENIED-COMMAND",
        "PRIVATE-ARGV",
        "/private/path",
        "PROVIDER-VALUE",
        "PROMPT-VALUE",
        "CREDENTIAL-VALUE",
        "PROCESS-LOCAL-VALUE",
    ];
    let private = PRIVATE.join(" ");
    let disposition = ShellDiscoveryDispositionV1::excluded_never_executed_local_policy_denial();

    for mode in [
        CaptureModeV1::Metadata,
        CaptureModeV1::Transcript,
        CaptureModeV1::Diagnostic,
    ] {
        let recorder = Arc::new(Recorder::default());
        let human = Arc::new(HumanPreviewRecorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1 {
                capture: mode,
                ..Default::default()
            },
            Arc::new(FakeClock::new(0..10)),
            vec![recorder.clone(), human.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("provider", "model"));
        run.observability.events.emit(AgentEvent::ToolStart {
            id: "denied-bash".to_string(),
            name: "bash".to_string(),
            // The normalizer independently enforces the argument-free closed
            // state even if an internal producer supplies stale presentations.
            arg_preview: Some(private.clone()),
            diagnostic_arguments: Some(DiagnosticToolArguments::new(format!(
                r#"{{"command":"{private}","argv":["PRIVATE-ARGV"]}}"#
            ))),
            shell_discovery_disposition: Some(disposition),
        });

        let frames = recorder.0.lock().expect("frames");
        let started = frames
            .iter()
            .find_map(|frame| match &frame.event {
                AgentActivityEventV1::ToolStarted(started) => Some((frame, started)),
                _ => None,
            })
            .expect("tool.started boundary");
        assert_eq!(started.1.arguments, None, "capture mode {mode:?}");
        assert_eq!(
            started.1.shell_discovery_disposition,
            Some(disposition),
            "capture mode {mode:?}"
        );
        assert_eq!(
            disposition.status,
            ShellDiscoveryDispositionStatusV1::ExcludedNeverExecutedLocalPolicyDenial
        );
        assert_eq!(&*human.0.lock().expect("human previews"), &[None]);

        let serialized = serde_json::to_string(started.0).expect("frame serializes");
        let debug = format!("{:?}", started.0);
        assert!(serialized.contains("excluded_never_executed_local_policy_denial"));
        for private in PRIVATE {
            assert!(!serialized.contains(private), "activity leaked {private}");
            assert!(!debug.contains(private), "Debug leaked {private}");
        }
    }
}

#[test]
fn diagnostic_shell_evidence_is_omitted_instead_of_truncated_to_policy() {
    let recorder = Arc::new(Recorder::default());
    let factory = ScopeFactory::with_parts(
        AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Diagnostic,
            max_inline_bytes: 24,
            ..Default::default()
        },
        Arc::new(FakeClock::new(0..10)),
        vec![recorder.clone()],
    );
    let run = factory.main("main", ModelIdentity::new("provider", "model"));
    run.observability.events.emit(AgentEvent::ToolStart {
        id: "bash-over-limit".to_string(),
        name: "bash".to_string(),
        arg_preview: Some("`cargo test -p temper-agent…`".to_string()),
        diagnostic_arguments: Some(DiagnosticToolArguments::new(
            r#"{"command":"cargo test -p temper-agent --all-targets"}"#.to_string(),
        )),
        shell_discovery_disposition: None,
    });

    let frames = recorder.0.lock().expect("frames");
    let started = frames
        .iter()
        .find_map(|frame| match &frame.event {
            AgentActivityEventV1::ToolStarted(started) => Some(started),
            _ => None,
        })
        .expect("tool.started boundary");
    assert_eq!(started.arguments, None);
}
