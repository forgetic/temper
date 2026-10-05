//! What the brief's scenarios expect, held by a referee (testing.md,
//! 5.2) that sees what the parent asks for, what the brief asks its sources
//! and what they serve, and what the brief answers, each as the step that
//! made it ends; never the brief's state:
//!
//! - the entrance: a brief past the limits is refused as oversized, one of
//!   no sections is rendered at once, and one with sections is refused as
//!   busy exactly when every brief's place, or the room for its reads (the
//!   reads of briefs that have answered included), is taken; a refusal is
//!   owed a room notice, sent in the step that makes room, and only then;
//! - the reads: one per section, in order, of the section's source (a list
//!   of dependencies cut to what a source may name), with the end, the fit
//!   and the bounds the section's kind calls for;
//! - the answer, as soon as it is due and no sooner: when a required
//!   section's read fails or brings too much, a failure saying so; when
//!   every read has ended, or the deadline has come, a rendering; a section
//!   missing exactly when its read did not bring content within its bounds
//!   in time, and saying why;
//! - every section exactly its content, cut as told: what it holds of each
//!   part, rebuilt with a line wherever bytes were left out (one line for
//!   cuts next to each other, with the count of the bytes left out, the
//!   source's included) and a line for the items cut, is the section,
//!   byte for byte; what it holds follows its kind's order (the first
//!   bytes, the last, an even share, whole lines and the last); and it is
//!   UTF-8;
//! - the budgets: no section over its kind's, all within the brief's;
//!   a section within the even share of the brief's budget is whole, and a
//!   section cut comes as near its bound as its order allows;
//! - every brief asked for is answered within its time to gather.

use std::collections::BTreeMap;

use skein_lib::bytes::find_from;
use skein_lib::{Duration, Time};
use temper_engine_domain_brief::{Body, Fit, Keep, Kind, Limits, Part, Refusal, Section, Source, Unread};
use temper_world::{Expectations, Judge};

/// The longest cut line, and items line: the brief's floor for a budget.
const CUT_LINE: usize = b"\n[".len() + 20 + b" bytes cut]\n".len();
const ITEMS_LINE: usize = b"\n[".len() + 10 + b" items cut]\n".len();
pub const FLOOR: usize = CUT_LINE + ITEMS_LINE;

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The brief takes in the brief `brief` the parent asked for, of these
    /// sections: each one's source, and whether it is required.
    Asked {
        brief: u64,
        sections: Vec<(Source, bool)>,
    },
    /// The brief asked to read the section at `index` of `brief` so.
    Read {
        brief: u64,
        index: u32,
        source: Source,
        keep: Keep,
        fit: Fit,
        parts: u32,
        bytes: u32,
    },
    /// The read of the section at `index` of `brief` reached the brief:
    /// content, or a failure.
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
        why: Unread,
    },
    Refused {
        brief: u64,
        refusal: Refusal,
    },
    /// The brief told its parent there is room again.
    Room,
    /// A step of the brief, or an alarm, has ended, and what it emitted has
    /// been seen.
    Stepped,
}

/// What a read brought.
#[derive(Clone, Debug)]
pub enum Served {
    Content(Vec<Part>),
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
    /// How long past its deadline a brief may take to be answered: the
    /// iteration's slack.
    slack: Duration,
    /// The briefs taken in and not answered.
    taken: BTreeMap<u64, Brief>,
    /// The reads in flight, those of briefs that have answered included.
    reading: u32,
    /// Whether a refusal is owed a room notice.
    owed: bool,
    /// Sections and cut lines judged.
    pub sections: u64,
    pub cuts: u64,
}

#[derive(Debug)]
struct Brief {
    sections: Vec<(Source, bool)>,
    /// When it was taken in, and what the entrance should make of it.
    since: Time,
    entrance: Entrance,
    /// The read asked for each section, and what each brought, in time.
    reads: BTreeMap<u32, Asked>,
    served: BTreeMap<u32, Served>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Entrance {
    Oversized,
    Empty,
    Busy,
    Admitted,
}

#[derive(Debug)]
struct Asked {
    parts: u32,
    bytes: u32,
}

/// What became of a section's read when its brief answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fate {
    Read,
    Unread(Unread),
}

