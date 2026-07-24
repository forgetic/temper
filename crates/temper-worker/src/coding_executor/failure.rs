use temper_protocol_activity::ModelFailureV1;
use temper_protocol_worker::FailureClass;

use crate::executor::JobOutcome;

pub(super) fn failure(class: FailureClass, message: impl Into<String>) -> JobOutcome {
    failure_with_evidence(class, message, None)
}

pub(super) fn failure_with_evidence(
    class: FailureClass,
    message: impl Into<String>,
    model_failure: Option<ModelFailureV1>,
) -> JobOutcome {
    JobOutcome::Failure {
        class,
        message: message.into(),
        model_failure,
        session_recovery: None,
    }
}
