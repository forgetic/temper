//! The charters the fake assigns, drawn from its random state and its
//! configuration, and their encoding.
//!
//! - The brief is a text of a length drawn from the configured range.
//! - Reading is always granted; writing, the shell, forge reads and a
//!   "comment" outlet each at random; sub-agents with the configured chance,
//!   with two more models listed for them.
//! - The outcome is a change, the verdicts "approve" (no children) and
//!   "request-changes" (one to eight children, each "blocking" or a "nit",
//!   with a "path" and a "body"), or either, with the configured chances; so
//!   is whether a change must pass its checks. A workspace with nothing
//!   writable gets the verdicts and no change.
//! - The budget's turns, tokens and time are drawn from their ranges.
//!
//! An assignment carries its charter encoded, fields in the order
//! [`Charter`] declares them, and likewise within each part: a `bool` is one
//! byte, 0 or 1; a `u32` or a `u64` is little-endian, and a time is a `u64`
//! of nanoseconds; bytes are a `u32` length, then the bytes; a list is a
//! `u32` count, then its items.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Duration, List, Rng, Writer};

use crate::api::{Budget, Charter, Outcome, Tools, Verdict};
use crate::model::Config;

/// What a brief says, over and over.
const BRIEF: &[u8] = b"Fix the failing test in the parser, and keep the change small. ";

pub(crate) fn draw(rng: &mut Rng, config: &Config, writable: bool) -> Charter {
    let brief = text(BRIEF, rng.between(u64::from(config.brief_min), u64::from(config.brief_max)));
    let tools = Tools { read: true, write: rng.chance(500), shell: rng.chance(500) };
    let forge = rng.chance(500);
    let outlets: Box<[Box<[u8]>]> = if rng.chance(500) { Box::new([copy_of(b"comment")]) } else { Box::new([]) };
    let verdicts = if !writable || rng.chance(config.verdicts) { verdicts() } else { Box::new([]) };
    let change = writable && (rng.chance(config.changes) || verdicts.is_empty());
    let outcome = Outcome { change, checks: change && rng.chance(config.checks), verdicts };
    let budget = Budget {
        turns: u32::try_from(rng.between(u64::from(config.turns_min), u64::from(config.turns_max)))
            .expect("drawn between two u32s"),
        input_tokens: rng.between(config.tokens_min, config.tokens_max),
        output_tokens: rng.between(config.tokens_min, config.tokens_max),
        cache_read_tokens: rng.between(config.tokens_min, config.tokens_max),
        cache_write_tokens: rng.between(config.tokens_min, config.tokens_max),
        wall_time: Duration::from_nanos(rng.between(config.time_min.as_nanos(), config.time_max.as_nanos())),
    };
    Charter {
        brief,
        tools,
        forge,
        agents: rng.chance(config.agents),
        outlets,
        outcome,
        budget,
        endpoint: 0,
        model: copy_of(b"fake-1"),
        max_tokens: config.max_tokens,
        models: Box::new([copy_of(b"fake-2"), copy_of(b"fake-3")]),
    }
}

/// `len` bytes of `pattern`, over and over.
pub(crate) fn text(pattern: &[u8], len: u64) -> Box<[u8]> {
    let len = u32::try_from(len).expect("a configured length fits a u32");
    let mut text = List::with_capacity(len);
    for &byte in pattern.iter().cycle().take(usize::try_from(len).expect("a u32 fits in a usize")) {
        text.push(byte).expect("room for the whole text");
    }
    text.into_boxed()
}

fn verdicts() -> Box<[Verdict]> {
    let approve = Verdict {
        name: copy_of(b"approve"),
        min_children: 0,
        max_children: 0,
        kinds: Box::new([]),
        fields: Box::new([]),
    };
    let request = Verdict {
        name: copy_of(b"request-changes"),
        min_children: 1,
        max_children: 8,
        kinds: Box::new([copy_of(b"blocking"), copy_of(b"nit")]),
        fields: Box::new([copy_of(b"path"), copy_of(b"body")]),
    };
    Box::new([approve, request])
}

/// `charter`, encoded as the module says: counted first, then written into a
/// box of that length.
#[must_use]
pub fn encode(charter: &Charter) -> Box<[u8]> {
    let mut counting = Encoder { len: 0, writer: None };
    counting.charter(charter);
    let mut writing = Encoder { len: 0, writer: Some(Writer::new(counting.len)) };
    writing.charter(charter);
    writing.writer.expect("made with a writer").finish()
}

/// Counts the bytes of an encoding, and writes them too once it has a writer
/// of the length counted.
#[derive(Debug)]
struct Encoder {
    len: usize,
    writer: Option<Writer>,
}

impl Encoder {
    fn charter(&mut self, charter: &Charter) {
        let Charter { brief, tools, forge, agents, outlets, outcome, budget, endpoint, model, max_tokens, models } =
            charter;
        self.bytes(brief);
        let Tools { read, write, shell } = *tools;
        self.flag(read);
        self.flag(write);
        self.flag(shell);
        self.flag(*forge);
        self.flag(*agents);
        self.names(outlets);
        let Outcome { change, checks, verdicts } = outcome;
        self.flag(*change);
        self.flag(*checks);
        self.count(verdicts.len());
        for verdict in verdicts {
            let Verdict { name, min_children, max_children, kinds, fields } = verdict;
            self.bytes(name);
            self.word(*min_children);
            self.word(*max_children);
            self.names(kinds);
            self.names(fields);
        }
        let Budget { turns, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, wall_time } = *budget;
        self.word(turns);
        self.long(input_tokens);
        self.long(output_tokens);
        self.long(cache_read_tokens);
        self.long(cache_write_tokens);
        self.long(wall_time.as_nanos());
        self.word(*endpoint);
        self.bytes(model);
        self.word(*max_tokens);
        self.names(models);
    }

    fn names(&mut self, names: &[Box<[u8]>]) {
        self.count(names.len());
        for name in names {
            self.bytes(name);
        }
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.count(bytes.len());
        self.raw(bytes);
    }

    fn count(&mut self, count: usize) {
        self.word(u32::try_from(count).expect("a charter's counts fit a u32"));
    }

    fn flag(&mut self, flag: bool) {
        self.raw(&[u8::from(flag)]);
    }

    fn word(&mut self, word: u32) {
        self.raw(&word.to_le_bytes());
    }

    fn long(&mut self, long: u64) {
        self.raw(&long.to_le_bytes());
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.len = self.len.checked_add(bytes.len()).expect("a charter's length fits a usize");
        if let Some(writer) = &mut self.writer {
            writer.put(bytes).expect("the writer is as long as the bytes counted");
        }
    }
}
