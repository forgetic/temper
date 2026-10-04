//! Peers: what the top level keeps of each conversation the run opens, from
//! its `Open` to its session's `Ended`, and the tickets that session carries.
//!
//! A session cannot name the run's types, and step code has no generics, so
//! what is run-typed in a session is a ticket: an opaque token, which the top
//! level resolves to the value it stands for when it translates the session's
//! records for the run and the protocol layer. Tickets name values within one
//! session, so each peer keeps its own, and they go when it does: the tools
//! the run serves it, two fixed tickets ([`FINISH`], [`SUB_AGENT`]); the asks
//! of its last completion's calls to them, from the completion until the
//! session dispatches each to the run (or until it yields or calls the LLM
//! again, when the calls it did not dispatch never will be); and the run's
//! answers, from the run's `Return` until the session ends, as they stay in
//! its transcript and are copied into every prompt.
//!
//! Both are bounded by the session's byte limit, so that a peer holds little
//! more than its session could: an answer is charged to the session at its
//! fixed size plus its payload (the `bytes` of the session's `Answer`), and a
//! session holds no more answers than fit its limit, besides those of the one
//! batch it waits for, which it has not charged: on the ready list until they
//! reach it, or arriving once it is closing or has no room for them (see
//! [`payload`]); and the asks a completion makes are held only while what
//! they hold, counted the same way, fits the limit too. A call beyond that is
//! handed to the session as too large, and answered so to the LLM.

use core::mem::size_of;

use skein_lib::{List, Map, Token};
use temper_agent_domain_run::outcome::{Change, Child, Declared, Field, Verdict};
use temper_agent_domain_run::{self as run, Ask};
use temper_agent_domain_session::{self as session, llm as sllm};

use crate::llm::{self, Block, Decoded, Message, Prompt, Returned, Said, Served};
use crate::translate::{self, FINISH, FIRST, Offered, SUB_AGENT};

/// A conversation the run opened, and its session's tickets.
#[derive(Debug)]
pub(crate) struct Peer {
    /// The run's token for the conversation, which is its session's opener.
    pub(crate) conversation: Token,
    pub(crate) account: u32,
    /// The session's token for itself, once it has opened.
    pub(crate) session: Option<Token>,
    offered: Offered,
    /// The asks of the last completion's calls to the tools the run serves,
    /// by ticket, until the session dispatches them; and what they hold.
    asks: Map<u64, Ask>,
    held: u64,
    /// The run's answers to the session's calls, by ticket.
    answers: Map<u64, run::Returned>,
    /// The next ticket to give.
    next: u64,
}

impl Peer {
    pub(crate) fn new(conversation: Token, account: u32, offered: Offered, limits: &session::Limits) -> Peer {
        Peer {
            conversation,
            account,
            session: None,
            offered,
            asks: Map::with_capacity(asks(limits)),
            held: 0,
            answers: Map::with_capacity(answers(limits)),
            next: FIRST,
        }
    }

    /// The tickets it holds.
    pub(crate) fn tickets(&self) -> u32 {
        self.asks.len().saturating_add(self.answers.len())
    }

    /// The session's view of `completion`: each call to a tool the run serves
    /// becomes a ticket for its ask, while what the asks hold fits `limit`; a
    /// call to one the session was not offered is no call.
    pub(crate) fn completion(&mut self, completion: llm::Completion, limit: u64) -> sllm::Completion {
        let llm::Completion { content, stop, usage } = completion;
        let mut blocks = List::with_capacity(u32::try_from(content.len()).expect("a completion fits in memory"));
        for said in content {
            let block = match said {
                Said::Text { text } => sllm::Block::Text { text },
                Said::Opaque { bytes } => sllm::Block::Opaque { bytes },
                Said::ToolCall { id, name, input, call } => {
                    sllm::Block::ToolCall { id, name, input, call: self.decoded(call, limit) }
                }
            };
            blocks.push(block).expect("room for every block");
        }
        sllm::Completion { content: blocks.into_boxed(), stop, usage }
    }

