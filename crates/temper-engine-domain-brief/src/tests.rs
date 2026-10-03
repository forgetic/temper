//! Feed the domain events, inspect the requests that come out; and cut
//! sections and briefs directly.

#![expect(
    clippy::arithmetic_side_effects,
    reason = "the tests count small lengths, and an overflow traps in a test as anywhere"
)]

use alloc::boxed::Box;

use skein_lib::bytes::find_from;
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token, Wall};

use crate::brief::{Content, Slot};
use crate::cut::{self, CUT_LINE, FLOOR, probe};
use crate::limits::{KINDS, budget};
use crate::{
    Body, Budgets, Commit, Domain, Event, Fact, Fit, Gathered, Item, Keep, Kind, Limits, Part, Read, Refusal, Request,
    Section, Source, Unread, Wanted, fire, max_out, step, worst_case,
};

const BUDGETS: Budgets = Budgets {
    item: 64,
    comments: 64,
    dependencies: 96,
    ci: 96,
    reviews: 96,
    pull: 64,
    attempts: 64,
    plan: 64,
    notes: 64,
    template: 64,
};

const LIMITS: Limits = Limits {
    briefs: 2,
    sections: 4,
    items: 3,
    parts: 4,
    read_bytes: 256,
    budgets: BUDGETS,
    brief_bytes: 400,
    gather: Duration::from_secs(10),
    facts: 64,
};

const ISSUE: Item = Item { repository: 0, number: 7 };
const HEAD: Commit = Commit([9; 32]);

/// The domain, its environment, and room for one step's output.
struct Harness {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    calls: u64,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        let out = Queue::with_capacity(max_out(&limits));
        Harness { domain: Domain::new(&limits), env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits }, out, calls: 0 }
    }

    /// Asks for a brief of `sections`, returning its reply token and what the
    /// step emitted.
    fn render(&mut self, sections: &[(Source, bool)]) -> (Token, Box<[Request]>) {
        self.calls += 1;
        let token = Token::new(self.calls);
        let mut wanted = List::with_capacity(u32::try_from(sections.len()).unwrap());
        for (source, required) in sections {
            wanted.push(Wanted { source: source.clone(), required: *required }).unwrap();
        }
        let requests = self.step(Event::Render { reply_to: ReplyTo::new(token), sections: wanted.into_boxed() });
        (token, requests)
    }

    /// Answers the read `owner` with `parts`, none of them cut by the source.
    fn got(&mut self, owner: Token, parts: &[&[u8]]) -> Box<[Request]> {
        self.step(Event::Read { owner, read: Read::Got(parts_of(parts)) })
    }

    fn failed(&mut self, owner: Token) -> Box<[Request]> {
        self.step(Event::Read { owner, read: Read::Failed })
    }

    /// Steps `event`, returning what it emitted, oldest first.
    fn step(&mut self, event: Event) -> Box<[Request]> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Fires what is due at `now`, returning what it emitted.
    fn fire_at(&mut self, now: Time) -> Box<[Request]> {
        self.env.now = now;
        while self.domain.is_due(now) {
            fire(&mut self.domain, &self.env, &mut self.out);
        }
        self.drain()
    }

    fn drain(&mut self) -> Box<[Request]> {
        let mut requests = List::with_capacity(self.out.len());
        for _ in 0..self.out.len() {
            requests.push(self.out.pop().unwrap()).unwrap();
        }
        // The iteration ends: the reclaim point.
        self.domain.reclaim();
        requests.into_boxed()
    }

    fn facts(&mut self) -> Box<[Fact]> {
        let mut facts = List::with_capacity(LIMITS.facts);
        while let Some(fact) = self.domain.pop_fact() {
            facts.push(fact).unwrap();
        }
        facts.into_boxed()
    }
}

fn parts_of(parts: &[&[u8]]) -> Box<[Part]> {
    let mut list = List::with_capacity(u32::try_from(parts.len()).unwrap());
    for bytes in parts {
        list.push(Part { bytes: Box::from(*bytes), left: 0 }).unwrap();
    }
    list.into_boxed()
}

/// The owner of each read in `requests`, checking each reads `kinds`'
/// sources in order, cut as its section is.
fn reads(requests: &[Request], kinds: &[Kind]) -> Box<[Token]> {
    assert_eq!(requests.len(), kinds.len(), "a read per section: {requests:?}");
    let mut owners = List::with_capacity(u32::try_from(kinds.len()).unwrap());
    for (request, kind) in requests.iter().zip(kinds) {
        let Request::Read { owner, source, keep, fit, parts, bytes } = request else {
            panic!("a read: {request:?}");
        };
        let (expected_keep, expected_fit, expected_bytes) = shape(*kind);
        assert_eq!((source.kind(), *keep, *fit), (*kind, expected_keep, expected_fit), "{request:?}");
        assert_eq!((*parts, *bytes), (LIMITS.parts, expected_bytes), "{kind:?}: within the limits");
        owners.push(*owner).unwrap();
    }
    owners.into_boxed()
}

