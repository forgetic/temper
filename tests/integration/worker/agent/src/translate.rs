//! The channel between the agent sub-model and a scripted agent, as the
//! protocol layer would speak it: what the model sends down, decoded into
//! what the scripted agent hears, and what the scripted agent writes, decoded
//! into what the model reads. Garbage decodes to nothing: the read ends as
//! malformed.

use temper_lib::Token;
use temper_worker_model_agent::Event;
use temper_worker_model_agent::channel::{Ask, Down, Finish, Push, Reply, RunFailure, Up};

use crate::script::{Answer, Heard, Said, Why};

/// What the scripted agent hears of `message`.
#[must_use]
pub fn down(message: Down) -> Heard {
    match message {
        Down::Start { charter, snapshot } => {
            Heard::Start { charter: charter.into_vec(), snapshot: snapshot.map(<[u8]>::into_vec) }
        }
        Down::Event { event } => Heard::Event { event: event.into_vec() },
        Down::Answer { call, reply } => Heard::Answer { name: call.raw(), answer: answer(reply) },
        Down::Cancel => Heard::Cancel,
    }
}

fn answer(reply: Reply) -> Answer {
    match reply {
        Reply::Relayed { answer } => Answer::Relayed { answer: answer.into_vec() },
        Reply::Pushed(push) => Answer::Pushed { done: push == Push::Done },
        Reply::Unavailable => Answer::Unavailable,
        Reply::Busy => Answer::Busy,
        Reply::Withdrawn => Answer::Withdrawn,
        Reply::TooLarge => Answer::TooLarge,
    }
}

/// What the model reads of `said`, the read for `owner` ending with it.
#[must_use]
pub fn up(owner: Token, said: Said) -> Event {
    let message = match said {
        Said::Call { name, push, body } => {
            let body = body.into_boxed_slice();
            let ask = if push { Ask::Push { message: body } } else { Ask::Relay { body } };
            Up::Call { call: Token::new(name), ask }
        }
        Said::Fact { text } => Up::Fact { fact: text.into_boxed_slice() },
        Said::Withdraw { name } => Up::Withdraw { call: Token::new(name) },
        Said::Long { span } => Up::Long { span },
        Said::LongDone => Up::LongDone,
        Said::Waiting { heard } => Up::Waiting { heard },
        Said::Ended { outcome } => Up::Finish { finish: Finish::Ended { outcome: outcome.into_boxed_slice() } },
        Said::Parked { snapshot } => {
            Up::Finish { finish: Finish::Parked { snapshot: snapshot.map(Vec::into_boxed_slice) } }
        }
        Said::Failed { why } => Up::Finish { finish: Finish::Failed { failure: failure(why) } },
        Said::Garbage => return Event::Malformed { owner },
    };
    Event::Received { owner, message }
}

fn failure(why: Why) -> RunFailure {
    match why {
        Why::Model => RunFailure::Model,
        Why::Budget => RunFailure::Budget,
        Why::Policy => RunFailure::Policy,
        Why::Cancelled => RunFailure::Cancelled,
        Why::Stale => RunFailure::Stale,
    }
}
