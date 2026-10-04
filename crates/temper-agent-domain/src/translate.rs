//! The translations between the run's vocabulary and the session's (4.5):
//! siblings share no types, so the run's conversations and the sessions meet
//! here, through small total functions, each an exhaustive match, so that a
//! variant added on either side breaks the build in one place.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{List, Token};
use temper_agent_domain_run::charter::{Checkout, Families, Repository, Tools};
use temper_agent_domain_run::{self as run, Ask, Opening, Spend};
use temper_agent_domain_session::{self as session, Budget, Dimension, Spec, Yield, llm};
use temper_agent_domain_tools::{Authority, Effect, Grants, Name, Repo};

/// The ticket of `finish` among the tools a session is offered: tickets are
/// the top level's, and name values within one session.
pub(crate) const FINISH: Token = Token::new(0);

/// The ticket of the sub-agent tool.
pub(crate) const SUB_AGENT: Token = Token::new(1);

/// The first ticket of a session's calls and answers.
pub(crate) const FIRST: u64 = 2;

/// The tools the run serves a conversation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Offered {
    pub(crate) finish: bool,
    pub(crate) agents: bool,
}

/// The session's spec for a conversation the run opens with `opening`, and
/// the tools the run serves it; or `None` if the checkout cannot be laid out
/// for the tools.
///
/// The run's LLM is the session's endpoint, model and answer size; its system
/// text and first message are the session's; its share of the budget is the
/// session's budget. The tools it runs on the checkout, the checkout and which
/// of it may be written make the tools' authority. What the run serves is
/// offered as descriptors: `finish` if the opening says so, a write, run
/// alone; sub-agents if its families have them, a write when the families
/// asked for may write, which the widest the asker may give them do.
pub(crate) fn spec(opening: Opening) -> Option<(Spec, Offered)> {
    let Opening { llm, system, prompt, tools, checkout, budget, finish, families } = opening;
    let authority = authority(&checkout, tools)?;
    let offered = Offered { finish, agents: families.agents };
    let mut delegated = List::with_capacity(2);
    if finish {
        delegated.push(llm::Descriptor { ticket: FINISH, effect: Effect::Write }).expect("room for both");
    }
    if families.agents {
        delegated.push(llm::Descriptor { ticket: SUB_AGENT, effect: writes(families) }).expect("room for both");
    }
    let run::Budget { turns, input, output, cache_read, cache_write, time } = budget;
    let spec = Spec {
        endpoint: llm::Endpoint(llm.endpoint.0),
        model: llm.model,
        system,
        authority,
        delegated: delegated.into_boxed(),
        prompt,
        max_tokens: llm.max_tokens,
        budget: Budget { turns, input, output, cache_read, cache_write, time },
    };
    Some((spec, offered))
}

/// The tools' authority over `checkout` with `tools`, or `None` if a
/// repository's name cannot be a directory's.
///
/// Each repository is mounted at the root, under its name, which is how the
/// run names it to the LLM; relative paths start in the first repository, the
/// one the work is about, or at the root if there is none. Commands run with
/// an empty environment: a charter carries none yet.
fn authority(checkout: &Checkout, tools: Tools) -> Option<Authority> {
    let count = u32::try_from(checkout.repositories.len()).ok()?;
    let mut repos = List::with_capacity(count);
    for Repository { name, root, writable } in &checkout.repositories {
        let name = Name::new(copy_of(name))?;
        let repo = Repo { mount: Box::new([name]), root: *root, writable: *writable };
        repos.push(repo).expect("room for every repository");
    }
    let repos = repos.into_boxed();
    let cwd = match repos.first() {
        Some(first) => first.mount.clone(),
        None => Box::default(),
    };
    let Tools { inspect, modify, shell } = tools;
    Some(Authority { cwd, repos, grants: Grants { inspect, modify, shell }, env: Box::default() })
}

/// The effect of a call that opens a sub-agent with `families`: a write if it
/// may write.
fn writes(families: Families) -> Effect {
    if families.tools.modify || families.tools.shell { Effect::Write } else { Effect::Read }
}

/// The effect of a call to a tool the run serves.
pub(crate) fn effect(ask: &Ask) -> Effect {
    match ask {
        Ask::Finish { .. } => Effect::Write,
        Ask::SubAgent { families, .. } => writes(*families),
    }
}

