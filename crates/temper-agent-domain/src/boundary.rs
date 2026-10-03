//! The records that cross the boundary with the protocol layer (4.4). The
//! domain defines them; the protocol crate depends on it.
//!
//! Three peers are behind it, and each record names which by its variant:
//!
//! - The worker, the run child domain's face. A [`Event::Start`] is a call,
//!   answered by exactly one [`Request::Answer`], after a
//!   [`Request::Admitted`] that names the run if it was admitted, so that a
//!   [`Event::Cancel`] can name it. A run's host call, [`Request::Push`], is
//!   ended by exactly one [`Event::Pushed`], or after a
//!   [`Request::CancelHost`] by [`Event::HostCancelled`] if the cancel won.
//!   [`Request::Checking`] is a notice, with no terminal.
//! - LLM providers, for the sessions: a [`Request::Complete`] is ended by one
//!   of [`Event::Completed`], [`Event::Failed`] or, after a
//!   [`Request::Cancel`] that won its race, [`Event::Cancelled`]. Its prompt
//!   and its completion are in the conversation vocabulary ([`crate::llm`]):
//!   a prompt's messages hold text, tool calls sent back as the LLM wrote
//!   them, and their results (the tools' outcome, the run's answer to a tool
//!   it serves, the problem of a call that is none, or not run); a
//!   completion holds text and tool calls, each decoded into a call to the
//!   session's own tools, an ask of a tool the run serves, or a problem.
//! - io, in two families of records for now: the file and process operations
//!   of the sessions' tools, as the tools define them (a [`Request::Io`] is
//!   ended by [`Event::Done`], with `Cancelled` if a [`Request::CancelIo`]
//!   won its race), and the run's own looks in its checkout and
//!   its checks (a [`Request::Read`] by [`Event::Read`], a [`Request::Probe`]
//!   by [`Event::Probed`], a [`Request::Check`] by [`Event::Checked`], or
//!   after a [`Request::Abort`] by [`Event::Aborted`] if the abort won). They
//!   share a shape (a command run as a contained process, in a root, with a
//!   deadline, is a check for the run and a shell call for the tools), and
//!   become one vocabulary once the io layer is designed. Every io request
//!   carries its deadline, and io runs the race (5.3).
//!
//! A request's `owner` is the token of whoever asked, echoed on its terminal
//! event. Tokens of different families may be equal: the variant routes a
//! terminal to the child domain that asked.

use skein_lib::{Duration, ReplyTo, Time, Token};
use temper_agent_domain_run as run;
use temper_agent_domain_tools as tools;

use crate::llm::{Completion, Failure, Prompt};

/// protocol -> domain
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// From the worker, a call: start a run on `charter`, and answer once it
    /// has ended. `worker` is the worker's name for the run, echoed on
    /// `Admitted`.
    Start { reply_to: ReplyTo, worker: Token, charter: run::Charter },
    /// From the worker: end the run `run` as cancelled. A run that has already
    /// answered, or decided how it ends, ignores it.
    Cancel { run: Token },
    /// Terminal for `Push`.
    Pushed { owner: Token, push: run::Push },
    /// Terminal for `Push`, after `CancelHost`: it was abandoned.
    HostCancelled { owner: Token },
    /// Terminal for `Complete`: the LLM produced its next message.
    Completed { owner: Token, completion: Completion },
    /// Terminal for `Complete`: the call produced no message.
    Failed { owner: Token, failure: Failure },
    /// Terminal for `Complete`, after `Cancel`: the call was abandoned.
    Cancelled { owner: Token },
    /// Terminal for `Io`.
    Done { owner: Token, done: tools::Done },
    /// Terminal for `Read`.
    Read { owner: Token, read: run::Read },
    /// Terminal for `Probe`: whether an executable file is at the place. A
    /// failure or a deadline passed reads as not.
    Probed { owner: Token, executable: bool },
    /// Terminal for `Check`: what the checks' process did.
    Checked { owner: Token, ran: run::Ran },
    /// Terminal for `Check`, after `Abort`: the checks were stopped.
    Aborted { owner: Token },
}

/// domain -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// To the worker: the run it names `worker` was admitted, and is `run`
    /// from now on.
    Admitted { worker: Token, run: Token },
    /// To the worker, the answer to a `Start`: exactly one per start.
    Answer { to: ReplyTo, answer: run::Answer },
    /// To the worker: checks of the run it names `worker` are running until
    /// `deadline` at the latest, so its watchdog waits that long.
    Checking { worker: Token, deadline: Time },
    /// To the worker, a host call: commit what the checkout of the run it
    /// names `worker` holds, exactly as it is, and push it, with `change`'s
    /// title and body.
    Push { worker: Token, owner: Token, change: run::outcome::Change },
    /// Abandon the host call in flight for `owner`. Its terminal still comes:
    /// `HostCancelled`, or whichever outcome won the race.
    CancelHost { owner: Token },
    /// Ask an LLM for the next assistant message, giving up after `timeout`.
    Complete { owner: Token, prompt: Prompt, timeout: Duration },
    /// Abandon the `Complete` in flight for `owner`. Its terminal event still
    /// comes: `Cancelled`, or whichever outcome won the race.
    Cancel { owner: Token },
    /// Ask io for `op` for a session's tools, giving up at `deadline`.
    Io { owner: Token, op: tools::Op, deadline: Time },
    /// Abandon the `Io` in flight for `owner`. Its terminal event still comes:
    /// `Done` with `Cancelled`, or whichever outcome won the race.
    CancelIo { owner: Token },
    /// Read the first `max` bytes of the regular file at `at`, following
    /// symbolic links within its root, giving up at `deadline`.
    Read { owner: Token, at: run::Place, max: u32, deadline: Time },
    /// Find out whether an executable file is at `at`, giving up at
    /// `deadline`.
    Probe { owner: Token, at: run::Place, deadline: Time },
    /// Run the executable at `program`, in its repository's root, as a
    /// contained process, stopping it at `deadline`; keep the last `tail`
    /// bytes of what it writes.
    Check { owner: Token, program: run::Place, deadline: Time, tail: u32 },
    /// Stop the `Check` in flight for `owner`. Its terminal still comes:
    /// `Aborted`, or `Checked` if the checks ended first.
    Abort { owner: Token },
}
