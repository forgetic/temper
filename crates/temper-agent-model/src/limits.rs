use core::mem::size_of;

use temper_lib::{Duration, Time};

use crate::llm::Message;
use crate::session::{Alarm, Session};

/// The agent model's limits (section 7), handed to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Sessions at once. A run beyond them is refused as busy.
    pub sessions: u32,
    /// Messages a session's transcript holds, the task's prompt included.
    pub messages: u32,
    /// Bytes a session holds: its task, its transcript and the tool results
    /// it is collecting, each block counted at its fixed size plus its payload.
    pub session_bytes: u64,
    /// Completions a session may receive.
    pub turns: u32,
    /// The largest `max_tokens` a task may ask for.
    pub max_tokens: u32,
    /// Retries of a call that failed transiently, after which the session
    /// fails.
    pub retries: u32,
    /// The wait before the first retry, doubled for each one after it, with
    /// jitter.
    pub backoff_base: Duration,
    /// The longest wait before a retry, unless the provider asks for longer.
    pub backoff_max: Duration,
    /// How long the protocol layer gives each call.
    pub call_timeout: Duration,
    /// How long a session may run.
    pub session_timeout: Duration,
}

/// The most memory the model holds under `limits`, in bytes (6.4), or `None`
/// if it does not fit a `u64` or the limits cannot be honoured.
///
/// It counts entities and payloads, not allocator overhead. The prompts of
/// calls in flight are copies held by the protocol layer, which counts them.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    // Every session may have two alarms armed, each indexed twice.
    let alarms = limits.sessions.checked_mul(2)?;
    let alarm = u64::try_from(size_of::<(Time, Alarm)>().checked_add(size_of::<(Alarm, Time)>())?).ok()?;
    let transcript = u64::try_from(size_of::<Message>()).ok()?.checked_mul(u64::from(limits.messages))?;
    let session =
        u64::try_from(size_of::<Session>()).ok()?.checked_add(transcript)?.checked_add(limits.session_bytes)?;
    u64::from(limits.sessions).checked_mul(session)?.checked_add(u64::from(alarms).checked_mul(alarm)?)
}
