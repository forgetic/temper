//! Result-file acceptance after the managed run proves quiescence.
use super::super::output_files::{
    FirstPartyTerminalFailure, first_party_terminal_failure, read_operator_transcript,
};
use super::*;

pub(super) fn accept_result(
    outcome: ChildOutcome,
    first_party: bool,
    terminal_output_path: &Path,
    fence: &AttemptFence,
    result_path: &Path,
    accepted_submit: &AcceptedSubmitProofStore,
    operator_transcript_path: Option<&Path>,
) -> Result<AgentRunOutput, AgentRunError> {
    verify_exit(outcome, first_party, terminal_output_path)?;
    if !fence.is_open() {
        return Err(AgentRunError::new(
            temper_protocol_worker::FailureClass::Canceled,
            "agent attempt was cancelled before result acceptance",
        ));
    }
    let result_bytes = std::fs::read(result_path).map_err(|error| {
        AgentRunError::permanent(format!("agent did not write a valid result file: {error}"))
    })?;
    if !fence.is_open() {
        return Err(AgentRunError::new(
            temper_protocol_worker::FailureClass::Canceled,
            "agent attempt was cancelled while reading its result",
        ));
    }
    let result = serde_json::from_slice::<WorkspaceResult>(&result_bytes).map_err(|error| {
        AgentRunError::permanent(format!("agent result file is not valid JSON: {error}"))
    })?;
    let operator_transcript = read_operator_transcript(operator_transcript_path);
    Ok(AgentRunOutput {
        result,
        accepted_submit: fence.is_open().then(|| accepted_submit.latest()).flatten(),
        operator_transcript,
    })
}

fn verify_exit(
    outcome: ChildOutcome,
    first_party: bool,
    terminal_output_path: &Path,
) -> Result<(), AgentRunError> {
    let ChildOutcome {
        status_code,
        stderr_tail,
    } = outcome;
    match status_code {
        Some(0) => {}
        Some(code) => {
            let generic_message =
                format!("agent command exited with status {code}; stderr tail: {stderr_tail}");
            match first_party_terminal_failure(first_party, terminal_output_path) {
                Some(FirstPartyTerminalFailure::Model(model_failure)) => {
                    return Err(
                        AgentRunError::transient(generic_message).with_model_failure(model_failure)
                    );
                }
                Some(FirstPartyTerminalFailure::Policy(policy_failure)) => {
                    return Err(AgentRunError::permanent(format!(
                        "first-party agent terminal policy failure: {}",
                        policy_failure.reason.as_str()
                    ))
                    .with_failure_code(temper_protocol_activity::FailureCodeV1::Policy));
                }
                None => {}
            }
            return Err(AgentRunError::transient(generic_message));
        }
        None => {
            return Err(AgentRunError::transient(format!(
                "agent command terminated without an exit code; stderr tail: {stderr_tail}"
            )));
        }
    }

    Ok(())
}
