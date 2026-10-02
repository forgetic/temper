//! The charters the fake starts runs with, drawn from its random state and its
//! configuration.
//!
//! - The brief is a text of a length drawn from the configured range.
//! - The checkout is one or two repositories, the first writable at random.
//! - Reading is always granted; writing, the shell, forge reads and a
//!   "comment" outlet each at random; sub-agents never.
//! - The outcome is a change, the verdicts "approve" (no children) and
//!   "request-changes" (one to eight children, each "blocking" or a "nit",
//!   with a "path" and a "body"), or either.
//! - The budget's turns, tokens and time are drawn from their ranges.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Duration, List, Rng};

use crate::api::{Budget, Charter, Outcome, Repository, Tools, Verdict};
use crate::model::Config;

/// What a brief says, over and over.
const TEXT: &[u8] = b"Fix the failing test in the parser, and keep the change small. ";

pub(crate) fn draw(rng: &mut Rng, config: &Config) -> Charter {
    let brief = brief(rng.between(u64::from(config.brief_min), u64::from(config.brief_max)));
    let mut repositories = List::with_capacity(2);
    let writable = rng.chance(500);
    let temper = Repository { name: copy_of(b"temper"), path: copy_of(b"/work/temper"), writable };
    repositories.push(temper).expect("room for two");
    if rng.chance(500) {
        let docs = Repository { name: copy_of(b"docs"), path: copy_of(b"/work/docs"), writable: false };
        repositories.push(docs).expect("room for two");
    }
    let tools = Tools { read: true, write: rng.chance(500), shell: rng.chance(500) };
    let forge = rng.chance(500);
    let outlets: Box<[Box<[u8]>]> = if rng.chance(500) { Box::new([copy_of(b"comment")]) } else { Box::new([]) };
    let outcome = match rng.below(3) {
        0 => Outcome { change: true, verdicts: Box::new([]) },
        1 => Outcome { change: false, verdicts: verdicts() },
        _ => Outcome { change: true, verdicts: verdicts() },
    };
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
        repositories: repositories.into_boxed(),
        tools,
        forge,
        agents: false,
        outlets,
        outcome,
        budget,
        endpoint: 0,
        model: copy_of(b"fake-1"),
        max_tokens: config.max_tokens,
    }
}

fn brief(len: u64) -> Box<[u8]> {
    let len = u32::try_from(len).expect("a configured length fits a u32");
    let mut brief = List::with_capacity(len);
    for &byte in TEXT.iter().cycle().take(usize::try_from(len).expect("a u32 fits in a usize")) {
        brief.push(byte).expect("room for the whole brief");
    }
    brief.into_boxed()
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
