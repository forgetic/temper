use temper_agent_core::DiagnosticToolArguments;

use super::*;

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