    fn decoded(&mut self, call: Decoded, limit: u64) -> sllm::Decoded {
        let ask = match call {
            Decoded::Owned { call } => return sllm::Decoded::Owned { call },
            Decoded::Invalid { problem } => return sllm::Decoded::Invalid { problem },
            Decoded::Served { ask } => ask,
        };
        let offered = match &ask {
            Ask::Finish { .. } => self.offered.finish,
            Ask::SubAgent { .. } => self.offered.agents,
        };
        if !offered {
            return sllm::Decoded::Invalid { problem: sllm::Problem::UnknownTool };
        }
        let held = match ask_cost(&ask) {
            Some(cost) => self.held.checked_add(cost),
            None => None,
        };
        let held = match held {
            Some(held) if held <= limit && self.asks.len() < self.asks.capacity() => held,
            Some(_) | None => return sllm::Decoded::Invalid { problem: sllm::Problem::TooLarge },
        };
        let effect = translate::effect(&ask);
        let ticket = self.ticket();
        let fresh = self.asks.insert(ticket, ask).expect("checked for room above");
        assert!(fresh.is_none(), "tickets are not reused");
        self.held = held;
        sllm::Decoded::Delegated { ticket: Token::new(ticket), effect }
    }

    /// The ask the session dispatches as `ticket`.
    pub(crate) fn take(&mut self, ticket: Token) -> Ask {
        self.asks.remove(&ticket.raw()).expect("a session dispatches a call once, by the ticket it was given")
    }

    /// The session goes on without the calls it has not dispatched: it
    /// yielded, or called the LLM again. How many tickets went.
    pub(crate) fn forget_asks(&mut self, limits: &session::Limits) -> u32 {
        let forgotten = self.asks.len();
        self.asks = Map::with_capacity(asks(limits));
        self.held = 0;
        forgotten
    }

    /// Keeps the run's answer `returned` for the session, under a new ticket.
    pub(crate) fn answer(&mut self, returned: run::Returned) -> sllm::Answer {
        let bytes = returned_cost(&returned).expect("an answer the run makes fits in memory");
        let error = translate::failed(&returned);
        let ticket = self.ticket();
        let fresh = self.answers.insert(ticket, returned).expect("room for the answers a session may hold");
        assert!(fresh.is_none(), "tickets are not reused");
        sllm::Answer { ticket: Token::new(ticket), bytes, error }
    }

    /// The protocol layer's view of the session's `prompt`: every ticket in it
    /// resolved, the answers copied.
    pub(crate) fn prompt(&self, prompt: sllm::Prompt) -> Prompt {
        let sllm::Prompt { endpoint, model, system, tools, delegated, messages, max_tokens } = prompt;
        let mut served = List::with_capacity(u32::try_from(delegated.len()).expect("two at most"));
        for descriptor in &delegated {
            let tool = match descriptor.ticket {
                FINISH => Served::Finish,
                SUB_AGENT => Served::SubAgent,
                _ => unreachable!("a session's descriptors are the ones it was given"),
            };
            served.push(tool).expect("room for every descriptor");
        }
        let mut resolved = List::with_capacity(u32::try_from(messages.len()).expect("a transcript fits its limit"));
        for sllm::Message { role, content } in messages {
            let mut blocks = List::with_capacity(u32::try_from(content.len()).expect("a message fits in memory"));
            for block in content {
                blocks.push(self.block(block)).expect("room for every block");
            }
            resolved.push(Message { role, content: blocks.into_boxed() }).expect("room for every message");
        }
        Prompt {
            endpoint,
            model,
            system,
            tools,
            served: served.into_boxed(),
            messages: resolved.into_boxed(),
            max_tokens,
        }
    }

    fn block(&self, block: sllm::Block) -> Block {
        match block {
            sllm::Block::Text { text } => Block::Text { text },
            sllm::Block::Opaque { bytes } => Block::Opaque { bytes },
            sllm::Block::ToolCall { id, name, input, call: _ } => Block::ToolCall { id, name, input },
            sllm::Block::ToolResult { id, result } => {
                let result = match result {
                    sllm::Returned::Owned { outcome } => Returned::Owned { outcome },
                    sllm::Returned::Delegated { answer } => {
                        let returned = self.answers.get(&answer.ticket.raw()).expect("an answer lives as its session");
                        Returned::Served { returned: translate::copy(returned), error: answer.error }
                    }
                    sllm::Returned::Invalid { problem } => Returned::Invalid { problem },
                    sllm::Returned::NotRun => Returned::NotRun,
                };
                Block::ToolResult { id, result }
            }
        }
    }

    fn ticket(&mut self) -> u64 {
        let ticket = self.next;
        self.next = ticket.checked_add(1).expect("a session makes fewer calls than a u64 counts");
        ticket
    }
}

/// The most asks a peer holds: as many as fit the session's byte limit at
/// their fixed size.
pub(crate) fn asks(limits: &session::Limits) -> u32 {
    per(limits.session_bytes, size_of::<Ask>())
}

