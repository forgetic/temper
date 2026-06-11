use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{
    ConversationId, ConversationProfileId, ConversationRequest, ConversationTurn, InteractionError,
    InteractiveResponder, Participant, ProcessResponder, ProcessResponderConfig,
};

fn temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "temper-interaction-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn basic_request() -> ConversationRequest {
    ConversationRequest::new(
        ConversationProfileId::new("product-manager").expect("valid profile"),
        ConversationId::new("conversation-1").expect("valid conversation"),
        vec![ConversationTurn::new(Participant::human("human"), "hello")],
    )
}

#[test]
fn process_responder_sends_request_reads_reply_and_filters_environment() {
    temper_io_engine::block_on(async move {
        let request_path = temp_path("request.json");
        let script_path = temp_path("responder.sh");
        fs::write(
        &script_path,
        r#"cat > "$1"
if [ "${TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_ALLOWED:-}" = "allowed-value" ] && [ -z "${TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_BLOCKED:-}" ]; then
  printf '%s\n' '{"message":"env-ok","proposals":[]}'
else
  printf '%s\n' '{"message":"env-leaked","proposals":[]}'
fi
"#,
    )
    .expect("script writes");
        std::env::set_var(
            "TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_ALLOWED",
            "allowed-value",
        );
        std::env::set_var(
            "TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_BLOCKED",
            "blocked-value",
        );

        let responder = ProcessResponder::new(
            ProcessResponderConfig::new("/bin/sh")
                .with_args([
                    script_path.to_string_lossy().into_owned(),
                    request_path.to_string_lossy().into_owned(),
                ])
                .with_env_allowlist(["TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_ALLOWED"])
                .with_timeout(Duration::from_secs(2)),
        )
        .expect("config validates");
        let reply = responder.respond(&basic_request()).await.expect("responds");

        assert_eq!(reply.message, "env-ok");
        let captured: ConversationRequest =
            serde_json::from_str(&fs::read_to_string(&request_path).expect("request was captured"))
                .expect("captured request parses");
        assert_eq!(captured, basic_request());
        std::env::remove_var("TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_ALLOWED");
        std::env::remove_var("TEMPER_INTERACTION_PROCESS_RESPONDER_TEST_BLOCKED");
        let _ = fs::remove_file(request_path);
        let _ = fs::remove_file(script_path);
    })
}

#[test]
fn process_responder_reports_timeout_exit_malformed_json_and_duplicate_ids() {
    temper_io_engine::block_on(async move {
        let timeout = ProcessResponder::new(
            ProcessResponderConfig::new("/bin/sh")
                .with_args(["-c".to_string(), "cat >/dev/null; sleep 1".to_string()])
                .with_timeout(Duration::from_millis(20)),
        )
        .expect("config validates")
        .respond(&basic_request())
        .await
        .expect_err("timeout fails");
        assert!(matches!(
            timeout,
            InteractionError::ProcessResponderTimeout { .. }
        ));

        let exit = ProcessResponder::new(ProcessResponderConfig::new("/bin/sh").with_args([
            "-c".to_string(),
            "cat >/dev/null; printf 'bad news' >&2; exit 7".to_string(),
        ]))
        .expect("config validates")
        .respond(&basic_request())
        .await
        .expect_err("nonzero exit fails");
        assert!(matches!(
            exit,
            InteractionError::ProcessResponderExit { status, stderr }
                if status.contains('7') && stderr.contains("bad news")
        ));

        let malformed = ProcessResponder::new(
        ProcessResponderConfig::new("/bin/sh").with_args([
            "-c".to_string(),
            "cat >/dev/null; printf '%s' '{\"message\":\"one\",\"proposals\":[]}{\"message\":\"two\",\"proposals\":[]}'".to_string(),
        ]),
    )
    .expect("config validates")
    .respond(&basic_request())
    .await
    .expect_err("multiple JSON values fail");
        assert!(matches!(
            malformed,
            InteractionError::ProcessResponderMalformedJson { .. }
        ));

        let duplicate = ProcessResponder::new(
        ProcessResponderConfig::new("/bin/sh").with_args([
            "-c".to_string(),
            "cat >/dev/null; printf '%s' '{\"message\":\"dup\",\"proposals\":[{\"id\":\"same\",\"kind\":\"issue\",\"title\":\"First\",\"payload\":{}},{\"id\":\"same\",\"kind\":\"issue\",\"title\":\"Second\",\"payload\":{}}]}'".to_string(),
        ]),
    )
    .expect("config validates")
    .respond(&basic_request())
    .await
    .expect_err("duplicate ids fail");
        assert!(matches!(
            duplicate,
            InteractionError::DuplicateProposalId { .. }
        ));
    })
}
