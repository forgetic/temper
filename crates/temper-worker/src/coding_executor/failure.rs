use temper_protocol_activity::ModelFailureV1;
use temper_protocol_worker::{FailureClass, SessionRecoveryEvidenceV1};

use crate::executor::JobOutcome;

pub(super) fn failure(class: FailureClass, message: impl Into<String>) -> JobOutcome {
    failure_with_evidence(class, message, None)
}

pub(super) fn failure_with_evidence(
    class: FailureClass,
    message: impl Into<String>,
    model_failure: Option<ModelFailureV1>,
) -> JobOutcome {
    failure_with_recovery(class, message, model_failure, None)
}

pub(super) fn failure_with_recovery(
    class: FailureClass,
    message: impl Into<String>,
    model_failure: Option<ModelFailureV1>,
    session_recovery: Option<SessionRecoveryEvidenceV1>,
) -> JobOutcome {
    JobOutcome::Failure {
        class,
        message: message.into(),
        model_failure,
        session_recovery,
    }
}