impl Briefs {
    /// Expectations of a brief under `limits`, each answered within its
    /// time to gather and `slack`.
    #[must_use]
    pub fn new(limits: Limits, slack: Duration) -> Briefs {
        Briefs { limits, slack, taken: BTreeMap::new(), reading: 0, owed: false, sections: 0, cuts: 0 }
    }

    fn gathering(&self) -> u32 {
        let admitted = self.taken.values().filter(|brief| brief.entrance == Entrance::Admitted).count();
        u32::try_from(admitted).expect("small")
    }

    /// Whether a brief of sections may be taken in now.
    fn room(&self) -> bool {
        let reads = 2 * self.limits.briefs * self.limits.sections;
        self.gathering() < self.limits.briefs && self.reading + self.limits.sections <= reads
    }

    fn asked(&mut self, brief: u64, sections: Vec<(Source, bool)>, judge: &mut Judge<Expected, Stimulus>) {
        let count = sections.len();
        let entrance = if count > usize::try_from(self.limits.sections).expect("small") {
            Entrance::Oversized
        } else if count == 0 {
            Entrance::Empty
        } else if self.room() {
            Entrance::Admitted
        } else {
            Entrance::Busy
        };
        let since = judge.now();
        let record = Brief { sections, since, entrance, reads: BTreeMap::new(), served: BTreeMap::new() };
        self.taken.insert(brief, record);
        let within = self.limits.gather.saturating_add(self.slack);
        judge.expect(Expected::Answer(brief), within);
    }

    fn read(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        let Seen::Read { brief, index, source, keep, fit, parts, bytes } = seen else { unreachable!("a read") };
        self.reading += 1;
        let Some(record) = self.taken.get_mut(&brief) else {
            return judge.fail(format_args!("brief {brief}: a read for a brief taken in"));
        };
        judge.check(
            record.entrance == Entrance::Admitted,
            format_args!("brief {brief}: reads only for a brief admitted, not {:?}", record.entrance),
        );
        let first = record.reads.insert(index, Asked { parts, bytes }).is_none();
        judge.check(first, format_args!("brief {brief}: section {index} is read once"));
        let Some((asked, _)) = record.sections.get(usize::try_from(index).expect("small")) else {
            return judge.fail(format_args!("brief {brief}: a read for section {index} it has"));
        };
        let expected = bounded(asked, self.limits.items);
        judge.check(source == expected, format_args!("brief {brief}: section {index} is read from its source"));
        let kind = asked.kind();
        let (want_keep, want_fit) = shape(kind);
        let most = match want_fit {
            Fit::Run | Fit::Lines => self.limits.read_bytes.min(budget(&self.limits, kind)),
            Fit::Each => self.limits.read_bytes,
        };
        judge.check(
            (keep, fit, parts, bytes) == (want_keep, want_fit, self.limits.parts, most),
            format_args!(
                "brief {brief}: section {index} of {kind:?} is read as it is cut: {keep:?} {fit:?} {parts} {bytes}"
            ),
        );
    }

    fn served(&mut self, brief: u64, index: u32, read: Served, judge: &mut Judge<Expected, Stimulus>) {
        self.reading = self.reading.checked_sub(1).expect("a read served was asked for");
        // A read that ends after its brief answered is dropped.
        if let Some(record) = self.taken.get_mut(&brief) {
            let first = record.served.insert(index, read).is_none();
            judge.check(first, format_args!("brief {brief}: section {index}'s read ends once"));
        }
    }

    /// Takes out the brief `brief`, which answers.
    fn answered(&mut self, brief: u64, judge: &mut Judge<Expected, Stimulus>) -> Option<Brief> {
        judge.meet(&Expected::Answer(brief));
        let record = self.taken.remove(&brief);
        if record.is_none() {
            judge.fail(format_args!("brief {brief}: answered once, after it was taken in"));
        }
        record
    }

    /// What became of each section's read, with what it brought.
    fn fates(record: &Brief) -> Vec<(Fate, Option<&[Part]>)> {
        let mut fates = Vec::new();
        for index in 0..record.sections.len() {
            let index = u32::try_from(index).expect("small");
            let fate = match record.served.get(&index) {
                None => (Fate::Unread(Unread::Late), None),
                Some(Served::Failed) => (Fate::Unread(Unread::Failed), None),
                Some(Served::Content(parts)) => {
                    let asked = record.reads.get(&index).expect("a read is served once asked for");
                    let bytes: usize = parts.iter().map(|part| part.bytes.len()).sum();
                    let within = parts.len() <= asked.parts as usize && bytes <= asked.bytes as usize;
                    if within { (Fate::Read, Some(parts.as_slice())) } else { (Fate::Unread(Unread::Oversized), None) }
                }
            };
            fates.push(fate);
        }
        fates
    }

