//! What the engine tells whoever watches it (engine-domain.md, section 11):
//! the child domains' facts, content-free, gathered after every entry point
//! into one bounded queue the loop drains at its own pace, with the top
//! level's own. What does not fit is dropped and counted, and nothing the
//! domain decides depends on it. What runs report goes to the views, which
//! stream and trace it: it is not among these.

use temper_engine_domain_brief as brief;
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_forge as forge;
use temper_engine_domain_notes as notes;
use temper_engine_domain_views as views;
use temper_engine_domain_work as work;

use crate::boundary::Item;

/// Something that happened in a child domain, or at the top level.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    Work {
        fact: work::Fact,
    },
    Forge {
        fact: forge::Fact,
    },
    Fleet {
        fact: fleet::Fact,
    },
    Brief {
        fact: brief::Fact,
    },
    Notes {
        fact: notes::Fact,
    },
    Views {
        fact: views::Fact,
    },
    /// The cold start is done: every claim its records hold is adopted.
    Loaded,
    /// An item announced with a record that does not split (a part missing
    /// or unknown): it is held for a person.
    Mangled {
        item: Item,
    },
    /// An item tracked that carries no record and that the engine did not
    /// make or take in: it is let go.
    Untracked {
        item: Item,
    },
    /// The rules held a write, or a run, of the item: `refused` if they
    /// refused it, else waiting for facts or a person.
    Ruled {
        item: Item,
        refused: bool,
    },
}