/// How a section of `kind` is read under the test limits: the end kept, the
/// fit, and the bytes, the kind's budget unless it shares evenly.
fn shape(kind: Kind) -> (Keep, Fit, u32) {
    let most = LIMITS.read_bytes.min(budget(&BUDGETS, kind));
    match kind {
        Kind::Item | Kind::Pull | Kind::Plan | Kind::Template => (Keep::Start, Fit::Run, most),
        Kind::Comments | Kind::Attempts => (Keep::End, Fit::Run, most),
        Kind::Dependencies | Kind::Reviews => (Keep::Start, Fit::Each, LIMITS.read_bytes),
        Kind::Ci => (Keep::End, Fit::Each, LIMITS.read_bytes),
        Kind::Notes => (Keep::Start, Fit::Lines, most),
    }
}

/// The sections of the one answer in `requests`, a rendered brief for
/// `token`.
fn rendered(requests: Box<[Request]>, token: Token) -> Box<[Section]> {
    assert_eq!(requests.len(), 1, "one answer: {requests:?}");
    let Some(Request::Rendered { reply_to, sections }) = requests.into_iter().next() else {
        panic!("a rendered brief");
    };
    assert_eq!(reply_to, ReplyTo::new(token));
    sections
}

fn text(section: &Section) -> &[u8] {
    match &section.body {
        Body::Text(bytes) => bytes,
        Body::Missing(_) => panic!("a section with text: {section:?}"),
    }
}

/// A source of every kind.
fn source(kind: Kind) -> Source {
    match kind {
        Kind::Item => Source::Item(ISSUE),
        Kind::Comments => Source::Comments { item: ISSUE, since: 3 },
        Kind::Dependencies => Source::Dependencies(Box::from([Item { repository: 0, number: 2 }])),
        Kind::Ci => Source::Ci { item: ISSUE, head: HEAD },
        Kind::Reviews => Source::Reviews { item: ISSUE, head: HEAD },
        Kind::Pull => Source::Pull { item: ISSUE, head: HEAD },
        Kind::Attempts => Source::Attempts(ISSUE),
        Kind::Plan => Source::Plan { goal: Item { repository: 0, number: 1 } },
        Kind::Notes => Source::Notes { repository: 1, goal: Some(Item { repository: 0, number: 1 }) },
        Kind::Template => Source::Template(0),
    }
}

fn bytes(len: usize, byte: u8) -> Box<[u8]> {
    let mut list = List::with_capacity(u32::try_from(len).unwrap());
    for _ in 0..len {
        list.push(byte).unwrap();
    }
    list.into_boxed()
}

/// A section's text, read back: how many bytes it kept, and how many its cut
/// lines say were left out. The test content has no `[`.
fn read_back(text: &[u8]) -> (usize, u64) {
    let (mut lines, mut cut) = (0, 0_u64);
    let mut from = 0;
    while let Some(open) = find_from(text, b"[", from) {
        let close = find_from(text, b" bytes cut]\n", open).expect("a cut line is whole");
        let mut count = 0_u64;
        for digit in &text[open + 1..close] {
            assert!(digit.is_ascii_digit(), "a count is digits");
            count = count * 10 + u64::from(digit - b'0');
        }
        cut += count;
        let end = close + b" bytes cut]\n".len();
        // A line after what the section holds starts on a line of its own.
        let start = if open > 0 { open - 1 } else { open };
        assert!(open == 0 || text[open - 1] == b'\n', "a cut line is on a line of its own");
        lines += end - start;
        from = end;
    }
    (text.len() - lines, cut)
}

#[expect(clippy::disallowed_methods, reason = "a test checks the text is still UTF-8")]
fn is_utf8(text: &[u8]) -> bool {
    core::str::from_utf8(text).is_ok()
}

// Cutting a section.

#[test]
fn a_section_that_fits_is_its_parts_one_after_another() {
    for kind in KINDS {
        let parts = parts_of(&[b"the first\n", b"", b"the second\n"]);
        let (text, whole) = cut::section(kind, &parts, 0, 21);
        assert_eq!((&*text, whole), (&b"the first\nthe second\n"[..], true), "{kind:?}");
        let (text, whole) = cut::section(kind, &parts, 0, 20);
        assert!(!whole && text.len() <= 20, "{kind:?}: one byte short is cut: {text:?}");
    }
}

#[test]
fn first_keeps_the_start_and_cuts_the_tail() {
    let parts = parts_of(&[&bytes(40, b'a'), &bytes(40, b'b')]);
    let (text, whole) = cut::section(Kind::Item, &parts, 0, 64);
    let mut expected = [b'a'; 48];
    expected[40..].fill(b'b');
    assert!(!whole);
    assert_eq!(&text[..48], &expected[..]);
    assert_eq!(&text[48..], b"\n[32 bytes cut]\n");
    assert_eq!(text.len(), 64, "the cut line counts in the budget");
}

