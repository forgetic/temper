//! Brief sections retained by the forge connector (jig's domain/connectors.md,
//! section 9). The parent sees a token and rendered size while the connector
//! owns the words. Taking or dropping a token ends that ownership.

use alloc::boxed::Box;

use skein_lib::{Decimal, Queue, Token, Writer};

use crate::{BriefRead, BriefSource, Domain, Request, brief};

#[derive(Debug)]
pub(crate) struct Pending {
    source: BriefSource,
    budget: u32,
    dropped: bool,
}

#[derive(Debug)]
pub(crate) struct Held {
    source: BriefSource,
    words: Box<[u8]>,
}

#[expect(clippy::too_many_arguments, reason = "one bounded section handoff names its source and read limits")]
pub(crate) fn gather(
    domain: &mut Domain,
    section: Token,
    source: BriefSource,
    parts: u32,
    bytes: u32,
    max_job_bytes: u32,
    limit: u32,
    out: &mut Queue<Request>,
) {
    if domain.brief_pending.contains_key(&section)
        || domain.brief_held.contains_key(&section)
        || domain.brief_pending.len().saturating_add(domain.brief_held.len()) >= domain.brief_held.capacity()
    {
        out.push(Request::BriefSized { section, size: None });
        return;
    }
    domain
        .brief_pending
        .insert(section, Pending { source, budget: bytes, dropped: false })
        .expect("section room checked");
    brief::gather(domain, section, source, parts, bytes, max_job_bytes, limit, out);
}

/// Route a finished forge read to either the token owner or the old caller.
pub(crate) fn ready(domain: &mut Domain, section: Token, read: BriefRead, out: &mut Queue<Request>) {
    let Some(pending) = domain.brief_pending.remove(&section) else {
        out.push(Request::BriefReady { owner: section, read });
        return;
    };
    if pending.dropped {
        return;
    }
    let words = match read {
        BriefRead::Got { bytes, left } => render(pending.source, bytes, left, pending.budget),
        BriefRead::Failed => None,
    };
    match words {
        Some(words) => {
            let size = u32::try_from(words.len()).expect("bounded rendered section");
            domain
                .brief_held
                .insert(section, Held { source: pending.source, words })
                .expect("pending reserved held room");
            out.push(Request::BriefSized { section, size: Some(size) });
        }
        None => out.push(Request::BriefSized { section, size: None }),
    }
}

pub(crate) fn cut(domain: &mut Domain, section: Token, bytes: u32, out: &mut Queue<Request>) {
    let Some(held) = domain.brief_held.remove(&section) else {
        out.push(Request::BriefSized { section, size: None });
        return;
    };
    if bytes >= u32::try_from(held.words.len()).expect("bounded section") {
        let size = u32::try_from(held.words.len()).expect("bounded section");
        domain.brief_held.insert(section, held).expect("same held room");
        out.push(Request::BriefSized { section, size: Some(size) });
        return;
    }
    match render(held.source, held.words.clone(), 0, bytes) {
        Some(words) => {
            let size = u32::try_from(words.len()).expect("bounded cut");
            domain.brief_held.insert(section, Held { source: held.source, words }).expect("same held room");
            out.push(Request::BriefSized { section, size: Some(size) });
        }
        None => {
            domain.brief_held.insert(section, held).expect("same held room");
            out.push(Request::BriefSized { section, size: None });
        }
    }
}

pub(crate) fn take(domain: &mut Domain, section: Token, out: &mut Queue<Request>) {
    let bytes = match domain.brief_held.remove(&section) {
        Some(held) => Some(held.words),
        None => None,
    };
    out.push(Request::BriefTaken { section, bytes });
}

pub(crate) fn drop_section(domain: &mut Domain, section: Token) {
    domain.brief_held.remove(&section);
    if let Some(pending) = domain.brief_pending.get_mut(&section) {
        pending.dropped = true;
    }
}

fn render(source: BriefSource, text: Box<[u8]>, left: u64, budget: u32) -> Option<Box<[u8]>> {
    let room = usize::try_from(budget).ok()?;
    if left == 0 && text.len() <= room {
        return Some(text);
    }
    let tail = match source {
        BriefSource::Ci { .. } => true,
        BriefSource::Reviews { .. } | BriefSource::Pull { .. } => false,
    };
    let mut low = 0_usize;
    let mut high = text.len().min(room);
    let minimum = marker(left.checked_add(u64::try_from(text.len()).ok()?)?, false);
    if minimum > room {
        return None;
    }
    for _ in 0..usize::BITS {
        if low >= high {
            break;
        }
        let mid = low.saturating_add(high.saturating_sub(low).div_ceil(2));
        let kept = edge(&text, mid, tail);
        let lost = left.checked_add(u64::try_from(text.len().saturating_sub(kept.len())).ok()?)?;
        if kept.len().saturating_add(marker(lost, !kept.is_empty())) <= room {
            low = mid;
        } else {
            high = mid.saturating_sub(1);
        }
    }
    let kept = edge(&text, low, tail);
    let lost = left.checked_add(u64::try_from(text.len().saturating_sub(kept.len())).ok()?)?;
    if lost == 0 {
        return Some(Box::from(kept));
    }
    let digits = Decimal::of(lost);
    let size = kept.len().saturating_add(marker(lost, !kept.is_empty()));
    let mut writer = Writer::new(size);
    writer.put(kept).expect("measured section");
    if !kept.is_empty() {
        writer.put(b"\n").expect("measured break");
    }
    writer.put(b"[").expect("measured marker");
    writer.put(digits.as_bytes()).expect("measured count");
    writer.put(b" bytes cut]\n").expect("measured marker");
    Some(writer.finish())
}

fn marker(lost: u64, leading: bool) -> usize {
    usize::from(leading)
        .saturating_add(b"[".len())
        .saturating_add(Decimal::of(lost).as_bytes().len())
        .saturating_add(b" bytes cut]\n".len())
}

fn edge(text: &[u8], wanted: usize, tail: bool) -> &[u8] {
    if tail {
        let mut start = text.len().saturating_sub(wanted);
        for _ in 0..3 {
            if continues(text.get(start)) {
                start = start.saturating_add(1);
            }
        }
        text.get(start..).expect("within section")
    } else {
        let mut end = wanted;
        for _ in 0..3 {
            if continues(text.get(end)) {
                end = end.saturating_sub(1);
            }
        }
        text.get(..end).expect("within section")
    }
}

fn continues(byte: Option<&u8>) -> bool {
    match byte {
        Some(byte) => byte & 0b1100_0000 == 0b1000_0000,
        None => false,
    }
}
