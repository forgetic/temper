//! The fake's script: what it answers, from the query and its random state.
//!
//! - With the configured chances, the call fails as overloaded or
//!   rate-limited.
//! - The query must end with a user message whose tool outputs answer exactly
//!   the tool calls of the assistant message before it. Real providers reject
//!   anything else, and so does the fake: the call fails as invalid.
//! - With the configured chances, the answer is refused, or says it calls
//!   tools and calls none.
//! - While the conversation has had fewer rounds of tool calls than
//!   configured since the client last wrote, and the query offers tools, the
//!   answer calls one of them, picked at random.
//! - Otherwise the answer is the text "done".
//! - An answer longer than the query's `max_tokens` is cut short there.
//! - The prompt takes a token for every four bytes of text. All of it but the
//!   last message is read from the cache, written by the call before; the
//!   last message is read afresh, and cached for the next call.

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
    let roll = rng.below(1000);
    let refused = u64::from(config.refused);
    if roll < refused {
        return Ok(answer(query, Box::new([Part::Text { text: copy_of(b"no") }]), Finish::ContentFilter, 1));
    }
    if roll < refused.saturating_add(u64::from(config.no_calls)) {
        let parts = Box::new([Part::Text { text: copy_of(b"let me call a tool") }]);
        return Ok(answer(query, parts, Finish::ToolCalls, 4));
    }
    let tools = u64::try_from(query.tools.len()).expect("a usize fits in a u64");
    if tool_rounds(&query.messages) < config.tool_rounds && tools > 0 {
        let index = usize::try_from(rng.below(tools)).expect("an index below a usize fits in one");
        let tool = query.tools.get(index).expect("picked below the tool count");
        *minted = minted.wrapping_add(1);
        let call = Part::ToolCall { id: call_id(*minted), name: tool.name.clone(), arguments: copy_of(b"{}") };
        return Ok(answer(query, Box::new([call]), Finish::ToolCalls, 8));
    }
    let tokens = rng.between(1, config.answer_tokens.max(1).into());
    Ok(answer(query, Box::new([Part::Text { text: copy_of(b"done") }]), Finish::Stop, tokens))
}

/// The answer of `parts`, which take `tokens` to say, cut short at the query's
/// `max_tokens`.
fn answer(query: &Query, parts: Box<[Part]>, finish: Finish, tokens: u64) -> Answer {
    let most = u64::from(query.max_tokens);
    if tokens > most {
        let cut = Box::new([Part::Text { text: copy_of(b"do") }]);
        return Answer { parts: cut, finish: Finish::Length, usage: usage(query, most) };
    }
    Answer { parts, finish, usage: usage(query, tokens) }
}