#[test]
fn last_keeps_the_end_and_cuts_the_oldest() {
    let parts = parts_of(&[&bytes(40, b'a'), &bytes(40, b'b')]);
    for kind in [Kind::Comments, Kind::Attempts] {
        let (text, _) = cut::section(kind, &parts, 0, 64);
        assert_eq!(&text[..15], b"[31 bytes cut]\n", "{kind:?}: a line first has no newline before it");
        assert_eq!(&text[15..24], &[b'a'; 9][..], "{kind:?}: the head of the oldest kept is cut");
        assert_eq!(&text[24..], &[b'b'; 40][..], "{kind:?}");
    }
    // The oldest part goes whole before the newest is cut.
    let parts = parts_of(&[&bytes(40, b'a'), &bytes(80, b'b')]);
    let (text, _) = cut::section(Kind::Comments, &parts, 0, 64);
    assert_eq!(&text[..15], b"[71 bytes cut]\n");
    assert_eq!(&text[15..], &[b'b'; 49][..]);
}

#[test]
fn even_gives_each_part_a_share_and_keeps_its_start() {
    let parts = parts_of(&[&bytes(100, b'a'), &bytes(10, b'b'), &bytes(100, b'c')]);
    for kind in [Kind::Dependencies, Kind::Reviews] {
        let (text, _) = cut::section(kind, &parts, 0, 96);
        let line = b"\n[73 bytes cut]\n";
        assert_eq!(&text[..27], &[b'a'; 27][..], "{kind:?}");
        assert_eq!(&text[27..43], line, "{kind:?}");
        assert_eq!(&text[43..53], &[b'b'; 10][..], "{kind:?}: a short part is whole");
        assert_eq!(&text[53..80], &[b'c'; 27][..], "{kind:?}");
        assert_eq!(&text[80..], line, "{kind:?}");
    }
}

#[test]
fn ci_gives_each_failed_check_a_share_and_keeps_the_end_of_its_output() {
    let mut first = bytes(100, b'a');
    first[99] = b'!';
    let parts = parts_of(&[&first, &bytes(100, b'b')]);
    let (text, _) = cut::section(Kind::Ci, &parts, 0, 96);
    assert_eq!(&text[..15], b"[68 bytes cut]\n");
    assert_eq!(&text[15..46], &[b'a'; 31][..]);
    assert_eq!(text[46], b'!', "the end of the output, where the failure shows");
    assert_eq!(&text[47..63], b"\n[68 bytes cut]\n");
    assert_eq!(&text[63..], &[b'b'; 32][..]);
}

#[test]
fn what_the_source_cut_is_told_where_it_was_and_adjacent_cuts_are_one_line() {
    let parts = [
        Part { bytes: Box::from(&b"abc"[..]), left: 5 },
        Part { bytes: Box::from(&b"def"[..]), left: 0 },
        Part { bytes: Box::from(&b""[..]), left: 7 },
    ];
    let (text, whole) = cut::section(Kind::Item, &parts, 0, 64);
    assert_eq!((&*text, whole), (&b"abc\n[5 bytes cut]\ndef\n[7 bytes cut]\n"[..], false));
    let (text, _) = cut::section(Kind::Comments, &parts, 0, 64);
    assert_eq!(&*text, b"[5 bytes cut]\nabcdef\n[7 bytes cut]\n", "the source cut each part's head");
    // A part cut to nothing joins the cuts around it.
    let parts = [Part { bytes: Box::from(&b"abc"[..]), left: 5 }, Part { bytes: bytes(60, b'x'), left: 0 }];
    let (text, _) = cut::section(Kind::Item, &parts, 0, 39);
    assert_eq!(&*text, b"abc\n[5 bytes cut]\nxxxxx\n[55 bytes cut]\n");
    let (text, _) = cut::section(Kind::Item, &parts, 0, 34);
    assert_eq!(&*text, b"abc\n[65 bytes cut]\n", "a byte of the second part would need a line of its own");
}

#[test]
fn a_cut_never_splits_a_utf8_sequence() {
    // Two-, three- and four-byte sequences, cut at every length.
    let mut content = List::with_capacity(72);
    for _ in 0..8_u32 {
        for byte in "é→𝄞".as_bytes() {
            content.push(*byte).unwrap();
        }
    }
    let content = content.into_boxed();
    let parts = parts_of(&[&content, &content]);
    for kind in KINDS {
        for budget in CUT_LINE..content.len() * 2 + 2 {
            let (text, _) = cut::section(kind, &parts, 0, budget);
            assert!(text.len() <= budget && is_utf8(&text), "{kind:?} within {budget}: {text:?}");
            let (kept, cut) = read_back(&text);
            assert_eq!(u64::try_from(kept).unwrap() + cut, u64::try_from(content.len() * 2).unwrap());
        }
    }
}

#[test]
fn cuts_at_and_around_every_budget_say_how_much_they_left_out() {
    let contents: [&[&[u8]]; 4] = [
        &[],
        &[b"one line\n"],
        &[b"a first part\n", b"a second, longer part\n", b"3\n"],
        &[&[b'x'; 120], b"short\n", &[b'y'; 77]],
    ];
    for kind in KINDS {
        for content in contents {
            let parts = parts_of(content);
            let mut total = 0;
            for part in content {
                total += part.len();
            }
            for budget in CUT_LINE..total + 3 {
                let (text, whole) = cut::section(kind, &parts, 0, budget);
                assert!(text.len() <= budget, "{kind:?} within {budget}: {} bytes", text.len());
                let (kept, cut) = read_back(&text);
                assert_eq!(kept + usize::try_from(cut).unwrap(), total, "{kind:?} within {budget}: {text:?}");
                assert_eq!(whole, budget >= total, "{kind:?} within {budget}: whole once it fits");
                if whole {
                    assert_eq!(text.len(), total, "{kind:?} within {budget}: passed through");
                }
            }
        }
    }
}