    fn rendered(&mut self, brief: u64, sections: &[Section], judge: &mut Judge<Expected, Stimulus>) {
        let Some(record) = self.answered(brief, judge) else {
            return;
        };
        if record.entrance == Entrance::Empty {
            judge.check(sections.is_empty(), format_args!("brief {brief}: a brief of no sections is so rendered"));
            return;
        }
        judge.check(
            record.entrance == Entrance::Admitted,
            format_args!("brief {brief}: rendered only once admitted, not {:?}", record.entrance),
        );
        let kinds: Vec<Kind> = sections.iter().map(|section| section.kind).collect();
        let asked: Vec<Kind> = record.sections.iter().map(|(source, _)| source.kind()).collect();
        if kinds != asked {
            return judge.fail(format_args!("brief {brief}: a section for each asked, in order"));
        }
        let fates = Self::fates(&record);
        let deadline = record.since.saturating_add(self.limits.gather);
        let mut wholes = Vec::new();
        for (index, section) in sections.iter().enumerate() {
            self.sections += 1;
            let (source, required) = &record.sections[index];
            let (fate, parts) = fates[index];
            let whole = match &section.body {
                Body::Missing(why) => {
                    match fate {
                        Fate::Unread(unread) => {
                            judge.check(
                                *why == unread,
                                format_args!("brief {brief}: section {index} is missing as {unread:?}"),
                            );
                            judge.check(!required, format_args!("brief {brief}: a required section is never missing"));
                            if unread == Unread::Late {
                                judge.check(
                                    judge.now() >= deadline,
                                    format_args!(
                                        "brief {brief}: rendered without section {index} only at its deadline"
                                    ),
                                );
                            }
                        }
                        Fate::Read => {
                            judge.fail(format_args!("brief {brief}: section {index} was read in time but is missing"));
                        }
                    }
                    None
                }
                Body::Text(_) => match fate {
                    Fate::Unread(unread) => {
                        judge.fail(format_args!("brief {brief}: section {index} has text, but was {unread:?}"));
                        None
                    }
                    Fate::Read => {
                        let parts = parts.expect("a section read has its parts");
                        let items = cut_items(source, self.limits.items);
                        Some(render(parts, shape(source.kind()).0, &whole_of(parts), items))
                    }
                },
            };
            wholes.push(whole);
        }
        self.budgets(brief, sections, &record, &fates, &wholes, judge);
    }

    /// Judges each section's text against its parts, and the budgets.
    fn budgets(
        &mut self,
        brief: u64,
        sections: &[Section],
        record: &Brief,
        fates: &[(Fate, Option<&[Part]>)],
        wholes: &[Option<Vec<u8>>],
        judge: &mut Judge<Expected, Stimulus>,
    ) {
        let total = self.limits.brief_bytes as usize;
        let bound = share(&self.limits, sections, wholes, total);
        let mut sum = 0;
        for (index, section) in sections.iter().enumerate() {
            let text = match &section.body {
                Body::Text(text) => text,
                Body::Missing(_) => continue,
            };
            let Some(whole) = &wholes[index] else {
                continue;
            };
            let parts = fates[index].1.expect("a section with text has its parts");
            sum += text.len();
            let kind = section.kind;
            let room = budget(&self.limits, kind) as usize;
            judge.check(
                text.len() <= room,
                format_args!("brief {brief}: section {index} is within its budget: {} > {room}", text.len()),
            );
            let source = &record.sections[index].0;
            let items = cut_items(source, self.limits.items);
            if self.text(brief, index, kind, text, parts, items, judge).is_none() {
                continue;
            }
            let bound = room.min(bound);
            if whole.len() <= bound {
                judge.check(
                    **text == **whole,
                    format_args!("brief {brief}: section {index}, within its share {bound}, is whole"),
                );
            } else {
                let slack = slack(kind, parts);
                judge.check(
                    text.len() <= bound && text.len() + slack >= bound,
                    format_args!(
                        "brief {brief}: section {index} of {kind:?}, cut, is near its share: {} of {bound}",
                        text.len()
                    ),
                );
            }
        }
        judge.check(sum <= total, format_args!("brief {brief}: within the brief's budget: {sum} > {total}"));
    }