/// What a call with `completion_tokens` in its answer took: all of the prompt
/// but its last message from the cache, which the call before wrote, and the
/// last message afresh, which this call writes for the next.
fn usage(query: &Query, completion_tokens: u64) -> Usage {
    let mut cached = 0;
    let mut fresh = len(&query.system);
    if let Some((last, earlier)) = query.messages.split_last() {
        // The first call has nothing cached, the system text included.
        if !earlier.is_empty() {
            cached = fresh;
            fresh = 0;
        }
        for message in earlier {
            cached = cached.saturating_add(text_of(message));
        }
        fresh = fresh.saturating_add(text_of(last));
    }
    Usage { prompt_tokens: fresh / 4, cached_tokens: cached / 4, cache_creation_tokens: fresh / 4, completion_tokens }
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

/// Assistant messages that call tools, since the last user message that the
/// client wrote (one with text, not only tool outputs).
fn tool_rounds(messages: &[Message]) -> u32 {
    let mut rounds: u32 = 0;
    for message in messages {
        match message.role {
            Role::Assistant => {
                if calls_any(&message.parts) {
                    rounds = rounds.saturating_add(1);
                }
            }
            Role::User => {
                if says_any(&message.parts) {
                    rounds = 0;
                }
            }
        }
    }
    rounds
}

fn says_any(parts: &[Part]) -> bool {
    for part in parts {
        match part {
            Part::Text { .. } => return true,
            Part::ToolCall { .. } | Part::ToolOutput { .. } => {}
        }
    }
    false
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

/// The bytes of text in a message, which a rough count turns into tokens.
fn text_of(message: &Message) -> u64 {
    let mut bytes: u64 = 0;
    for part in &message.parts {
        let size = match part {
            Part::Text { text } => len(text),
            Part::ToolCall { id: _, name, arguments } => len(name).saturating_add(len(arguments)),
            Part::ToolOutput { id: _, output, is_error: _ } => len(output),
        };
        bytes = bytes.saturating_add(size);
    }
    bytes
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
        refused: 0,
        no_calls: 0,
        answer_tokens: 1,
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

        let called = assistant(answer.parts);
        let messages =
            Box::new([user(Box::new([text()])), called.clone(), user(Box::new([output(b"call_0000000000000001")]))]);
        let answer = respond(&mut rng, &mut minted, &CONFIG, &query(messages)).expect("a valid query");
        assert_eq!(answer.finish, Finish::Stop);

        // The client writes again: another round of tools, then the answer.
        let messages = Box::new([
            user(Box::new([text()])),
            called,
            user(Box::new([output(b"call_0000000000000001")])),
            assistant(answer.parts),
            user(Box::new([text()])),
        ]);
        let answer = respond(&mut rng, &mut minted, &CONFIG, &query(messages)).expect("a valid query");
        assert_eq!(answer.finish, Finish::ToolCalls);
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
    fn the_script_refuses_or_names_no_tool_with_the_configured_chances() {
        let mut rng = Rng::new(1);
        let mut minted = 0;
        let first = || query(Box::new([user(Box::new([text()]))]));
        let config = Config { refused: 1000, ..CONFIG };
        let answer = respond(&mut rng, &mut minted, &config, &first()).expect("a valid query");
        assert_eq!(answer.finish, Finish::ContentFilter);
        let config = Config { no_calls: 1000, ..CONFIG };
        let answer = respond(&mut rng, &mut minted, &config, &first()).expect("a valid query");
        assert_eq!(answer.finish, Finish::ToolCalls);
        assert!(!super::calls_any(&answer.parts), "it names no tool");
    }

    #[test]
    fn an_answer_longer_than_its_max_tokens_is_cut_short() {
        let mut rng = Rng::new(1);
        let mut minted = 0;
        let mut first = query(Box::new([user(Box::new([text()]))]));
        first.max_tokens = 3;
        let answer = respond(&mut rng, &mut minted, &CONFIG, &first).expect("a valid query");
        assert_eq!((answer.finish, answer.usage.completion_tokens), (Finish::Length, 3));
        let config = Config { tool_rounds: 0, answer_tokens: 2, ..CONFIG };
        let answer = respond(&mut rng, &mut minted, &config, &first).expect("a valid query");
        assert_eq!(answer.finish, Finish::Stop);
        assert!(answer.usage.completion_tokens <= 2, "within the configured answer");
    }

    #[test]
    fn all_but_the_last_message_is_read_from_the_cache() {
        let mut rng = Rng::new(1);
        let mut minted = 0;
        let asked = || user(Box::new([Part::Text { text: copy_of(b"12345678") }]));
        let answer = respond(&mut rng, &mut minted, &CONFIG, &query(Box::new([asked()]))).expect("a valid query");
        let usage = answer.usage;
        assert_eq!((usage.prompt_tokens, usage.cached_tokens, usage.cache_creation_tokens), (2, 0, 2));
        let messages = Box::new([asked(), assistant(answer.parts), user(Box::new([output(b"call_0000000000000001")]))]);
        let usage = respond(&mut rng, &mut minted, &CONFIG, &query(messages)).expect("a valid query").usage;
        // The prompt and the call ("ls", "{}") from the cache; the output "ok"
        // afresh.
        assert_eq!((usage.prompt_tokens, usage.cached_tokens, usage.cache_creation_tokens), (0, 3, 0));
    }

    #[test]
    fn call_ids_are_hex() {
        assert_eq!(&*call_id(0xAB), b"call_00000000000000ab");
        assert_eq!(&*call_id(u64::MAX), b"call_ffffffffffffffff");
    }
}