#[test]
fn a_section_cut_to_nothing_is_one_cut_line() {
    let parts = parts_of(&[&bytes(200, b'a'), &bytes(56, b'b')]);
    for kind in KINDS {
        let (text, _) = cut::section(kind, &parts, 0, CUT_LINE);
        assert!(text.len() <= CUT_LINE, "{kind:?}");
        assert_eq!(read_back(&text).1, 256 - u64::try_from(read_back(&text).0).unwrap(), "{kind:?}");
    }
    let (text, _) = cut::section(Kind::Item, &parts_of(&[&bytes(256, b'a')]), 0, 17);
    assert_eq!(&*text, b"[256 bytes cut]\n", "a byte kept would not leave room for its line");
}

// Cutting a brief.

fn slot(kind: Kind, parts: &[&[u8]]) -> Slot {
    Slot { kind, required: false, content: Content::Got(parts_of(parts)), items: 0 }
}

fn missing(kind: Kind) -> Slot {
    Slot { kind, required: false, content: Content::Missing(Unread::Failed), items: 0 }
}

#[test]
fn a_brief_within_its_budget_keeps_every_section_whole() {
    let slots = [slot(Kind::Item, &[b"the item\n"]), missing(Kind::Ci), slot(Kind::Notes, &[b"a note\n"])];
    let brief = cut::brief(&slots, &LIMITS);
    assert_eq!((brief.cut, brief.missing), (0, 1));
    let kinds: [Kind; 3] = [brief.sections[0].kind, brief.sections[1].kind, brief.sections[2].kind];
    assert_eq!(kinds, [Kind::Item, Kind::Ci, Kind::Notes], "in the order asked");
    assert_eq!(text(&brief.sections[0]), b"the item\n");
    assert_eq!(brief.sections[1].body, Body::Missing(Unread::Failed));
    assert_eq!(text(&brief.sections[2]), b"a note\n");
}

#[test]
fn a_brief_over_its_budget_holds_its_long_sections_to_an_even_share() {
    let limits = Limits { brief_bytes: 120, ..LIMITS };
    let slots = [slot(Kind::Item, &[&bytes(80, b'a')]), slot(Kind::Comments, &[&bytes(80, b'b')])];
    let brief = cut::brief(&slots, &limits);
    assert_eq!(brief.cut, 2);
    let item = text(&brief.sections[0]);
    let comments = text(&brief.sections[1]);
    assert_eq!((item.len(), comments.len()), (60, 60), "an even share each");
    assert_eq!(&item[44..], b"\n[36 bytes cut]\n");
    assert_eq!(&comments[..15], b"[35 bytes cut]\n");
    // A short section stays whole, and the long one takes what is left.
    let limits = Limits { brief_bytes: 174, budgets: Budgets { comments: 256, ..BUDGETS }, ..LIMITS };
    let slots = [slot(Kind::Item, &[&bytes(20, b'a')]), slot(Kind::Comments, &[&bytes(300, b'b')]), missing(Kind::Ci)];
    let brief = cut::brief(&slots, &limits);
    assert_eq!(text(&brief.sections[0]), &bytes(20, b'a')[..]);
    assert_eq!(text(&brief.sections[1]).len(), 154);
    assert_eq!((brief.cut, brief.missing), (1, 1));
}

#[test]
fn a_brief_is_within_its_budget_whatever_its_sections_hold() {
    let limits = Limits { brief_bytes: 4 * u32::try_from(FLOOR).unwrap(), ..LIMITS };
    let long = bytes(250, b'z');
    for kind in KINDS {
        let slots = [slot(kind, &[&long]), slot(Kind::Item, &[&long]), slot(Kind::Ci, &[b"ok"]), slot(kind, &[])];
        let brief = cut::brief(&slots, &limits);
        let mut total = 0;
        for (section, held) in brief.sections.iter().zip([250, 250, 2, 0]) {
            let text = text(section);
            total += text.len();
            let (kept, cut) = read_back(text);
            assert_eq!(kept + usize::try_from(cut).unwrap(), held, "{kind:?}");
        }
        assert!(total <= 4 * FLOOR, "{kind:?}: {total}");
    }
}

// Gathering a brief.

