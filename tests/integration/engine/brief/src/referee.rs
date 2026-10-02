//! What the brief's scenarios expect, held by a referee (testing-pyramid.md,
//! 5.2) that sees what the parent asks for, what its sources serve and what
//! the brief answers, never the brief's state:
//!
//! - a rendered brief has a section for each asked, in that order; a section
//!   has text exactly when its read brought content within the read's bounds
//!   before the brief answered, and a required one is never missing;
//! - no section is longer than its kind's budget, and the sections together
//!   no longer than the brief's;
//! - every cut is told: a section's cut lines count exactly the bytes its
//!   source served or left out that it does not hold; what it holds is its
//!   content's bytes, unchanged and in order, between the lines; a section
//!   with no line is its content exactly, as is every section of a brief
//!   whose content fits all its budgets; a kind that keeps its start (or its
//!   end) holds its content's first (or last) bytes; and the text is UTF-8,
//!   as its content was;
//! - a brief fails only for want of a required section, and is refused as
//!   oversized exactly when it is past the limits;
//! - every brief asked for is answered within its time to gather.

use std::collections::BTreeMap;

use temper_engine_model_brief::{Body, Kind, Limits, Part, Refusal, Section};
use temper_lib::Duration;
use temper_lib::bytes::find_from;
use temper_world::{Expectations, Judge};

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The parent asked for the brief `brief` of these sections (each kind,
    /// and whether it is required), whose sources name at most `items`
    /// items.
    Asked {
        brief: u64,
        sections: Vec<(Kind, bool)>,
        items: usize,
    },
    /// The read of the section at `index` of `brief` reached it.
    Served {
        brief: u64,
        index: u32,
        read: Served,
    },
    /// The brief answered.
    Rendered {
        brief: u64,
        sections: Vec<Section>,
    },
    Failed {
        brief: u64,
        missing: Kind,
    },
    Refused {
        brief: u64,
        refusal: Refusal,
    },
}

/// What a read brought.
#[derive(Clone, Debug)]
pub enum Served {
    /// Content within the read's bounds.
    Content(Vec<Part>),
    /// Content past them.
    Oversized,
    Failed,
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The brief the parent names so is answered.
    Answer(u64),
}

/// This world injects nothing of its own.
#[derive(Debug)]
pub enum Stimulus {}

/// The expectations of the brief's world.
#[derive(Debug)]
pub struct Briefs {
    limits: Limits,
    /// How long a brief may take to be answered.
    within: Duration,
    /// The briefs asked for and not answered.
    asked: BTreeMap<u64, Asked>,
    /// Sections and cut lines judged.
    pub sections: u64,
    pub cuts: u64,
}

#[derive(Debug)]
struct Asked {
    sections: Vec<(Kind, bool)>,
    /// What each section's read brought before the brief answered.
    served: BTreeMap<u32, Served>,
}

impl Briefs {
    /// Expectations of a brief under `limits`, each answered `within` its
    /// asking.
    #[must_use]
    pub fn new(limits: Limits, within: Duration) -> Briefs {
        Briefs { limits, within, asked: BTreeMap::new(), sections: 0, cuts: 0 }
    }

    fn oversized(&self, asked: &Asked) -> bool {
        asked.sections.len() > usize::try_from(self.limits.sections).expect("small")
    }