/// Why the session yielded, as the run hears it.
pub(crate) const fn stop(stop: Yield) -> run::Stop {
    match stop {
        Yield::Done => run::Stop::EndTurn,
        Yield::Truncated => run::Stop::MaxTokens,
        Yield::Refused => run::Stop::Refusal,
        Yield::Malformed => run::Stop::NoCalls,
    }
}

/// What `turns` completions that used `usage` spent.
pub(crate) const fn spend(turns: u32, usage: llm::Usage) -> Spend {
    let llm::Usage { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens } = usage;
    Spend {
        turns,
        input: input_tokens,
        output: output_tokens,
        cache_read: cache_read_tokens,
        cache_write: cache_write_tokens,
    }
}

/// How the session ended, as the run hears it: a failed call is its
/// provider's fault, but for a conversation too long for the model, which is
/// a full context, as is a transcript past the session's limits.
pub(crate) const fn end(end: session::End) -> run::End {
    match end {
        session::End::Busy => run::End::Busy,
        session::End::Invalid => run::End::Invalid,
        session::End::Closed => run::End::Closed,
        session::End::Failed { failure } => match failure {
            llm::Failure::Exhausted { .. } => run::End::Fault(run::Fault::Exhausted),
            llm::Failure::ContextTooLong => run::End::Fault(run::Fault::ContextFull),
            llm::Failure::Overloaded
            | llm::Failure::RateLimited { .. }
            | llm::Failure::Unavailable
            | llm::Failure::TimedOut
            | llm::Failure::Invalid
            | llm::Failure::Unauthorized => run::End::Fault(run::Fault::Provider),
        },
        session::End::Budget { spent } => run::End::Budget(exhausted(spent)),
        session::End::TranscriptFull => run::End::Fault(run::Fault::ContextFull),
    }
}

const fn exhausted(spent: Dimension) -> run::Exhausted {
    match spent {
        Dimension::Turns => run::Exhausted::Turns,
        Dimension::Input => run::Exhausted::Input,
        Dimension::Output => run::Exhausted::Output,
        Dimension::CacheRead => run::Exhausted::CacheRead,
        Dimension::CacheWrite => run::Exhausted::CacheWrite,
        Dimension::Time => run::Exhausted::Time,
    }
}

/// Whether the run's answer to a call is a failure, for the LLM to read as
/// one.
pub(crate) const fn failed(returned: &run::Returned) -> bool {
    match returned {
        run::Returned::Accepted | run::Returned::Answered { .. } => false,
        run::Returned::Rejected { .. }
        | run::Returned::ChecksFailed { .. }
        | run::Returned::Moved
        | run::Returned::Unpushed { .. }
        | run::Returned::Cancelled
        | run::Returned::TimedOut
        | run::Returned::Busy
        | run::Returned::Unanswered { .. }
        | run::Returned::Refused { .. } => true,
    }
}

/// A copy of the run's answer, for a prompt.
pub(crate) fn copy(returned: &run::Returned) -> run::Returned {
    match returned {
        run::Returned::Accepted => run::Returned::Accepted,
        run::Returned::Rejected { problems } => run::Returned::Rejected { problems: problems.clone() },
        run::Returned::ChecksFailed { repository, ran } => {
            let run::Ran { exit, output, cut } = ran;
            let ran = run::Ran { exit: *exit, output: copy_of(output), cut: *cut };
            run::Returned::ChecksFailed { repository: copy_of(repository), ran }
        }
        run::Returned::Moved => run::Returned::Moved,
        run::Returned::Unpushed { failure } => run::Returned::Unpushed { failure: *failure },
        run::Returned::Cancelled => run::Returned::Cancelled,
        run::Returned::TimedOut => run::Returned::TimedOut,
        run::Returned::Busy => run::Returned::Busy,
        run::Returned::Answered { text, cut, stop } => {
            run::Returned::Answered { text: copy_of(text), cut: *cut, stop: *stop }
        }
        run::Returned::Unanswered { end } => run::Returned::Unanswered { end: *end },
        run::Returned::Refused { refusal } => run::Returned::Refused { refusal: *refusal },
    }
}