#[test]
fn a_brief_reads_every_section_at_once_and_is_rendered_in_order() {
    let mut harness = Harness::new(LIMITS);
    let sections = [(source(Kind::Item), true), (source(Kind::Comments), false), (source(Kind::Notes), false)];
    let (token, requests) = harness.render(&sections);
    let owners = reads(&requests, &[Kind::Item, Kind::Comments, Kind::Notes]);
    assert_eq!(harness.domain.next_deadline(), Some(Time::ZERO.saturating_add(LIMITS.gather)));
    assert!(harness.got(owners[2], &[b"a note\n"]).is_empty());
    assert!(harness.got(owners[0], &[b"the item\n", b"its parent\n"]).is_empty());
    let requests = harness.got(owners[1], &[]);
    let sections = rendered(requests, token);
    assert_eq!(text(&sections[0]), b"the item\nits parent\n");
    assert_eq!((sections[1].kind, text(&sections[1])), (Kind::Comments, &b""[..]));
    assert_eq!(text(&sections[2]), b"a note\n");
    assert_eq!((harness.domain.briefs(), harness.domain.reads(), harness.domain.next_deadline()), (0, 0, None));
    let facts = harness.facts();
    assert_eq!(facts[0], Fact::Gathering { sections: 3 });
    assert_eq!(facts[1..4], [Fact::Read { read: Gathered::Got }; 3]);
    assert_eq!(facts[4], Fact::Rendered { sections: 3, cut: 0, missing: 0 });
}

#[test]
fn every_kind_is_read_from_its_source_keeping_its_end() {
    let mut harness = Harness::new(LIMITS);
    for kind in KINDS {
        let (token, requests) = harness.render(&[(source(kind), false)]);
        let [Request::Read { source: asked, .. }] = &*requests else { panic!("{requests:?}") };
        assert_eq!(*asked, source(kind));
        let owners = reads(&requests, &[kind]);
        let requests = harness.got(owners[0], &[b"content"]);
        let sections = rendered(requests, token);
        assert_eq!((sections[0].kind, text(&sections[0])), (kind, &b"content"[..]));
    }
}

#[test]
fn a_brief_of_no_sections_is_rendered_at_once() {
    let mut harness = Harness::new(LIMITS);
    let (token, requests) = harness.render(&[]);
    assert!(rendered(requests, token).is_empty());
    assert_eq!(harness.domain.briefs(), 0);
}

#[test]
fn an_optional_section_that_cannot_be_read_is_missing() {
    let mut harness = Harness::new(LIMITS);
    let (token, requests) = harness.render(&[(source(Kind::Item), true), (source(Kind::Notes), false)]);
    let owners = reads(&requests, &[Kind::Item, Kind::Notes]);
    assert!(harness.failed(owners[1]).is_empty());
    let sections = rendered(harness.got(owners[0], &[b"item"]), token);
    assert_eq!((text(&sections[0]), &sections[1].body), (&b"item"[..], &Body::Missing(Unread::Failed)));
}

#[test]
fn an_answer_past_what_a_read_may_bring_counts_as_failed() {
    let mut harness = Harness::new(LIMITS);
    let (token, requests) = harness.render(&[(source(Kind::Comments), false), (source(Kind::Ci), false)]);
    let owners = reads(&requests, &[Kind::Comments, Kind::Ci]);
    assert!(harness.got(owners[0], &[b"1", b"2", b"3", b"4", b"5"]).is_empty(), "a part too many");
    let sections = rendered(harness.got(owners[1], &[&bytes(257, b'x')]), token);
    let oversized = Body::Missing(Unread::Oversized);
    assert_eq!((&sections[0].body, &sections[1].body), (&oversized, &oversized));
    let facts = harness.facts();
    assert_eq!(facts[1..3], [Fact::Read { read: Gathered::Oversized }; 2]);
    // At the bounds, an answer is taken.
    let (token, requests) = harness.render(&[(source(Kind::Ci), false)]);
    let owners = reads(&requests, &[Kind::Ci]);
    let sections = rendered(harness.got(owners[0], &[&bytes(128, b'x'), b"", b"", &bytes(128, b'y')]), token);
    assert!(text(&sections[0]).len() <= 96);
}

#[test]
fn a_required_section_that_cannot_be_read_fails_the_brief_at_once() {
    let mut harness = Harness::new(LIMITS);
    let (token, requests) = harness.render(&[(source(Kind::Ci), true), (source(Kind::Item), false)]);
    let owners = reads(&requests, &[Kind::Ci, Kind::Item]);
    let requests = harness.failed(owners[0]);
    let failed = Request::Failed { reply_to: ReplyTo::new(token), missing: Kind::Ci, why: Unread::Failed };
    assert_eq!(&*requests, &[failed]);
    assert_eq!(harness.domain.next_deadline(), None, "an answered brief has no deadline");
    assert_eq!((harness.domain.briefs(), harness.domain.reads()), (0, 1), "retired, its read orphaned");
    assert!(harness.got(owners[1], &[b"late"]).is_empty(), "a late read is dropped");
    assert_eq!((harness.domain.briefs(), harness.domain.reads()), (0, 0));
    let facts = harness.facts();
    assert_eq!(facts[2], Fact::Failed { missing: Kind::Ci });
    assert_eq!(facts[3], Fact::Read { read: Gathered::Late });
}