    fn rendered(&mut self, brief: u64, asked: &Asked, sections: &[Section], judge: &mut Judge<Expected, Stimulus>) {
        let kinds: Vec<Kind> = sections.iter().map(|section| section.kind).collect();
        let asked_kinds: Vec<Kind> = asked.sections.iter().map(|(kind, _)| *kind).collect();
        judge.check(kinds == asked_kinds, format_args!("brief {brief}: a section for each asked, in order"));
        judge.check(!self.oversized(asked), format_args!("brief {brief}: a brief past the limits is refused"));
        let (mut total, mut fits, mut uncut) = (0, true, 0);
        for (index, section) in sections.iter().enumerate() {
            self.sections += 1;
            let required = asked.sections.get(index).is_some_and(|(_, required)| *required);
            let index = u32::try_from(index).expect("small");
            let content = content(asked.served.get(&index));
            let text =
                match &section.body {
                    Body::Text(text) => text,
                    Body::Missing(_) => {
                        match content {
                            Some(_) => judge
                                .fail(format_args!("brief {brief}: section {index} was read in time but is missing")),
                            None => judge
                                .check(!required, format_args!("brief {brief}: a required section is never missing")),
                        }
                        continue;
                    }
                };
            let Some(parts) = content else {
                judge.fail(format_args!("brief {brief}: section {index} has text it was not served"));
                continue;
            };
            total += text.len();
            let budget = budget(&self.limits, section.kind);
            judge.check(
                text.len() <= budget,
                format_args!("brief {brief}: section {index} is within its budget: {} > {budget}", text.len()),
            );
            let length: usize = parts.iter().map(|part| part.bytes.len()).sum();
            let left: u64 = parts.iter().map(|part| part.left).sum();
            fits &= left == 0 && length <= budget;
            uncut += length;
            self.text(brief, index, section.kind, text, parts, judge);
        }
        let most = usize::try_from(self.limits.brief_bytes).expect("small");
        judge.check(total <= most, format_args!("brief {brief}: within the brief's budget: {total} > {most}"));
        if !fits || uncut > most {
            return;
        }
        for (index, section) in sections.iter().enumerate() {
            let text = match &section.body {
                Body::Text(text) => text,
                Body::Missing(_) => continue,
            };
            let Some(parts) = content(asked.served.get(&u32::try_from(index).expect("small"))) else {
                continue;
            };
            let whole: Vec<u8> = parts.iter().flat_map(|part| part.bytes.iter().copied()).collect();
            judge.check(**text == *whole, format_args!("brief {brief}: a brief that fits keeps section {index} whole"));
        }
    }

    /// Judges the text of a section against the parts it was served.
    fn text(
        &mut self,
        brief: u64,
        index: u32,
        kind: Kind,
        text: &[u8],
        parts: &[Part],
        judge: &mut Judge<Expected, Stimulus>,
    ) {
        let content: Vec<u8> = parts.iter().flat_map(|part| part.bytes.iter().copied()).collect();
        let original: u64 = parts.iter().map(|part| u64::try_from(part.bytes.len()).expect("small") + part.left).sum();
        judge.check(std::str::from_utf8(text).is_ok(), format_args!("brief {brief}: section {index} is UTF-8"));
        let Some((pieces, cut)) = read_back(text) else {
            judge.fail(format_args!("brief {brief}: section {index} has a cut line that is not whole"));
            return;
        };
        self.cuts += u64::from(cut > 0);
        let kept: usize = pieces.iter().map(Vec::len).sum();
        judge.check(
            u64::try_from(kept).expect("small") + cut == original,
            format_args!(
                "brief {brief}: section {index}'s cut lines count what it left out: {kept} + {cut} != {original}"
            ),
        );
        if cut == 0 {
            judge.check(text == content, format_args!("brief {brief}: section {index} with no cut is its content"));
            return;
        }
        let mut from = 0;
        for piece in &pieces {
            let Some(at) = find_from(&content, piece, from) else {
                judge.fail(format_args!("brief {brief}: section {index} holds its content unchanged, in order"));
                return;
            };
            from = at + piece.len();
        }
        let Some(first) = pieces.first() else {
            return;
        };
        let last = pieces.last().expect("there is a first piece");
        match kind {
            Kind::Item | Kind::Pull | Kind::Plan | Kind::Template => judge.check(
                content.starts_with(first),
                format_args!("brief {brief}: section {index} of {kind:?} keeps its content's start"),
            ),
            Kind::Comments | Kind::Attempts => judge.check(
                content.ends_with(last),
                format_args!("brief {brief}: section {index} of {kind:?} keeps its content's end"),
            ),
            Kind::Dependencies | Kind::Ci | Kind::Reviews | Kind::Notes => {}
        }
    }
}

