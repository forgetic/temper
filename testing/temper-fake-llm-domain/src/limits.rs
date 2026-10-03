//! Owned payload caps and the provider's worst-case live heap.

use core::mem::size_of;

use skein_lib::{Deadlines, Id, Slab};

use crate::api::{Answer, Line, Message, Part, Query, Script, ToolSpec, Turn};
use crate::domain::{Call, Config};

pub(crate) fn fits(bytes: Option<u64>, cap: u32) -> bool {
    match bytes {
        Some(bytes) => bytes <= u64::from(cap),
        None => false,
    }
}

fn array(len: usize, item_bytes: usize) -> Option<u64> {
    u64::try_from(len).ok()?.checked_mul(u64::try_from(item_bytes).ok()?)
}

fn bytes(items: &[u8]) -> Option<u64> {
    u64::try_from(items.len()).ok()
}

fn parts(items: &[Part]) -> Option<u64> {
    let mut total = array(items.len(), size_of::<Part>())?;
    for part in items {
        let owned = match part {
            Part::Text { text } => bytes(text)?,
            Part::ToolCall { id, name, arguments } => {
                bytes(id)?.checked_add(bytes(name)?)?.checked_add(bytes(arguments)?)?
            }
            Part::ToolOutput { id, output, is_error: _ } => bytes(id)?.checked_add(bytes(output)?)?,
        };
        total = total.checked_add(owned)?;
    }
    Some(total)
}

pub(crate) fn query(query: &Query) -> Option<u64> {
    let mut total = bytes(&query.model)?.checked_add(bytes(&query.system)?)?;
    total = total
        .checked_add(array(query.tools.len(), size_of::<ToolSpec>())?)?
        .checked_add(array(query.messages.len(), size_of::<Message>())?)?;
    for tool in &query.tools {
        total = total
            .checked_add(bytes(&tool.name)?)?
            .checked_add(bytes(&tool.description)?)?
            .checked_add(bytes(&tool.parameters)?)?;
    }
    for message in &query.messages {
        total = total.checked_add(parts(&message.parts)?)?;
    }
    Some(total)
}

pub(crate) fn scripts(scripts: &[Script]) -> Option<u64> {
    let mut total = array(scripts.len(), size_of::<Script>())?;
    for script in scripts {
        total = total.checked_add(bytes(&script.cue)?)?.checked_add(array(script.turns.len(), size_of::<Turn>())?)?;
        for turn in &script.turns {
            total = total.checked_add(array(turn.lines.len(), size_of::<Line>())?)?;
            for line in &turn.lines {
                let owned = match line {
                    Line::Text { text } => bytes(text)?,
                    Line::Call { name, arguments } => bytes(name)?.checked_add(bytes(arguments)?)?,
                };
                total = total.checked_add(owned)?;
            }
        }
    }
    Some(total)
}

pub(crate) fn answer(answer: &Answer) -> Option<u64> {
    parts(&answer.parts)
}

/// The bytes of a scripted answer before any payload is copied.
pub(crate) fn scripted_answer(turn: &Turn) -> Option<u64> {
    let mut total = u64::try_from(turn.lines.len()).ok()?.checked_mul(u64::try_from(size_of::<Part>()).ok()?)?;
    for line in &turn.lines {
        let owned = match line {
            Line::Text { text } => bytes(text)?,
            Line::Call { name, arguments } => bytes(name)?.checked_add(bytes(arguments)?)?.checked_add(21)?,
        };
        total = total.checked_add(owned)?;
    }
    Some(total)
}

/// A bound before allocating random tool calls: any selected tool's name,
/// a minted id and the fixed argument menu. No allocation depends on an
/// unchecked query length or tool count.
pub(crate) fn random_answer(query: &Query, count: u32) -> Option<u64> {
    let mut name = 0;
    for tool in &query.tools {
        name = name.max(bytes(&tool.name)?);
    }
    // Covers malformed replacement names and the largest argument menu.
    let one = u64::try_from(size_of::<Part>()).ok()?.checked_add(name.max(17))?.checked_add(256)?;
    u64::from(count).checked_mul(one)
}

/// The provider's live heap, including stored scripts, delayed answers and
/// scratch while generating an answer. Container bookkeeping is included;
/// allocator overhead and requests handed to the receiver are not.
///
/// Query payloads are capped before generation. A random call copies at most
/// one query's bytes plus its fixed id and argument menu. A script's line
/// array and payloads fit `script_bytes`; each line adds a generated id and
/// a Part. Converting the temporary List into a boxed slice can hold both
/// arrays at once, and the truncated answer can coexist with the full one.
#[must_use]
pub fn worst_case(config: &Config) -> Option<u64> {
    let query = u64::from(config.query_bytes);
    let scripts = u64::from(config.script_bytes);
    let part = u64::try_from(size_of::<Part>()).ok()?;
    // Dynamic answers are admitted before copying. Fixed replies are tiny
    // even when the configured cap refuses them after generation.
    let generated = u64::from(config.answer_bytes).max(part.checked_add(256)?);
    let scratch = query.checked_add(generated.checked_mul(3)?)?;
    Slab::<Call>::worst_case(config.calls)?
        .checked_add(Deadlines::<Id<Call>>::worst_case(config.calls)?)?
        .checked_add(scripts)?
        .checked_add(u64::from(config.calls).checked_mul(u64::from(config.answer_bytes))?)?
        .checked_add(scratch)
}