    /// Judges the text of a section against the parts it was served, and
    /// returns what it holds of each.
    #[expect(clippy::too_many_arguments, reason = "the section's every input")]
    fn text(
        &mut self,
        brief: u64,
        index: usize,
        kind: Kind,
        text: &[u8],
        parts: &[Part],
        items: u32,
        judge: &mut Judge<Expected, Stimulus>,
    ) -> Option<Vec<usize>> {
        judge.check(std::str::from_utf8(text).is_ok(), format_args!("brief {brief}: section {index} is UTF-8"));
        let keep = shape(kind).0;
        let Some(kept) = parse(text, parts, keep, items) else {
            judge.fail(format_args!("brief {brief}: section {index} of {kind:?} is its content, cut as told"));
            return None;
        };
        let rebuilt = render(parts, keep, &kept, items);
        if rebuilt != text {
            judge.fail(format_args!("brief {brief}: section {index} of {kind:?} is its content, cut as told"));
            return None;
        }
        if kept != whole_of(parts) || items > 0 || parts.iter().any(|part| part.left > 0) {
            self.cuts += 1;
        }
        judge.check(
            follows(kind, parts, &kept),
            format_args!("brief {brief}: section {index} of {kind:?} keeps what its order keeps: {kept:?}"),
        );
        Some(kept)
    }

    fn failed(&mut self, brief: u64, missing: Kind, why: Unread, judge: &mut Judge<Expected, Stimulus>) {
        let Some(record) = self.answered(brief, judge) else {
            return;
        };
        judge.check(
            record.entrance == Entrance::Admitted,
            format_args!("brief {brief}: fails only once admitted, not {:?}", record.entrance),
        );
        let deadline = record.since.saturating_add(self.limits.gather);
        let fates = Self::fates(&record);
        let wanting =
            record.sections.iter().zip(&fates).any(|((source, required), (fate, _))| {
                *required && source.kind() == missing && *fate == Fate::Unread(why)
            });
        judge.check(
            wanting,
            format_args!("brief {brief}: it fails only for want of a required section of {missing:?}, {why:?}"),
        );
        if why == Unread::Late {
            judge.check(judge.now() >= deadline, format_args!("brief {brief}: late only at its deadline"));
        }
    }

    fn refused(&mut self, brief: u64, refusal: Refusal, judge: &mut Judge<Expected, Stimulus>) {
        let Some(record) = self.answered(brief, judge) else {
            return;
        };
        let expected = match refusal {
            Refusal::Oversized => Entrance::Oversized,
            Refusal::Busy => Entrance::Busy,
        };
        judge.check(
            record.entrance == expected,
            format_args!("brief {brief}: refused as {refusal:?} only when it is due: {:?}", record.entrance),
        );
        if refusal == Refusal::Busy {
            self.owed = true;
        }
    }

    /// A step ended: whatever was due in it was done.
    fn stepped(&mut self, judge: &mut Judge<Expected, Stimulus>) {
        if self.owed && self.room() {
            judge.fail("a refusal is told of room in the step that makes it");
        }
        for (brief, record) in &self.taken {
            match record.entrance {
                Entrance::Admitted => {}
                entrance @ (Entrance::Oversized | Entrance::Empty | Entrance::Busy) => {
                    judge.fail(format_args!("brief {brief}: answered at the entrance when {entrance:?}"));
                    continue;
                }
            }
            judge.check(
                record.reads.len() == record.sections.len(),
                format_args!("brief {brief}: a read for each section, as it is admitted"),
            );
            let fates = Self::fates(record);
            let all = record.served.len() == record.sections.len();
            let wanting = record.sections.iter().zip(&fates).any(|((_, required), (fate, _))| {
                *required && (*fate == Fate::Unread(Unread::Failed) || *fate == Fate::Unread(Unread::Oversized))
            });
            judge.check(!all && !wanting, format_args!("brief {brief}: answered as soon as it is due"));
        }
    }
}