impl Expectations for Briefs {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Asked { brief, sections, items: _ } => {
                self.asked.insert(brief, Asked { sections, served: BTreeMap::new() });
                judge.expect(Expected::Answer(brief), self.within);
            }
            Seen::Served { brief, index, read } => {
                // A read that ends after its brief answered is dropped.
                if let Some(asked) = self.asked.get_mut(&brief) {
                    let first = asked.served.insert(index, read).is_none();
                    judge.check(first, format_args!("brief {brief}: section {index} is read once"));
                }
            }
            Seen::Rendered { brief, sections } => {
                judge.meet(&Expected::Answer(brief));
                let asked = self.asked.remove(&brief).expect("a brief answers once it was asked");
                self.rendered(brief, &asked, &sections, judge);
            }
            Seen::Failed { brief, missing } => {
                judge.meet(&Expected::Answer(brief));
                let asked = self.asked.remove(&brief).expect("a brief answers once it was asked");
                judge.check(!self.oversized(&asked), format_args!("brief {brief}: a brief past the limits is refused"));
                let wanting = asked.sections.iter().enumerate().any(|(index, (kind, required))| {
                    let read = asked.served.get(&u32::try_from(index).expect("small"));
                    *kind == missing && *required && content(read).is_none()
                });
                judge.check(
                    wanting,
                    format_args!("brief {brief}: it fails only for want of a required section of {missing:?}"),
                );
            }
            Seen::Refused { brief, refusal } => {
                judge.meet(&Expected::Answer(brief));
                let asked = self.asked.remove(&brief).expect("a brief answers once it was asked");
                let oversized = self.oversized(&asked);
                judge.check(
                    (refusal == Refusal::Oversized) == oversized,
                    format_args!("brief {brief}: refused as oversized exactly when past the limits: {refusal:?}"),
                );
            }
        }
    }
}

/// The content a read brought within its bounds, if it did.
fn content(served: Option<&Served>) -> Option<&Vec<Part>> {
    match served? {
        Served::Content(parts) => Some(parts),
        Served::Oversized | Served::Failed => None,
    }
}

/// The budget of a section of `kind` under `limits`.
#[must_use]
pub fn budget(limits: &Limits, kind: Kind) -> usize {
    let budgets = &limits.budgets;
    let bytes = match kind {
        Kind::Item => budgets.item,
        Kind::Comments => budgets.comments,
        Kind::Dependencies => budgets.dependencies,
        Kind::Ci => budgets.ci,
        Kind::Reviews => budgets.reviews,
        Kind::Pull => budgets.pull,
        Kind::Attempts => budgets.attempts,
        Kind::Plan => budgets.plan,
        Kind::Notes => budgets.notes,
        Kind::Template => budgets.template,
    };
    usize::try_from(bytes).expect("a u32 fits")
}

/// A section's text read back: the runs of content between its cut lines,
/// and how many bytes the lines say were cut; `None` if a line is not
/// whole. A line after what the section holds starts with a newline of its
/// own. The world's content has no `[`.
#[must_use]
pub fn read_back(text: &[u8]) -> Option<(Vec<Vec<u8>>, u64)> {
    const END: &[u8] = b" bytes cut]\n";
    let (mut pieces, mut cut, mut from) = (Vec::new(), 0, 0);
    while let Some(open) = find_from(text, b"[", from) {
        let close = find_from(text, END, open)?;
        let digits = std::str::from_utf8(&text[open + 1..close]).ok()?;
        if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
            return None;
        }
        cut += digits.parse::<u64>().ok()?;
        let start = if open > 0 {
            if text[open - 1] != b'\n' {
                return None;
            }
            open - 1
        } else {
            open
        };
        if start > from {
            pieces.push(text[from..start].to_vec());
        }
        from = close + END.len();
    }
    if from < text.len() {
        pieces.push(text[from..].to_vec());
    }
    Some((pieces, cut))
}