#[test]
fn past_its_deadline_a_brief_is_rendered_with_what_it_has() {
    let mut harness = Harness::new(LIMITS);
    let sections = [(source(Kind::Item), true), (source(Kind::Comments), false), (source(Kind::Plan), false)];
    let (token, requests) = harness.render(&sections);
    let owners = reads(&requests, &[Kind::Item, Kind::Comments, Kind::Plan]);
    assert!(harness.got(owners[0], &[b"item"]).is_empty());
    let deadline = Time::ZERO.saturating_add(LIMITS.gather);
    assert!(harness.fire_at(Time::from_nanos(deadline.as_nanos() - 1)).is_empty(), "not yet");
    let sections = rendered(harness.fire_at(deadline), token);
    assert_eq!(text(&sections[0]), b"item");
    let late = Body::Missing(Unread::Late);
    assert_eq!((&sections[1].body, &sections[2].body), (&late, &late));
    assert!(harness.got(owners[2], &[b"plan"]).is_empty());
    assert_eq!((harness.domain.briefs(), harness.domain.reads()), (0, 1), "one read is still in flight");
    assert!(harness.failed(owners[1]).is_empty());
    assert_eq!((harness.domain.briefs(), harness.domain.reads()), (0, 0));
    let facts = harness.facts();
    assert!(facts.contains(&Fact::Expired { waiting: 2 }));
    assert!(facts.contains(&Fact::Rendered { sections: 3, cut: 0, missing: 2 }));
}

#[test]
fn past_its_deadline_a_brief_without_a_required_section_fails() {
    let mut harness = Harness::new(LIMITS);
    let (token, requests) = harness.render(&[(source(Kind::Item), false), (source(Kind::Reviews), true)]);
    let owners = reads(&requests, &[Kind::Item, Kind::Reviews]);
    assert!(harness.got(owners[0], &[b"item"]).is_empty());
    let requests = harness.fire_at(Time::ZERO.saturating_add(LIMITS.gather));
    let failed = Request::Failed { reply_to: ReplyTo::new(token), missing: Kind::Reviews, why: Unread::Late };
    assert_eq!(&*requests, &[failed]);
    assert!(harness.got(owners[1], &[b"late"]).is_empty());
    assert_eq!(harness.domain.briefs(), 0);
}

#[test]
fn a_read_that_ends_as_the_deadline_falls_due_wins() {
    let mut harness = Harness::new(LIMITS);
    let (token, requests) = harness.render(&[(source(Kind::Item), true)]);
    let owners = reads(&requests, &[Kind::Item]);
    // The read arrives in the iteration the deadline falls due: inputs
    // first, then alarms.
    harness.env.now = Time::ZERO.saturating_add(LIMITS.gather);
    assert!(harness.domain.is_due(harness.env.now));
    let sections = rendered(harness.got(owners[0], &[b"item"]), token);
    assert_eq!(text(&sections[0]), b"item");
    assert!(!harness.domain.is_due(harness.env.now), "its deadline went with it");
}

#[test]
fn a_brief_of_more_sections_than_it_may_have_is_refused() {
    let mut harness = Harness::new(LIMITS);
    let item = (source(Kind::Item), false);
    let five = [item.clone(), item.clone(), item.clone(), item.clone(), item];
    let (token, requests) = harness.render(&five);
    assert_eq!(&*requests, &refused(token, Refusal::Oversized));
    assert_eq!(&*harness.facts(), &[Fact::Refused { refusal: Refusal::Oversized }]);
}

#[test]
fn a_list_of_more_items_than_a_source_may_name_is_cut_and_told() {
    let mut harness = Harness::new(LIMITS);
    let named: Box<[Item]> = Box::from([1, 2, 3, 4, 5].map(|number| Item { repository: 0, number }));
    let (token, requests) = harness.render(&[(Source::Dependencies(named), false)]);
    let [Request::Read { source: Source::Dependencies(asked), .. }] = &*requests else { panic!("{requests:?}") };
    assert_eq!(**asked, [1, 2, 3].map(|number| Item { repository: 0, number }), "the first three");
    let owners = reads(&requests, &[Kind::Dependencies]);
    let sections = rendered(harness.got(owners[0], &[b"one\n", b"two\n", b"three\n"]), token);
    assert_eq!(text(&sections[0]), b"one\ntwo\nthree\n\n[2 items cut]\n");
    let facts = harness.facts();
    assert_eq!(facts[2], Fact::Rendered { sections: 1, cut: 1, missing: 0 });
    // Cut to nothing, the section still says so.
    let (text, whole) = cut::section(Kind::Dependencies, &parts_of(&[&bytes(300, b'a')]), 2, FLOOR);
    assert!(!whole && text.len() <= FLOOR);
    assert_eq!(&text[..26], &bytes(26, b'a')[..]);
    assert_eq!(&text[26..], b"\n[274 bytes cut]\n\n[2 items cut]\n");
    let (text, _) = cut::section(Kind::Dependencies, &parts_of(&[&bytes(300, b'a')]), 2, 31);
    assert_eq!(&*text, b"[300 bytes cut]\n\n[2 items cut]\n", "cut to nothing, it still says so");
}