impl Expectations for Briefs {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Asked { brief, sections } => self.asked(brief, sections, judge),
            seen @ Seen::Read { .. } => self.read(seen, judge),
            Seen::Served { brief, index, read } => self.served(brief, index, read, judge),
            Seen::Rendered { brief, sections } => self.rendered(brief, &sections, judge),
            Seen::Failed { brief, missing, why } => self.failed(brief, missing, why, judge),
            Seen::Refused { brief, refusal } => self.refused(brief, refusal, judge),
            Seen::Room => {
                judge.check(self.owed && self.room(), "room is told only when a refusal is owed it, and there is");
                self.owed = false;
            }
            Seen::Stepped => self.stepped(judge),
        }
    }
}

/// The end a section of `kind` keeps, and how its source fits it.
#[must_use]
pub fn shape(kind: Kind) -> (Keep, Fit) {
    match kind {
        Kind::Task | Kind::Item | Kind::Pull | Kind::Plan | Kind::Template => (Keep::Start, Fit::Run),
        Kind::Comments | Kind::Attempts => (Keep::End, Fit::Run),
        Kind::Dependencies | Kind::Reviews => (Keep::Start, Fit::Each),
        Kind::Ci => (Keep::End, Fit::Each),
        Kind::Notes => (Keep::Start, Fit::Lines),
    }
}

/// The budget of a section of `kind` under `limits`.
#[must_use]
pub fn budget(limits: &Limits, kind: Kind) -> u32 {
    let budgets = &limits.budgets;
    match kind {
        Kind::Task => budgets.task,
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
    }
}

/// `source` as the brief reads it: a list of dependencies cut to what a
/// source may name.
fn bounded(source: &Source, items: u32) -> Source {
    match source {
        Source::Dependencies(named) => Source::Dependencies(named.iter().take(items as usize).copied().collect()),
        Source::Task { .. }
        | Source::Item(_)
        | Source::Comments { .. }
        | Source::Ci { .. }
        | Source::Reviews { .. }
        | Source::Pull { .. }
        | Source::Attempts(_)
        | Source::Plan { .. }
        | Source::Notes { .. }
        | Source::Template(_) => source.clone(),
    }
}

/// How many items of `source` the brief cuts at its entrance.
fn cut_items(source: &Source, items: u32) -> u32 {
    match source {
        Source::Dependencies(named) => u32::try_from(named.len()).expect("small").saturating_sub(items),
        Source::Task { .. }
        | Source::Item(_)
        | Source::Comments { .. }
        | Source::Ci { .. }
        | Source::Reviews { .. }
        | Source::Pull { .. }
        | Source::Attempts(_)
        | Source::Plan { .. }
        | Source::Notes { .. }
        | Source::Template(_) => 0,
    }
}

fn whole_of(parts: &[Part]) -> Vec<usize> {
    parts.iter().map(|part| part.bytes.len()).collect()
}

/// The text of `parts` keeping `kept` bytes of each, at the end `keep`
/// names: a line wherever bytes were left out, one for cuts next to each
/// other, and the line for `items` cut last.
#[must_use]
pub fn render(parts: &[Part], keep: Keep, kept: &[usize], items: u32) -> Vec<u8> {
    let mut text = Vec::new();
    let mut pending = 0;
    for (part, &kept) in parts.iter().zip(kept) {
        let bytes = &part.bytes;
        let held = match keep {
            Keep::Start => &bytes[..kept],
            Keep::End => &bytes[bytes.len() - kept..],
        };
        let lost = part.left + (bytes.len() - kept) as u64;
        if keep == Keep::End {
            pending += lost;
        }
        if !held.is_empty() {
            if pending > 0 {
                line(&mut text, pending, "bytes");
                pending = 0;
            }
            text.extend_from_slice(held);
        }
        if keep == Keep::Start {
            pending += lost;
        }
    }
    if pending > 0 {
        line(&mut text, pending, "bytes");
    }
    if items > 0 {
        line(&mut text, u64::from(items), "items");
    }
    text
}

fn line(text: &mut Vec<u8>, count: u64, unit: &str) {
    if !text.is_empty() {
        text.push(b'\n');
    }
    text.extend_from_slice(format!("[{count} {unit} cut]\n").as_bytes());
}

/// A piece of a section's text: content, or a cut line and its count.
#[derive(Debug)]
enum Token<'a> {
    Content(&'a [u8]),
    Line(u64),
}

