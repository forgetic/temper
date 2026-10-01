//! The fake's script: what it answers, from the query and its random state.
//!
//! - With the configured chances, the call fails as overloaded or
//!   rate-limited.
//! - The query must end with a user message whose tool outputs answer exactly
//!   the tool calls of the assistant message before it. Real providers reject
//!   anything else, and so does the fake: the call fails as invalid.
//! - While the conversation has had fewer rounds of tool calls than
//!   configured, and the query offers tools, the answer calls one of them,
//!   picked at random.
//! - Otherwise the answer is the text "done".

use alloc::boxed::Box;

use temper_lib::Rng;
use temper_lib::bytes::copy_of;

use crate::api::{Answer, Error, Finish, Message, Part, Query, Role, Usage};
use crate::model::Config;

pub(crate) fn respond(rng: &mut Rng, minted: &mut u64, config: &Config, query: &Query) -> Result<Answer, Error> {
    let roll = rng.below(1000);
    let overloaded = u64::from(config.overloaded);
    if roll < overloaded {
        return Err(Error::Overloaded);
    }
    if roll < overloaded.saturating_add(u64::from(config.rate_limited)) {
        return Err(Error::RateLimited { retry_after: config.retry_after });
    }
    if !valid(&query.messages) {
        return Err(Error::InvalidRequest);
    }
    let prompt_tokens = tokens(query);
    let tools = u64::try_from(query.tools.len()).expect("a usize fits in a u64");
    if tool_rounds(&query.messages) < config.tool_rounds && tools > 0 {
        let index = usize::try_from(rng.below(tools)).expect("an index below a usize fits in one");
        let tool = query.tools.get(index).expect("picked below the tool count");
        *minted = minted.wrapping_add(1);
        let call = Part::ToolCall { id: call_id(*minted), name: tool.name.clone(), arguments: copy_of(b"{}") };
        let usage = Usage { prompt_tokens, completion_tokens: 8 };
        return Ok(Answer { parts: Box::new([call]), finish: Finish::ToolCalls, usage });
    }
    let usage = Usage { prompt_tokens, completion_tokens: 1 };
    Ok(Answer { parts: Box::new([Part::Text { text: copy_of(b"done") }]), finish: Finish::Stop, usage })
}

/// Whether the conversation ends with a user message whose tool outputs answer
/// exactly the tool calls of the message before it.
fn valid(messages: &[Message]) -> bool {
    let Some((last, earlier)) = messages.split_last() else {
        return false;
    };
    match last.role {
        Role::User => {}
        Role::Assistant => return false,
    }
    let calls: &[Part] = match earlier.last() {
        Some(previous) => match previous.role {
            Role::Assistant => &previous.parts,
            Role::User => &[],
        },
        None => &[],
    };
    for part in &last.parts {
        match part {
            Part::ToolOutput { id, .. } => {
                if !calls_with(calls, id) {
                    return false;
                }
            }
            Part::Text { .. } | Part::ToolCall { .. } => {}
        }
    }
    for part in calls {
        match part {
            Part::ToolCall { id, .. } => {
                if !outputs_for(&last.parts, id) {
                    return false;
                }
            }
            Part::Text { .. } | Part::ToolOutput { .. } => {}
        }
    }
    true
}

fn calls_with(parts: &[Part], wanted: &[u8]) -> bool {
    for part in parts {
        match part {
            Part::ToolCall { id, .. } => {
                if **id == *wanted {
                    return true;
                }
            }
            Part::Text { .. } | Part::ToolOutput { .. } => {}
        }
    }
    false
}

fn outputs_for(parts: &[Part], wanted: &[u8]) -> bool {
    for part in parts {
        match part {
            Part::ToolOutput { id, .. } => {
                if **id == *wanted {
                    return true;
                }
            }
            Part::Text { .. } | Part::ToolCall { .. } => {}
        }
    }
    false
}

/// Assistant messages that call tools.
fn tool_rounds(messages: &[Message]) -> u32 {
    let mut rounds: u32 = 0;
    for message in messages {
        let calls_tools = match message.role {
            Role::Assistant => calls_any(&message.parts),
            Role::User => false,
        };
        if calls_tools {
            rounds = rounds.saturating_add(1);
        }
    }
    rounds
}

fn calls_any(parts: &[Part]) -> bool {
    for part in parts {
        match part {
            Part::ToolCall { .. } => return true,
            Part::Text { .. } | Part::ToolOutput { .. } => {}
        }
    }
    false
}

