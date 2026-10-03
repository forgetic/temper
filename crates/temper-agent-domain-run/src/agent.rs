//! Sub-agents (agent-domain.md, section 5): a delegated call the run serves by
//! opening another conversation, on the same checkout, with families of tools
//! no wider than its asker's and never `finish`, on an LLM the charter lists,
//! with a share of what the run has left of its budget.
//!
//! The child lives as long as the call. A child that yields is done: its last
//! message is the call's result, and it is closed. One that ends without
//! yielding returns how it ended, as a tool error for its asker to read, not a
//! failure of the run. Either way, the call returns only once the child has
//! ended. Withdrawing the call, or its deadline passing, closes the child, and
//! a closing child withdraws its own calls in turn: closing cascades down the
//! tree, each owner closing what it owns, one event at a time. A child's time
//! runs out no later than its call's deadline. A refused ask is a tool error
//! too.
//!
//! Choosing the model (agent-domain.md, section 5): the charter lists the LLMs
//! a sub-agent may run on; the asking LLM may name one of them, by its model,
//! compared byte for byte; otherwise the child runs on main's.
//!
//! ```text
//! state      event                        next       emits
//! -          ask refused                  -          return: refused, or busy
//!            ask                          Working    open the child
//! Working    child yielded                Answering  close the child
//!            withdraw, deadline           Closing    close the child
//!            child ended                  Closed     return: unanswered
//! Answering  withdraw, deadline           Answering
//!            child ended                  Closed     return: answered
//! Closing    child yielded, withdraw      Closing
//!            child ended                  Closed     return: cancelled, or timed out
//! ```
//!
//! Every other cell is unreachable: a child yields or ends once, before its
//! call has returned, a call is withdrawn at most once, and its deadline's
//! alarm is cancelled when it is.

use alloc::boxed::Box;
use core::mem;

use skein_lib::bytes::copy_of;
use skein_lib::{Duration, Id};

use crate::boundary::{AskRefusal, End, Returned, Stop};
use crate::budget::{Budget, Spend};
use crate::call::{self, Withdrawal};
use crate::charter::{Charter, Families, Llm};
use crate::limits::Limits;
use crate::run::Conversation;

/// A sub-agent's call.
#[derive(Debug)]
pub(crate) enum Child {
    /// The sub-agent `child` is at work.
    Working { child: Id<Conversation> },
    /// It yielded `text`, the first of `cut` more bytes, for `stop`, and is
    /// being closed.
    Answering { child: Id<Conversation>, text: Box<[u8]>, cut: u64, stop: Stop },
    /// The call was stopped for `why`, and the child is being closed.
    Closing { child: Id<Conversation>, why: Withdrawal },
    /// Terminal: holds nothing.
    Closed,
}

/// What a sub-agent is opened with, once its ask is granted.
#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) llm: Llm,
    pub(crate) families: Families,
    pub(crate) depth: u32,
    pub(crate) budget: Budget,
}

/// What a run has to open a sub-agent with: what it has spent, how many
/// conversations it has, and the time it has left.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Means {
    pub(crate) spent: Spend,
    pub(crate) conversations: u32,
    pub(crate) left: Duration,
}

/// What a conversation with `families` at `depth` may be given when it asks
/// for a sub-agent with `wanted` families, on the LLM named `llm`, with a
/// share of at most `share`; or why not.
#[expect(clippy::too_many_arguments, reason = "an ask is checked against everything it is about")]
pub(crate) fn plan(
    charter: &Charter,
    means: Means,
    families: Families,
    depth: u32,
    wanted: Families,
    llm: Option<&[u8]>,
    share: Option<Spend>,
    limits: &Limits,
) -> Result<Plan, AskRefusal> {
    if !families.agents || !wanted.within(families) {
        return Err(AskRefusal::NotGranted);
    }
    let depth = depth.saturating_add(1);
    if depth > limits.depth {
        return Err(AskRefusal::TooDeep);
    }
    if means.conversations >= limits.run_conversations {
        return Err(AskRefusal::TooMany);
    }
    let llm = match llm {
        None => charter.llm.clone(),
        Some(name) => model(&charter.models, name).ok_or(AskRefusal::UnknownLlm)?,
    };
    let left = charter.budget.remainder(means.spent, means.left);
    let budget = match share {
        None => left,
        Some(share) => Budget {
            turns: share.turns.min(left.turns),
            input: share.input.min(left.input),
            output: share.output.min(left.output),
            cache_read: share.cache_read.min(left.cache_read),
            cache_write: share.cache_write.min(left.cache_write),
            time: left.time,
        },
    };
    if !budget.is_workable() {
        return Err(AskRefusal::Unworkable);
    }
    Ok(Plan { llm, families: wanted, depth, budget })
}

/// The child `child` of a call that just began.
pub(crate) fn working(child: Id<Conversation>) -> Child {
    Child::Working { child }
}

/// The child yielded `text` for `stop`: it is done, and its answer is kept,
/// at most `answer_bytes` of it. The child to close, if it is not closing
/// already.
pub(crate) fn yielded(call: &mut Child, text: &[u8], stop: Stop, limits: &Limits) -> Option<Id<Conversation>> {
    let state = mem::replace(call, Child::Closed);
    let (next, close) = match state {
        Child::Working { child } => {
            let max = usize::try_from(limits.answer_bytes).expect("a u32 fits in a usize");
            let kept = text.get(..max).unwrap_or(text);
            let cut = u64::try_from(text.len().saturating_sub(kept.len())).expect("a usize fits in a u64");
            (Child::Answering { child, text: copy_of(kept), cut, stop }, Some(child))
        }
        // Stopped already, and being closed.
        Child::Closing { child, why } => (Child::Closing { child, why }, None),
        Child::Answering { .. } | Child::Closed => unreachable!("a child yields at most once before it is closed"),
    };
    *call = next;
    close
}

/// The call is stopped for `why`: the child to close, if it is not closing
/// already.
pub(crate) fn withdraw(call: &mut Child, why: Withdrawal) -> Option<Id<Conversation>> {
    let state = mem::replace(call, Child::Closed);
    let (next, close) = match state {
        Child::Working { child } => (Child::Closing { child, why }, Some(child)),
        // It has answered, and is being closed: the answer stands.
        Child::Answering { child, text, cut, stop } => (Child::Answering { child, text, cut, stop }, None),
        // Its deadline stopped it before its asker withdrew it.
        Child::Closing { child, why: first } => {
            assert!(why == Withdrawal::Withdrawn, "a withdraw cancels its call's deadline");
            (Child::Closing { child, why: first }, None)
        }
        Child::Closed => unreachable!("a call is stopped only while it is in flight"),
    };
    *call = next;
    close
}

/// The child ended as `end`: what the call returns.
pub(crate) fn ended(call: &mut Child, end: End) -> Returned {
    match mem::replace(call, Child::Closed) {
        Child::Working { child: _ } => Returned::Unanswered { end },
        Child::Answering { child: _, text, cut, stop } => Returned::Answered { text, cut, stop },
        Child::Closing { child: _, why } => call::stopped(why),
        Child::Closed => unreachable!("a child ends once"),
    }
}

/// The LLM among `models` named `name`, if one is.
fn model(models: &[Llm], name: &[u8]) -> Option<Llm> {
    let mut found = None;
    for (index, llm) in models.iter().enumerate() {
        if *llm.model == *name && found.is_none() {
            found = Some(index);
        }
    }
    models.get(found?).cloned()
}