/// What a section's `text` holds of each of `parts`, read back from its cut
/// lines as the brief puts them, keeping the end `keep` names, with the
/// line for `items` cut last; `None` if it cannot be so read. The world's
/// content has no `[`.
#[must_use]
pub fn parse(text: &[u8], parts: &[Part], keep: Keep, items: u32) -> Option<Vec<usize>> {
    let text = if items > 0 {
        let line = format!("[{items} items cut]\n");
        let rest = text.strip_suffix(line.as_bytes())?;
        match rest.strip_suffix(b"\n") {
            Some(rest) => rest,
            None if rest.is_empty() => rest,
            None => return None,
        }
    } else {
        text
    };
    let tokens = tokens(text)?;
    match keep {
        Keep::Start => parse_start(&tokens, parts),
        Keep::End => parse_end(&tokens, parts),
    }
}

fn tokens(text: &[u8]) -> Option<Vec<Token<'_>>> {
    const END: &[u8] = b" bytes cut]\n";
    let mut tokens = Vec::new();
    let mut from = 0;
    while let Some(open) = find_from(text, b"[", from) {
        let close = find_from(text, END, open)?;
        let digits = std::str::from_utf8(&text[open + 1..close]).ok()?;
        if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
            return None;
        }
        let count = digits.parse::<u64>().ok()?;
        let start = if open > 0 {
            if text[open - 1] != b'\n' {
                return None;
            }
            open - 1
        } else {
            open
        };
        if start > from {
            tokens.push(Token::Content(&text[from..start]));
        }
        tokens.push(Token::Line(count));
        from = close + END.len();
    }
    if from < text.len() {
        tokens.push(Token::Content(&text[from..]));
    }
    Some(tokens)
}

