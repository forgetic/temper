//! The charter (agent-domain.md, 4.1): what the worker gives a run when it
//! starts it, most of it from the engine's assignment.
//!
//! It is policy as data. The run interprets no workflow vocabulary: the names
//! in a charter (of repositories, outlets, verdicts, kinds and fields) are
//! labels, compared byte for byte, and text is the LLM's to read.

use alloc::boxed::Box;
use core::mem::size_of;

use skein_lib::Token;

use crate::boundary::Invalid;
use crate::budget::Budget;
use crate::limits::Limits;
use crate::outcome::{self, OutcomeSpec};

/// What a run is given when it starts.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Charter {
    /// Text for the LLM, rendered by the engine: the work item and its
    /// lineage, the role, the action's guidance.
    pub brief: Box<[u8]>,
    pub checkout: Checkout,
    pub grants: Grants,
    /// What counts as done.
    pub outcome: OutcomeSpec,
    /// What the run may spend, across all its conversations.
    pub budget: Budget,
    /// The LLM the main conversation starts with.
    pub llm: Llm,
    /// The LLMs a sub-agent may be opened on, each named by its model, which
    /// it lists once.
    pub models: Box<[Llm]>,
}

/// The repositories prepared for the run, and which of them it may write: the
/// effective write authority, decided by the engine and enforced by the tools.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Checkout {
    pub repositories: Box<[Repository]>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Repository {
    /// The name the LLM and the tools know it by.
    pub name: Box<[u8]>,
    /// io's name for its root directory, where the worker put it.
    pub root: Token,
    pub writable: bool,
}

/// What the LLM may do besides talking. Data, never derived from a role.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Grants {
    /// The tools that act on the checkout, which conversations run themselves.
    pub tools: Tools,
    /// Reading the forge, relayed by the worker.
    pub forge: bool,
    /// Asking for sub-agents: conversations of the LLM's own, opened by the run.
    pub agents: bool,
    /// What the LLM may act on the world through, besides finishing.
    pub outlets: Box<[Outlet]>,
}

/// The families of tools a conversation has: those it runs on the checkout,
/// and those the run serves. Outlets stay with main for now.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Families {
    pub tools: Tools,
    pub forge: bool,
    pub agents: bool,
}

impl Families {
    /// The families `grants` give.
    pub(crate) fn of(grants: &Grants) -> Families {
        Families { tools: grants.tools, forge: grants.forge, agents: grants.agents }
    }

    /// Whether these families are among `wider`'s.
    pub(crate) fn within(self, wider: Families) -> bool {
        let (Tools { inspect, modify, shell }, wide) = (self.tools, wider.tools);
        (!inspect || wide.inspect)
            && (!modify || wide.modify)
            && (!shell || wide.shell)
            && (!self.forge || wider.forge)
            && (!self.agents || wider.agents)
    }
}

/// The tool families a conversation runs on the checkout.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tools {
    /// Read, list and search.
    pub inspect: bool,
    /// Write and edit, in the writable repositories.
    pub modify: bool,
    /// Run commands.
    pub shell: bool,
}

/// A delegated tool that acts through the worker, and through the engine where
/// it touches the forge (agent-domain.md, 4.3). What it does is theirs: the run
/// knows it by its name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Outlet {
    pub name: Box<[u8]>,
}

/// An LLM to talk to.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Llm {
    pub endpoint: Endpoint,
    /// The provider's name for the model.
    pub model: Box<[u8]>,
    /// The most tokens each answer may take.
    pub max_tokens: u32,
}

/// A provider endpoint the agent is configured with: which provider, where,
/// with which credentials. The protocol layer holds all of that; the domain
/// only names it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Endpoint(pub u32);

/// Whether a run may start on `charter` under `limits`, or what about it does
/// not fit them. Counts are checked before anything that compares names in
/// pairs.
pub(crate) fn check(charter: &Charter, limits: &Limits) -> Result<(), Invalid> {
    let Charter { brief: _, checkout, grants, outcome, budget, llm, models } = charter;
    if !budget.is_workable() || !budget.within(&limits.budget) {
        return Err(Invalid::Budget);
    }
    if !fits(llm, limits) || count(models.len()) > limits.models || repeated_model(models) {
        return Err(Invalid::Llm);
    }
    for model in models {
        if !fits(model, limits) {
            return Err(Invalid::Llm);
        }
    }
    match cost(charter) {
        Some(bytes) if bytes <= limits.run_bytes => {}
        Some(_) | None => return Err(Invalid::TooLarge),
    }
    if count(checkout.repositories.len()) > limits.repositories || repeated_repository(&checkout.repositories) {
        return Err(Invalid::Checkout);
    }
    if count(grants.outlets.len()) > limits.outlets || repeated_outlet(&grants.outlets) {
        return Err(Invalid::Grants);
    }
    if !outcome::is_valid(outcome, limits) {
        return Err(Invalid::Outcome);
    }
    Ok(())
}

/// The bytes a charter holds beyond its fixed size: each part held in a box
/// at its fixed size, plus its payload. `None` past a `u64`.
pub(crate) fn cost(charter: &Charter) -> Option<u64> {
    let mut cost = len(&charter.brief)?.checked_add(len(&charter.llm.model)?)?;
    let llm = u64::try_from(size_of::<Llm>()).ok()?;
    for Llm { endpoint: _, model, max_tokens: _ } in &charter.models {
        cost = cost.checked_add(llm)?.checked_add(len(model)?)?;
    }
    let repository = u64::try_from(size_of::<Repository>()).ok()?;
    for Repository { name, root: _, writable: _ } in &charter.checkout.repositories {
        cost = cost.checked_add(repository)?.checked_add(len(name)?)?;
    }
    let outlet = u64::try_from(size_of::<Outlet>()).ok()?;
    for Outlet { name } in &charter.grants.outlets {
        cost = cost.checked_add(outlet)?.checked_add(len(name)?)?;
    }
    cost.checked_add(outcome::cost(&charter.outcome)?)
}

/// Whether an LLM asks for an answer that fits the limits.
fn fits(llm: &Llm, limits: &Limits) -> bool {
    llm.max_tokens > 0 && llm.max_tokens <= limits.max_tokens
}

fn repeated_model(models: &[Llm]) -> bool {
    for (index, llm) in models.iter().enumerate() {
        for other in models.get(index.saturating_add(1)..).unwrap_or_default() {
            if other.model == llm.model {
                return true;
            }
        }
    }
    false
}

fn repeated_repository(repositories: &[Repository]) -> bool {
    for (index, repository) in repositories.iter().enumerate() {
        for other in repositories.get(index.saturating_add(1)..).unwrap_or_default() {
            if other.name == repository.name {
                return true;
            }
        }
    }
    false
}

fn repeated_outlet(outlets: &[Outlet]) -> bool {
    for (index, outlet) in outlets.iter().enumerate() {
        for other in outlets.get(index.saturating_add(1)..).unwrap_or_default() {
            if other.name == outlet.name {
                return true;
            }
        }
    }
    false
}

/// A length as a count, saturating: anything past a `u32` is past every limit.
pub(crate) fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

pub(crate) fn len(bytes: &[u8]) -> Option<u64> {
    u64::try_from(bytes.len()).ok()
}