#[test]
fn a_brief_is_busy_while_every_brief_gathers_and_its_parent_is_told_when_it_is_not() {
    let mut harness = Harness::new(LIMITS);
    let (_, first) = harness.render(&[(source(Kind::Item), false)]);
    let (_, second) = harness.render(&[(source(Kind::Item), true), (source(Kind::Notes), false)]);
    let (token, requests) = harness.render(&[(source(Kind::Item), false)]);
    assert_eq!(&*requests, &refused(token, Refusal::Busy));
    let (token, requests) = harness.render(&[(source(Kind::Item), false)]);
    assert_eq!(&*requests, &refused(token, Refusal::Busy), "busy again: one notice will do for both");
    let (token, requests) = harness.render(&[]);
    assert_eq!(rendered(requests, token).len(), 0, "a brief of no sections needs no room");
    let (first, second) = (reads(&first, &[Kind::Item]), reads(&second, &[Kind::Item, Kind::Notes]));
    let requests = harness.failed(second[0]);
    let failed = Request::Failed { reply_to: ReplyTo::new(Token::new(2)), missing: Kind::Item, why: Unread::Failed };
    assert_eq!(&*requests, &[failed, Request::Room]);
    // The room is there at once, in the same iteration.
    let (_, requests) = harness.render(&[(source(Kind::Item), false)]);
    let third = reads(&requests, &[Kind::Item]);
    assert_eq!(harness.got(first[0], &[b"item"]).len(), 1, "no notice owed: nothing was refused since");
    assert_eq!(harness.got(third[0], &[b"item"]).len(), 1);
    assert!(harness.got(second[1], &[b"late"]).is_empty());
}

#[test]
fn reads_that_outlive_their_briefs_hold_their_room_until_they_end() {
    // Room for two briefs' reads of two sections, and one brief at once.
    let limits = Limits { briefs: 1, sections: 2, ..LIMITS };
    let mut harness = Harness::new(limits);
    let mut orphans = List::with_capacity(3);
    for _ in 0..3_u32 {
        let (_, requests) = harness.render(&[(source(Kind::Ci), true), (source(Kind::Item), false)]);
        let owners = reads(&requests, &[Kind::Ci, Kind::Item]);
        assert_eq!(harness.failed(owners[0]).len(), 1, "it fails, its other read orphaned");
        orphans.push(owners[1]).unwrap();
    }
    assert_eq!((harness.domain.briefs(), harness.domain.reads()), (0, 3));
    let (token, requests) = harness.render(&[(source(Kind::Item), false)]);
    assert_eq!(&*requests, &refused(token, Refusal::Busy), "three reads, and room for four");
    assert_eq!(&*harness.got(orphans.as_slice()[0], &[b"late"]), &[Request::Room]);
    let (_, requests) = harness.render(&[(source(Kind::Item), false), (source(Kind::Plan), false)]);
    assert_eq!(reads(&requests, &[Kind::Item, Kind::Plan]).len(), 2);
    let facts = harness.facts();
    assert!(facts.contains(&Fact::Read { read: Gathered::Late }));
}