/// Parts kept from their start: each holds the content's next bytes, and a
/// part cut ends its run of content, the line after it counting its cut and
/// those of the parts after it left out whole.
fn parse_start(tokens: &[Token<'_>], parts: &[Part]) -> Option<Vec<usize>> {
    let mut kept = Vec::new();
    let (mut at, mut offset) = (0, 0);
    // What of the line at hand is not yet counted for.
    let mut owed: Option<u64> = None;
    for part in parts {
        let all = part.bytes.len() as u64 + part.left;
        if let Some(rest) = owed {
            if rest > 0 {
                owed = Some(rest.checked_sub(all)?);
                kept.push(0);
                continue;
            }
            owed = None;
            at += 1;
        }
        match tokens.get(at) {
            Some(Token::Content(content)) => {
                let take = part.bytes.len().min(content.len() - offset);
                if content[offset..offset + take] != part.bytes[..take] {
                    return None;
                }
                offset += take;
                kept.push(take);
                if offset == content.len() {
                    at += 1;
                    offset = 0;
                }
                let cut = all - take as u64;
                if cut > 0 {
                    if offset != 0 {
                        return None;
                    }
                    match tokens.get(at) {
                        Some(Token::Line(count)) => owed = Some(count.checked_sub(cut)?),
                        Some(Token::Content(_)) | None => return None,
                    }
                }
            }
            Some(Token::Line(count)) => {
                owed = Some(count.checked_sub(all)?);
                kept.push(0);
            }
            None => {
                if all > 0 {
                    return None;
                }
                kept.push(0);
            }
        }
    }
    if let Some(rest) = owed {
        if rest > 0 {
            return None;
        }
        at += 1;
    }
    (at == tokens.len() && offset == 0).then_some(kept)
}

/// Parts kept from their end: a part's cut comes before what it holds, and
/// a line counts the cuts up to the next part that holds anything.
fn parse_end(tokens: &[Token<'_>], parts: &[Part]) -> Option<Vec<usize>> {
    let mut kept = Vec::new();
    let (mut at, mut offset) = (0, 0);
    let mut owed: Option<u64> = None;
    for part in parts {
        let all = part.bytes.len() as u64 + part.left;
        if owed.is_none() {
            match tokens.get(at) {
                Some(Token::Line(count)) => owed = Some(*count),
                Some(Token::Content(content)) => {
                    // A part with nothing cut, in the run of content.
                    if part.left > 0 || !content[offset..].starts_with(&part.bytes) {
                        return None;
                    }
                    offset += part.bytes.len();
                    kept.push(part.bytes.len());
                    if offset == content.len() {
                        at += 1;
                        offset = 0;
                    }
                    continue;
                }
                None => {
                    if all > 0 {
                        return None;
                    }
                    kept.push(0);
                    continue;
                }
            }
        }
        let rest = owed.expect("a line is at hand");
        if all <= rest {
            owed = Some(rest - all);
            kept.push(0);
            continue;
        }
        // The line ends with this part's cut, and what it holds follows.
        let take = usize::try_from(all - rest).ok()?;
        if take > part.bytes.len() {
            return None;
        }
        owed = None;
        at += 1;
        let content = match tokens.get(at) {
            Some(Token::Content(content)) => content,
            Some(Token::Line(_)) | None => return None,
        };
        if !content.starts_with(&part.bytes[part.bytes.len() - take..]) {
            return None;
        }
        offset = take;
        kept.push(take);
        if offset == content.len() {
            at += 1;
            offset = 0;
        }
    }
    if let Some(rest) = owed {
        if rest > 0 {
            return None;
        }
        at += 1;
    }
    (at == tokens.len() && offset == 0).then_some(kept)
}

/// Whether `kept` follows the order of `kind`'s section.
fn follows(kind: Kind, parts: &[Part], kept: &[usize]) -> bool {
    let lens = whole_of(parts);
    let whole: Vec<bool> = lens.iter().zip(kept).map(|(len, kept)| kept == len).collect();
    let none: Vec<bool> = kept.iter().map(|kept| *kept == 0).collect();
    match kind {
        // The first bytes: whole parts, then one cut, then none.
        Kind::Task | Kind::Item | Kind::Pull | Kind::Plan | Kind::Template => {
            let edge = whole.iter().position(|whole| !whole).unwrap_or(parts.len());
            none.iter().skip(edge + 1).all(|none| *none)
        }
        Kind::Comments | Kind::Attempts => {
            let edge = whole.iter().rposition(|whole| !whole).map_or(0, |edge| edge + 1);
            none.iter().take(edge.saturating_sub(1)).all(|none| *none)
        }
        // An even share: no part whole past it, none cut far short of it.
        Kind::Dependencies | Kind::Reviews | Kind::Ci => {
            let Some(share) = kept.iter().zip(&whole).filter(|(_, whole)| !**whole).map(|(kept, _)| *kept).max() else {
                return true;
            };
            lens.iter().zip(kept).all(|(len, kept)| if kept == len { *len <= share + 3 } else { *kept + 3 >= share })
        }
        // Whole lines from the first, and the last with any.
        Kind::Notes => {
            let Some((last, lines)) = whole.split_last() else {
                return true;
            };
            let lines_kept = lines.iter().take_while(|whole| **whole).count();
            let all_or_none = whole.iter().zip(&none).all(|(whole, none)| *whole || *none);
            let prefix = none.iter().take(lines.len()).skip(lines_kept).all(|none| *none);
            let any = kept[..lines.len()].iter().any(|kept| *kept > 0);
            all_or_none && prefix && (!any || *last)
        }
    }
}

/// How far short of its bound a cut section of `kind` may come: the most
/// one step more of its measure would add.
fn slack(kind: Kind, parts: &[Part]) -> usize {
    match kind {
        Kind::Task | Kind::Item | Kind::Pull | Kind::Plan | Kind::Template | Kind::Comments | Kind::Attempts => {
            CUT_LINE + 4
        }
        Kind::Dependencies | Kind::Reviews | Kind::Ci => (CUT_LINE + 4) * (parts.len() + 1),
        Kind::Notes => parts.iter().map(|part| part.bytes.len()).max().unwrap_or(0) + CUT_LINE,
    }
}

/// The bound the brief's budget holds each section to: none, if they fit
/// at their own budgets; else the largest under which they fit, each at
/// the least of its whole length, its budget and the bound.
fn share(limits: &Limits, sections: &[Section], wholes: &[Option<Vec<u8>>], total: usize) -> usize {
    let lengths: Vec<(usize, usize)> = sections
        .iter()
        .zip(wholes)
        .filter_map(|(section, whole)| Some((whole.as_ref()?.len(), budget(limits, section.kind) as usize)))
        .collect();
    let asked = |bound: usize| -> usize { lengths.iter().map(|(whole, room)| (*whole).min(*room).min(bound)).sum() };
    if asked(usize::MAX) <= total {
        return usize::MAX;
    }
    let (mut low, mut high) = (FLOOR, lengths.iter().map(|(_, room)| *room).max().unwrap_or(FLOOR).max(FLOOR));
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if asked(mid) <= total {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}