/// The most answers a peer holds: as many as the session's byte limit takes
/// at their fixed size, and those of a batch that arrives once it takes no
/// more.
pub(crate) fn answers(limits: &session::Limits) -> u32 {
    per(limits.session_bytes, size_of::<run::Returned>()).saturating_add(limits.parallel_tools)
}

fn per(bytes: u64, size: usize) -> u32 {
    let size = u64::try_from(size).expect("a size fits in a u64").max(1);
    u32::try_from(bytes.checked_div(size).unwrap_or(0)).unwrap_or(u32::MAX)
}

/// What an ask holds: its fixed size, and each part held in a box at its
/// fixed size plus its payload.
fn ask_cost(ask: &Ask) -> Option<u64> {
    let payload = match ask {
        Ask::Finish { outcome: Declared::Change(Change { title, body }) } => len(title)?.checked_add(len(body)?)?,
        Ask::Finish { outcome: Declared::Verdict(Verdict { name, body, children }) } => {
            let child = size(size_of::<Child>())?;
            let field = size(size_of::<Field>())?;
            let mut cost = len(name)?.checked_add(len(body)?)?;
            for Child { kind, fields } in children {
                cost = cost.checked_add(child)?.checked_add(len(kind)?)?;
                for Field { name, value } in fields {
                    cost = cost.checked_add(field)?.checked_add(len(name)?)?.checked_add(len(value)?)?;
                }
            }
            cost
        }
        Ask::SubAgent { brief, families: _, llm, share: _ } => {
            let llm = match llm {
                Some(llm) => len(llm)?,
                None => 0,
            };
            len(brief)?.checked_add(llm)?
        }
    };
    size(size_of::<Ask>())?.checked_add(payload)
}

/// The most an answer holds beyond its fixed size, as [`returned_cost`]
/// counts it, under the run's `limits`, or `None` past a `u64`: a sub-agent's
/// answer; a failed check's tail and its repository's name, which the charter
/// holds; or the problems listed of an outcome rejected, each naming a field
/// of the outcome spec, which the charter holds, or of the outcome declared.
pub(crate) fn payload(limits: &run::Limits) -> Option<u64> {
    let answered = u64::from(limits.answer_bytes);
    let failed = u64::from(limits.check_tail).checked_add(limits.run_bytes)?;
    let problem = size(size_of::<run::outcome::Problem>())?.checked_add(limits.run_bytes.max(limits.outcome_bytes))?;
    let rejected = u64::from(run::outcome::Problems::LISTED).checked_mul(problem)?;
    Some(answered.max(failed).max(rejected))
}

/// What an answer holds, as the session is charged for it: its fixed size,
/// and its payload.
fn returned_cost(returned: &run::Returned) -> Option<u64> {
    let payload = match returned {
        run::Returned::Accepted
        | run::Returned::Moved
        | run::Returned::Unpushed { .. }
        | run::Returned::Cancelled
        | run::Returned::TimedOut
        | run::Returned::Busy
        | run::Returned::Unanswered { .. }
        | run::Returned::Refused { .. } => 0,
        run::Returned::Rejected { problems } => {
            let problem = size(size_of::<run::outcome::Problem>())?;
            let mut cost = problem.checked_mul(size(problems.listed.len())?)?;
            for listed in &problems.listed {
                let field = match listed {
                    run::outcome::Problem::MissingField { child: _, field }
                    | run::outcome::Problem::EmptyField { child: _, field }
                    | run::outcome::Problem::RepeatedField { child: _, field } => len(field)?,
                    run::outcome::Problem::TooLarge { .. }
                    | run::outcome::Problem::ChangeNotAllowed
                    | run::outcome::Problem::VerdictNotAllowed
                    | run::outcome::Problem::UnknownVerdict
                    | run::outcome::Problem::TooFewChildren { .. }
                    | run::outcome::Problem::TooManyChildren { .. }
                    | run::outcome::Problem::KindNotAllowed { .. }
                    | run::outcome::Problem::EmptyTitle
                    | run::outcome::Problem::EmptyBody => 0,
                };
                cost = cost.checked_add(field)?;
            }
            cost
        }
        run::Returned::ChecksFailed { repository, ran } => len(repository)?.checked_add(len(&ran.output)?)?,
        run::Returned::Answered { text, cut: _, stop: _ } => len(text)?,
    };
    size(size_of::<run::Returned>())?.checked_add(payload)
}

fn len(bytes: &[u8]) -> Option<u64> {
    size(bytes.len())
}

fn size(size: usize) -> Option<u64> {
    u64::try_from(size).ok()
}
