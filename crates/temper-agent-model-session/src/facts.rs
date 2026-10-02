//! What the sessions tell whoever watches the agent (agent-model.md, section
//! 7): a fact for each thing that happened, content-free (tokens, counts and
//! classifications, never what the LLM or the opener said), in a bounded
//! queue the parent drains at its own pace.
//!
//! Facts are outside the boundary's flow control: they are not requests, take
//! no room in `out`, and when the queue is full they are dropped and counted.
//! Nothing the session decides depends on whether a fact was kept.

use temper_lib::{Duration, Queue, Token};

use crate::boundary::{End, Yield};
use crate::llm::{Failure, Stop, Usage};

/// Something that happened in the session opened for `opener`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// The session was admitted.
    Opened { opener: Token },
    /// A completion was asked for, after `attempt` retries, with `messages`
    /// messages and room for `max_tokens` in its answer.
    CompletionStarted { opener: Token, attempt: u32, messages: u32, max_tokens: u32 },
    /// The completion came back with an answer of `blocks` blocks.
    CompletionAnswered { opener: Token, stop: Stop, blocks: u32 },
    /// The completion produced no answer.
    CompletionFailed { opener: Token, failure: Failure },
    /// The completion was abandoned.
    CompletionCancelled { opener: Token },
    /// The completion that failed is tried again after `delay`, as retry
    /// `attempt`.
    CompletionRetried { opener: Token, attempt: u32, delay: Duration },
    /// The tool call at `block` of the last message started.
    ToolStarted { opener: Token, block: u32 },
    /// The tool ran, with `output` bytes of output; `error` marks a failed run.
    ToolFinished { opener: Token, output: u64, error: bool },
    /// The tool run was abandoned.
    ToolCancelled { opener: Token },
    /// The session yielded.
    Yielded { opener: Token, stop: Yield },
    /// A completion came back, and used one turn and `usage`.
    Used { opener: Token, usage: Usage },
    /// The session ended, or was refused at the entrance.
    Ended { opener: Token, end: End, turns: u32, usage: Usage },
}

/// The facts not yet drained, and how many did not fit.
#[derive(Debug)]
pub(crate) struct Facts {
    queue: Queue<Fact>,
    lost: u64,
}

impl Facts {
    pub(crate) fn with_capacity(capacity: u32) -> Facts {
        Facts { queue: Queue::with_capacity(capacity), lost: 0 }
    }

    /// Keeps `fact` if there is room for it, and counts it otherwise.
    pub(crate) fn push(&mut self, fact: Fact) {
        if self.queue.try_push(fact).is_err() {
            self.lost = self.lost.saturating_add(1);
        }
    }

    pub(crate) fn pop(&mut self) -> Option<Fact> {
        self.queue.pop()
    }

    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }
}