fn refused(token: Token, refusal: Refusal) -> [Request; 1] {
    [Request::Refused { reply_to: ReplyTo::new(token), refusal }]
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bound = worst_case(&LIMITS).expect("the test limits fit");
    let held = u64::from(LIMITS.briefs * LIMITS.sections * LIMITS.read_bytes);
    assert!(bound > held, "it counts every brief with every section read in full");
    let more = worst_case(&Limits { briefs: 6, ..LIMITS }).expect("fits");
    assert!(more > bound, "a brief more is more");
    let wider = worst_case(&Limits { read_bytes: 4096, ..LIMITS }).expect("fits");
    assert!(wider > bound, "what a read may bring counts");
    let longer = worst_case(&Limits { brief_bytes: 1 << 20, ..LIMITS }).expect("fits");
    assert!(longer > bound, "a brief being rendered counts");
    assert_eq!(worst_case(&Limits { briefs: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { sections: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { parts: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { gather: Duration::from_nanos(0), ..LIMITS }), None);
    let floor = u32::try_from(FLOOR).unwrap();
    let small = Budgets { notes: floor - 1, ..BUDGETS };
    assert_eq!(worst_case(&Limits { budgets: small, ..LIMITS }), None, "a budget that cannot say what it cut");
    assert!(worst_case(&Limits { budgets: Budgets { notes: floor, ..BUDGETS }, ..LIMITS }).is_some());
    assert_eq!(worst_case(&Limits { brief_bytes: 4 * floor - 1, ..LIMITS }), None, "the lines in each section");
    assert!(worst_case(&Limits { brief_bytes: 4 * floor, ..LIMITS }).is_some());
    assert_eq!(worst_case(&Limits { briefs: u32::MAX, sections: u32::MAX, ..LIMITS }), None);
}

#[test]
fn notes_are_cut_by_whole_lines_and_keep_their_last_part() {
    let parts =
        parts_of(&[b"- build: how to build\n", b"- flaky: a test that fails\n", b"- style: tabs\n", b"3 more\n"]);
    assert!(cut::section(Kind::Notes, &parts, 0, 70).1, "70 bytes in all");
    let (text, whole) = cut::section(Kind::Notes, &parts, 0, 69);
    assert!(!whole);
    assert_eq!(&*text, b"- build: how to build\n\n[41 bytes cut]\n3 more\n");
    let (text, _) = cut::section(Kind::Notes, &parts, 0, 44);
    assert_eq!(&*text, b"[63 bytes cut]\n3 more\n", "the count of what did not fit stays");
    let (text, _) = cut::section(Kind::Notes, &parts, 0, 21);
    assert_eq!(&*text, b"[70 bytes cut]\n", "unless it does not fit with the line");
    // An index that all fits ends with an empty part.
    let parts = parts_of(&[b"- build: how to build\n", b""]);
    assert_eq!(cut::section(Kind::Notes, &parts, 0, 64), (Box::from(&b"- build: how to build\n"[..]), true));
}

#[test]
fn an_even_share_holds_after_the_source_cut_each_part() {
    // A source that cut each failed check to its end, and one it left whole.
    let parts = [
        Part { bytes: bytes(80, b'a'), left: 900 },
        Part { bytes: bytes(10, b'b'), left: 0 },
        Part { bytes: bytes(80, b'c'), left: 1200 },
    ];
    let (text, whole) = cut::section(Kind::Ci, &parts, 0, 120);
    assert!(!whole && text.len() <= 120, "{}", text.len());
    let (kept, cut) = read_back(&text);
    assert_eq!(u64::try_from(kept).unwrap() + cut, 80 + 900 + 10 + 80 + 1200);
    assert_eq!(&text[..16], b"[942 bytes cut]\n", "the source's cut and the brief's are one line");
    assert_eq!(&text[16..54], &bytes(38, b'a')[..], "an even share each");
    assert_eq!(&text[54..64], &bytes(10, b'b')[..], "a short check whole");
    assert_eq!(&text[64..82], b"\n[1242 bytes cut]\n");
    assert_eq!(&text[82..], &bytes(38, b'c')[..]);
}

/// Parts of every shape: multibyte text, cut by their source or not, empty,
/// long and short.
fn shapes() -> [Box<[Part]>; 4] {
    let mut text = List::with_capacity(144);
    for _ in 0..12_u32 {
        for byte in "ab é→𝄞".as_bytes() {
            text.push(*byte).unwrap();
        }
    }
    let text = text.into_boxed();
    let part = |bytes: &[u8], left: u64| Part { bytes: Box::from(bytes), left };
    [
        Box::from([part(&text, 0), part(&text[..40], 0), part(b"x", 0)]),
        Box::from([part(&text[..7], 3), part(b"", 9), part(&text, 0), part(&text[..30], 0)]),
        Box::from([part(&bytes(90, b'q'), 0), part(&text[5..], 17), part(b"", 0), part(&bytes(12, b'r'), 0)]),
        Box::from([part(&text, 0)]),
    ]
}

#[test]
fn the_measure_kept_is_the_largest_that_fits() {
    for kind in KINDS {
        for parts in shapes() {
            let uncut = probe::length(kind, &parts, probe::kept(kind, &parts, usize::MAX).1);
            for budget in CUT_LINE..uncut + 2 {
                let (kept, whole) = probe::kept(kind, &parts, budget);
                assert!(probe::length(kind, &parts, kept) <= budget, "{kind:?} within {budget}");
                for more in kept + 1..=whole {
                    let longer = probe::length(kind, &parts, more);
                    assert!(longer > budget, "{kind:?} within {budget}: {more} fits too, past {kept}");
                }
            }
        }
    }
}

#[test]
fn a_brief_of_many_sections_shares_its_budget_evenly() {
    let mut content = List::with_capacity(300);
    for _ in 0..20_u32 {
        for byte in "ab é→𝄞 ".as_bytes() {
            content.push(*byte).unwrap();
        }
    }
    let content = content.into_boxed();
    let floor = u32::try_from(FLOOR).unwrap();
    let budgets = Budgets { item: 200, comments: 200, ci: 200, ..BUDGETS };
    let limits = Limits { brief_bytes: 4 * floor + 40, budgets, ..LIMITS };
    let slots = [
        slot(Kind::Item, &[&content]),
        slot(Kind::Comments, &[&content[..52], &content]),
        slot(Kind::Ci, &[&content, &content[..8]]),
        slot(Kind::Notes, &[b"- a\n", b"1 more\n"]),
    ];
    let brief = cut::brief(&slots, &limits);
    let mut total = 0;
    let mut lengths = [0; 4];
    for (index, section) in brief.sections.iter().enumerate() {
        let bytes = text(section);
        assert!(is_utf8(bytes), "{:?}", section.kind);
        lengths[index] = bytes.len();
        total += bytes.len();
    }
    assert!(total <= 4 * FLOOR + 40, "{total}");
    assert_eq!(text(&brief.sections[3]), b"- a\n1 more\n", "a short section stays whole");
    // The long ones share what is left, to within a cut line's digits and
    // a character's bytes.
    let share = (4 * FLOOR + 40 - 11) / 3;
    for length in &lengths[..3] {
        assert!(*length <= share && *length + 8 >= share, "{lengths:?} share {share}");
    }
    assert_eq!(brief.cut, 3);
}
