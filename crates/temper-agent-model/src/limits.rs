use temper_agent_model_session as session;
use temper_lib::Queue;

/// The agent model's limits (section 7), handed to every step read-only: its
/// sub-models', each handed down to the one it bounds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// The session sub-model's.
    pub session: session::Limits,
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64` or the limits cannot be honoured.
///
/// It is the session sub-model's, plus the queue that holds what the session
/// emits in a step until it is routed out. What the queued requests own is
/// counted where they end up, as the session's worst case says.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let session = session::worst_case(&limits.session)?;
    let session_out = Queue::<session::Request>::worst_case(session::max_out(&limits.session))?;
    session.checked_add(session_out)
}
