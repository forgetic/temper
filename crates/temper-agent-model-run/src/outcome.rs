//! What counts as done (agent-model.md, 4.1 and 4.4): the outcome spec a
//! charter carries, the outcome an LLM declares when it finishes, and
//! [`judge`], which checks the one against the other. Names are labels,
//! compared byte for byte and never interpreted.

use alloc::boxed::Box;
use core::mem::size_of;

use temper_lib::List;
use temper_lib::bytes::copy_of;

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

/// An outcome an LLM declares when it finishes, typed by the protocol layer
/// from the input it wrote.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Declared {
    /// A change: the diff of the checkout, with the title and body of its pull
    /// request.
    Change {
        title: Box<[u8]>,
        body: Box<[u8]>,
    },
    Verdict(Verdict),
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Verdict {
    pub name: Box<[u8]>,
    /// What the LLM says about it.
    pub body: Box<[u8]>,
    pub children: Box<[Child]>,
}

/// One of a verdict's children: a comment, an issue to open, whatever its
/// kind names.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Child {
    pub kind: Box<[u8]>,
    pub fields: Box<[Field]>,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Field {
    pub name: Box<[u8]>,
    pub value: Box<[u8]>,
}

/// Something wrong with a declared outcome, for the LLM to fix. Children are
/// counted from zero, in the order the LLM gave them.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Problem {
    /// The run may not finish with a change.
    ChangeNotAllowed,
    /// The run may not finish with a verdict.
    VerdictNotAllowed,
    /// No verdict the run may finish with has this name.
    UnknownVerdict,
    /// The verdict has fewer children than its contract requires.
    TooFewChildren { min: u32 },
    /// The verdict has more children than its contract allows.
    TooManyChildren { max: u32 },
    /// A child is of a kind its verdict does not allow.
    KindNotAllowed { child: u32 },
    /// A child lacks `field`, which its verdict requires of every child.
    MissingField { child: u32, field: Box<[u8]> },
}

/// What is wrong with a declared outcome: the first problems found, at most
/// [`Problems::LISTED`] of them in the order they were found, and how many
/// more there were.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Problems {
    pub listed: Box<[Problem]>,
    pub more: u32,
}

impl Problems {
    /// Enough for the LLM to see what is wrong, few enough to keep what it is
    /// told small.
    pub const LISTED: u32 = 8;
}

/// Whether a run whose outcome spec is `spec` may finish with `declared`, or
/// what is wrong with it.
pub fn judge(spec: &OutcomeSpec, declared: &Declared) -> Result<(), Problems> {
    let mut found = Found { listed: List::with_capacity(Problems::LISTED), more: 0 };
    match declared {
        Declared::Change { title: _, body: _ } => {
            if !spec.change {
                found.add(Problem::ChangeNotAllowed);
            }
        }
        Declared::Verdict(verdict) => judge_verdict(&spec.verdicts, verdict, &mut found),
    }
    if found.listed.is_empty() {
        return Ok(());
    }
    Err(Problems { listed: found.listed.into_boxed(), more: found.more })
}

/// The problems found so far.
struct Found {
    listed: List<Problem>,
    more: u32,
}

impl Found {
    fn add(&mut self, problem: Problem) {
        match self.listed.push(problem) {
            Ok(()) => {}
            Err(_) => self.more = self.more.saturating_add(1),
        }
    }
}

fn judge_verdict(rules: &[VerdictRule], verdict: &Verdict, found: &mut Found) {
    if rules.is_empty() {
        found.add(Problem::VerdictNotAllowed);
        return;
    }
    let Some(rule) = rule_named(rules, &verdict.name) else {
        found.add(Problem::UnknownVerdict);
        return;
    };
    let Children { min, max } = rule.children;
    let children = count(verdict.children.len());
    if children < min {
        found.add(Problem::TooFewChildren { min });
    }
    if children > max {
        found.add(Problem::TooManyChildren { max });
    }
    let mut index: u32 = 0;
    for child in &verdict.children {
        if !names(&rule.kinds, &child.kind) {
            found.add(Problem::KindNotAllowed { child: index });
        }
        for field in &rule.fields {
            if !has_field(&child.fields, field) {
                found.add(Problem::MissingField { child: index, field: copy_of(field) });
            }
        }
        index = index.saturating_add(1);
    }
}

#[expect(clippy::manual_find, reason = "find takes a closure, and step code has none")]
fn rule_named<'a>(rules: &'a [VerdictRule], name: &[u8]) -> Option<&'a VerdictRule> {
    for rule in rules {
        if *rule.name == *name {
            return Some(rule);
        }
    }
    None
}

/// Whether `labels` has `label` among them.
fn names(labels: &[Box<[u8]>], label: &[u8]) -> bool {
    for candidate in labels {
        if **candidate == *label {
            return true;
        }
    }
    false
}

