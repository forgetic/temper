//! What counts as done (agent-model.md, 4.1 and 4.4): the outcome spec a
//! charter carries. Its names are labels, compared byte for byte and never
//! interpreted.

use alloc::boxed::Box;
use core::mem::size_of;

use crate::charter::{count, len};
use crate::limits::Limits;

/// What a run may finish with: a change, a verdict from a closed list, or
/// either.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct OutcomeSpec {
    /// Whether it may finish with a change: the diff of its checkout, with a
    /// title and body for its pull request.
    pub change: bool,
    /// The verdicts it may finish with, each with its contract. None, if it
    /// may only finish with a change.
    pub verdicts: Box<[VerdictRule]>,
}

/// A verdict a run may finish with, and its contract.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct VerdictRule {
    pub name: Box<[u8]>,
    /// How many children a verdict of this name has.
    pub children: Children,
    /// The kinds its children may be.
    pub kinds: Box<[Box<[u8]>]>,
    /// The fields each of its children must carry.
    pub fields: Box<[Box<[u8]>]>,
}

/// At least `min`, at most `max`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Children {
    pub min: u32,
    pub max: u32,
}

/// Whether `spec` fits `limits` and can be met: it allows some outcome, lists
/// no more verdicts than a run may hold and no name twice, and each verdict's
/// contract can be met, requiring no more children than it allows and giving
/// a kind for children to be if it allows any.
pub(crate) fn is_valid(spec: &OutcomeSpec, limits: &Limits) -> bool {
    let OutcomeSpec { change, verdicts } = spec;
    if !*change && verdicts.is_empty() {
        return false;
    }
    if count(verdicts.len()) > limits.verdicts {
        return false;
    }
    for (index, rule) in verdicts.iter().enumerate() {
        let Children { min, max } = rule.children;
        if min > max || (max > 0 && rule.kinds.is_empty()) {
            return false;
        }
        for other in verdicts.get(index.saturating_add(1)..).unwrap_or_default() {
            if other.name == rule.name {
                return false;
            }
        }
    }
    true
}

/// The bytes `spec` holds beyond its fixed size, counted as the charter's are.
/// `None` past a `u64`.
pub(crate) fn cost(spec: &OutcomeSpec) -> Option<u64> {
    let rule = u64::try_from(size_of::<VerdictRule>()).ok()?;
    let mut cost: u64 = 0;
    for VerdictRule { name, children: _, kinds, fields } in &spec.verdicts {
        cost = cost
            .checked_add(rule)?
            .checked_add(len(name)?)?
            .checked_add(labels(kinds)?)?
            .checked_add(labels(fields)?)?;
    }
    Some(cost)
}

fn labels(labels: &[Box<[u8]>]) -> Option<u64> {
    let label = u64::try_from(size_of::<Box<[u8]>>()).ok()?;
    let mut cost: u64 = 0;
    for name in labels {
        cost = cost.checked_add(label)?.checked_add(len(name)?)?;
    }
    Some(cost)
}