/// A rough token count: a token for every four bytes of text.
fn tokens(query: &Query) -> u64 {
    let mut bytes = len(&query.system);
    for message in &query.messages {
        for part in &message.parts {
            let size = match part {
                Part::Text { text } => len(text),
                Part::ToolCall { id: _, name, arguments } => len(name).saturating_add(len(arguments)),
                Part::ToolOutput { id: _, output, is_error: _ } => len(output),
            };
            bytes = bytes.saturating_add(size);
        }
    }
    bytes / 4
}

fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("a usize fits in a u64")
}

/// "call_" and sixteen hex digits of `n`.
fn call_id(n: u64) -> Box<[u8]> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut id = *b"call_0000000000000000";
    let mut rest = n;
    for digit in id.iter_mut().rev().take(16) {
        let nibble = usize::try_from(rest & 0xF).expect("a nibble fits in a usize");
        *digit = *HEX.get(nibble).expect("a nibble indexes sixteen digits");
        rest = rest.wrapping_shr(4);
    }
    copy_of(&id)
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use temper_lib::bytes::copy_of;
    use temper_lib::{Duration, Rng};

    use super::{call_id, respond, valid};
    use crate::api::{Error, Finish, Message, Part, Query, Role, ToolSpec};
    use crate::model::Config;

    const CONFIG: Config = Config {
        calls: 4,
        latency_min: Duration::ZERO,
        latency_max: Duration::ZERO,
        overloaded: 0,
        rate_limited: 0,
        retry_after: Duration::ZERO,
        tool_rounds: 1,
    };

    fn user(parts: Box<[Part]>) -> Message {
        Message { role: Role::User, parts }
    }

    fn assistant(parts: Box<[Part]>) -> Message {
        Message { role: Role::Assistant, parts }
    }

    fn text() -> Part {
        Part::Text { text: copy_of(b"hi") }
    }

    fn call(id: &[u8]) -> Part {
        Part::ToolCall { id: copy_of(id), name: copy_of(b"ls"), arguments: copy_of(b"{}") }
    }

    fn output(id: &[u8]) -> Part {
        Part::ToolOutput { id: copy_of(id), output: copy_of(b"ok"), is_error: false }
    }

    fn query(messages: Box<[Message]>) -> Query {
        let tool = ToolSpec { name: copy_of(b"ls"), description: copy_of(b""), parameters: copy_of(b"{}") };
        Query { model: copy_of(b"fake"), system: copy_of(b""), tools: Box::new([tool]), messages, max_tokens: 100 }
    }

    #[test]
    fn tool_outputs_must_answer_exactly_the_calls_before_them() {
        assert!(valid(&[user(Box::new([text()]))]));
        assert!(!valid(&[]));
        assert!(!valid(&[assistant(Box::new([text()]))]));
        let asked = assistant(Box::new([call(b"a"), call(b"b")]));
        assert!(valid(&[asked.clone(), user(Box::new([output(b"b"), output(b"a")]))]));
        assert!(!valid(&[asked.clone(), user(Box::new([output(b"a")]))]));
        assert!(!valid(&[asked, user(Box::new([output(b"a"), output(b"b"), output(b"c")]))]));
    }

    #[test]
    fn the_script_calls_tools_for_the_configured_rounds_then_answers() {
        let mut rng = Rng::new(1);
        let mut minted = 0;
        let first = respond(&mut rng, &mut minted, &CONFIG, &query(Box::new([user(Box::new([text()]))])));
        let answer = first.expect("a valid query");
        assert_eq!(answer.finish, Finish::ToolCalls);
        assert_eq!(&*answer.parts, &[call(b"call_0000000000000001")]);

        let messages = Box::new([
            user(Box::new([text()])),
            assistant(answer.parts),
            user(Box::new([output(b"call_0000000000000001")])),
        ]);
        let answer = respond(&mut rng, &mut minted, &CONFIG, &query(messages)).expect("a valid query");
        assert_eq!(answer.finish, Finish::Stop);
    }

    #[test]
    fn the_script_fails_with_the_configured_chances() {
        let mut rng = Rng::new(1);
        let mut minted = 0;
        let config = Config { rate_limited: 1000, retry_after: Duration::from_secs(2), ..CONFIG };
        let result = respond(&mut rng, &mut minted, &config, &query(Box::new([user(Box::new([text()]))])));
        assert_eq!(result, Err(Error::RateLimited { retry_after: Duration::from_secs(2) }));
    }

    #[test]
    fn call_ids_are_hex() {
        assert_eq!(&*call_id(0xAB), b"call_00000000000000ab");
        assert_eq!(&*call_id(u64::MAX), b"call_ffffffffffffffff");
    }
}