fn has_field(fields: &[Field], name: &[u8]) -> bool {
    for field in fields {
        if *field.name == *name {
            return true;
        }
    }
    false
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

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use temper_lib::bytes::copy_of;

    use super::{Child, Children, Declared, Field, OutcomeSpec, Problem, Problems, Verdict, VerdictRule, judge};

    fn labels(names: &[&[u8]]) -> Box<[Box<[u8]>]> {
        let mut labels = temper_lib::List::with_capacity(4);
        for name in names {
            labels.push(copy_of(name)).expect("a few labels");
        }
        labels.into_boxed()
    }

    /// A review: approve with no children, or request changes with one to
    /// three comments, each blocking or a nit, with a path and a body.
    fn review(change: bool) -> OutcomeSpec {
        let approve = VerdictRule {
            name: copy_of(b"approve"),
            children: Children { min: 0, max: 0 },
            kinds: labels(&[]),
            fields: labels(&[]),
        };
        let request = VerdictRule {
            name: copy_of(b"request-changes"),
            children: Children { min: 1, max: 3 },
            kinds: labels(&[b"blocking", b"nit"]),
            fields: labels(&[b"path", b"body"]),
        };
        OutcomeSpec { change, verdicts: Box::new([approve, request]) }
    }

    fn change() -> Declared {
        Declared::Change { title: copy_of(b"Fix the parser"), body: copy_of(b"It now accepts tabs.") }
    }

    fn verdict(name: &[u8], children: Box<[Child]>) -> Declared {
        Declared::Verdict(Verdict { name: copy_of(name), body: copy_of(b"See the comments."), children })
    }

    fn child(kind: &[u8], fields: &[&[u8]]) -> Child {
        let mut list = temper_lib::List::with_capacity(4);
        for name in fields {
            list.push(Field { name: copy_of(name), value: copy_of(b"...") }).expect("a few fields");
        }
        Child { kind: copy_of(kind), fields: list.into_boxed() }
    }

    fn comment() -> Child {
        child(b"nit", &[b"path", b"body"])
    }

    fn problems(listed: Box<[Problem]>, more: u32) -> Result<(), Problems> {
        Err(Problems { listed, more })
    }

    fn missing(child: u32, field: &[u8]) -> Problem {
        Problem::MissingField { child, field: copy_of(field) }
    }

    #[test]
    fn outcomes_are_judged_against_the_spec() {
        let cases: [(OutcomeSpec, Declared, Result<(), Problems>); 13] = [
            // What the spec allows.
            (review(true), change(), Ok(())),
            (OutcomeSpec { change: true, verdicts: Box::new([]) }, change(), Ok(())),
            (review(false), verdict(b"approve", Box::new([])), Ok(())),
            (
                review(false),
                verdict(b"request-changes", Box::new([comment(), child(b"blocking", &[b"body", b"path", b"line"])])),
                Ok(()),
            ),
            // What it does not.
            (review(false), change(), problems(Box::new([Problem::ChangeNotAllowed]), 0)),
            (
                OutcomeSpec { change: true, verdicts: Box::new([]) },
                verdict(b"approve", Box::new([])),
                problems(Box::new([Problem::VerdictNotAllowed]), 0),
            ),
            (review(false), verdict(b"reject", Box::new([])), problems(Box::new([Problem::UnknownVerdict]), 0)),
            (
                review(false),
                verdict(b"request-changes", Box::new([])),
                problems(Box::new([Problem::TooFewChildren { min: 1 }]), 0),
            ),
            (
                review(false),
                verdict(b"request-changes", Box::new([comment(), comment(), comment(), comment()])),
                problems(Box::new([Problem::TooManyChildren { max: 3 }]), 0),
            ),
            (
                review(false),
                verdict(b"approve", Box::new([comment()])),
                problems(Box::new([Problem::TooManyChildren { max: 0 }, Problem::KindNotAllowed { child: 0 }]), 0),
            ),
            (
                review(false),
                verdict(b"request-changes", Box::new([comment(), child(b"praise", &[b"path", b"body"])])),
                problems(Box::new([Problem::KindNotAllowed { child: 1 }]), 0),
            ),
            (
                review(false),
                verdict(b"request-changes", Box::new([child(b"nit", &[b"body"]), child(b"blocking", &[])])),
                problems(Box::new([missing(0, b"path"), missing(1, b"path"), missing(1, b"body")]), 0),
            ),
            // Labels are compared byte for byte.
            (
                review(false),
                verdict(b"request-changes", Box::new([child(b"Nit", &[b"path", b"body "])])),
                problems(Box::new([Problem::KindNotAllowed { child: 0 }, missing(0, b"body")]), 0),
            ),
        ];
        for (index, (spec, declared, judged)) in cases.into_iter().enumerate() {
            assert_eq!(judge(&spec, &declared), judged, "case {index}");
        }
    }

    #[test]
    fn the_problems_listed_are_bounded_and_the_rest_counted() {
        let mut children = temper_lib::List::with_capacity(5);
        for _ in 0_u32..5 {
            children.push(child(b"praise", &[])).expect("room for five");
        }
        let Err(problems) = judge(&review(false), &verdict(b"request-changes", children.into_boxed())) else {
            panic!("five children of no kind, with no fields, against a contract of three");
        };
        let listed = [
            Problem::TooManyChildren { max: 3 },
            Problem::KindNotAllowed { child: 0 },
            missing(0, b"path"),
            missing(0, b"body"),
            Problem::KindNotAllowed { child: 1 },
            missing(1, b"path"),
            missing(1, b"body"),
            Problem::KindNotAllowed { child: 2 },
        ];
        assert_eq!(&*problems.listed, &listed);
        assert_eq!(u32::try_from(problems.listed.len()), Ok(Problems::LISTED));
        // Three problems for each of the last two children, and one more for
        // the third.
        assert_eq!(problems.more, 2 + 3 + 3);
    }
}
