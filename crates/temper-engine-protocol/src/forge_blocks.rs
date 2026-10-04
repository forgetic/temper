//! Forge text markers and protected payload envelopes (forge.md, section 4).
//! This module owns the text format. The parent record's typed binary codec
//! is separate: these helpers never infer lifecycle or author policy.
use alloc::boxed::Box;
use skein_lib::{List, Reader, Writer, bytes};
use temper_channel::Sizes;
use temper_engine_domain::{
    Posted,
    forge::{Ci, Position, api::Mark},
    notes,
};
use temper_forge_forgejo::binary;

const VERSION: u8 = 1;
const KEY: &[u8] = b"<!-- temper:key ";
const KEY_V1: &[u8] = b"<!-- temper:key 1 ";
const RECORD: &[u8] = b"<!-- temper:record ";
const RECORD_V1: &[u8] = b"<!-- temper:record 1 ";
const HEAD_END: &[u8] = b" -->\n";
const BLOCK: &[u8] = b"<!-- temper:block 1\n";
const BLOCK_END: &[u8] = b"-->\n";
const OUTCOME: &[u8] = b"temper outcome\n";
const NONCE: &[u8] = b"<!-- temper:nonce ";
const NOTE: &[u8] = b"<!-- temper:note 1 ";
const NOTICE: &[u8] = b"temper keeps its record of this item here: please leave this comment as it is.\n";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub body_bytes: u32,
    pub payload_bytes: u32,
    pub key_bytes: u32,
    pub description_bytes: u32,
    pub references: u32,
}

#[derive(PartialEq, Eq, Debug)]
pub struct Note {
    pub nonce: Option<u64>,
    pub page: notes::Page,
}

/// A key marker, optionally attributing an engine-written message to a
/// person. Content bytes are appended separately by the call plan.
#[must_use]
pub fn key(key: &[u8], person: Option<u64>, limits: &Limits) -> Option<Box<[u8]>> {
    let count = bounded(key, limits.key_bytes)?;
    let mut length = size(KEY_V1)?.checked_add(count.checked_mul(2)?)?.checked_add(size(HEAD_END)?)?;
    if let Some(person) = person {
        length = length.checked_add(5)?.checked_add(decimal_len(person))?;
    }
    let mut out = writer(length, limits.body_bytes)?;
    out.put(KEY_V1).ok()?;
    let hex = binary::hex(key);
    out.put(&hex).ok()?;
    if let Some(person) = person {
        out.put(b" for ").ok()?;
        put_decimal(&mut out, person)?;
    }
    out.put(HEAD_END).ok()?;
    Some(out.finish())
}

/// The forge child's record head, including its complete opaque head id.
#[must_use]
pub fn record_head(position: Position, nonce: u64, limits: &Limits) -> Option<Box<[u8]>> {
    let length = record_head_len(position, nonce)?;
    let mut out = writer(length, limits.body_bytes)?;
    out.put(RECORD_V1).ok()?;
    for value in [position.comment, position.pull_comment, position.reviews] {
        put_decimal(&mut out, value)?;
        out.put(b" ").ok()?;
    }
    match position.head {
        Some(head) => out.put(&binary::hex(&head)).ok()?,
        None => out.put(b"-").ok()?,
    }
    out.put(b" ").ok()?;
    put_decimal(&mut out, ci_to(position.ci))?;
    out.put(b" ").ok()?;
    put_decimal(&mut out, nonce)?;
    out.put(HEAD_END).ok()?;
    Some(out.finish())
}

/// What the marker at the head says. Malformed engine markers are mangled;
/// an ordinary person's text has no marker. Author checks remain above.
#[must_use]
pub fn mark(body: &[u8], limits: &Limits) -> Mark {
    if bounded(body, limits.body_bytes).is_none() {
        return Mark::Mangled;
    }
    if body.starts_with(RECORD) {
        return match parse_record(body) {
            Some((position, nonce, _)) => Mark::Record { position, nonce },
            None => Mark::Mangled,
        };
    }
    if body.starts_with(KEY) {
        return match parse_key(body, limits) {
            Some((key, person, _)) => Mark::Key { key, person },
            None => Mark::Mangled,
        };
    }
    Mark::None
}

/// A versioned binary payload and the first sixteen SHA-256 bytes, base64
/// encoded in lines of seventy-six inside the hidden block.
#[must_use]
pub fn block(payload: &[u8], limits: &Limits) -> Option<Box<[u8]>> {
    let length = block_len(bounded(payload, limits.payload_bytes)?)?;
    let mut out = writer(length, limits.body_bytes)?;
    let protected = protect(payload, limits.payload_bytes)?;
    let encoded = binary::base64_encode(&protected, limits.body_bytes).ok()?;
    out.put(BLOCK).ok()?;
    for line in encoded.chunks(76) {
        out.put(line).ok()?;
        out.put(b"\n").ok()?;
    }
    out.put(BLOCK_END).ok()?;
    Some(out.finish())
}

/// Decodes only this block's complete versioned payload. Unknown versions,
/// edited digests, malformed base64, overlong lines, and trailing bytes fail.
#[must_use]
pub fn unblock(text: &[u8], limits: &Limits) -> Option<Box<[u8]>> {
    bounded(text, limits.body_bytes)?;
    let text = text.strip_prefix(BLOCK)?.strip_suffix(BLOCK_END)?;
    let mut encoded = List::with_capacity(bounded(text, limits.body_bytes)?);
    let mut line = 0_u32;
    for &byte in text {
        if byte == b'\n' {
            if line == 0 || line > 76 {
                return None;
            }
            line = 0;
        } else {
            line = line.checked_add(1)?;
            if line > 76 {
                return None;
            }
            encoded.push(byte).ok()?;
        }
    }
    if line != 0 {
        return None;
    }
    unprotect(encoded.as_slice(), limits.payload_bytes)
}

/// A complete record comment from its child head and typed parent payload.
#[must_use]
pub fn record(position: Position, nonce: u64, payload: &[u8], limits: &Limits) -> Option<Box<[u8]>> {
    let payload_len = bounded(payload, limits.payload_bytes)?;
    let length = record_head_len(position, nonce)?.checked_add(size(NOTICE)?)?.checked_add(block_len(payload_len)?)?;
    let mut out = writer(length, limits.body_bytes)?;
    out.put(&record_head(position, nonce, limits)?).ok()?;
    out.put(NOTICE).ok()?;
    out.put(&block(payload, limits)?).ok()?;
    Some(out.finish())
}

/// The parent payload of a record, with the child position and nonce.
#[must_use]
pub fn record_payload(body: &[u8], limits: &Limits) -> Option<(Position, u64, Box<[u8]>)> {
    bounded(body, limits.body_bytes)?;
    let (position, nonce, rest) = parse_record(body)?;
    Some((position, nonce, unblock(rest.strip_prefix(NOTICE)?, limits)?))
}

/// An outcome's protected block after the creation key's marker.
#[must_use]
pub fn posted(posted: &Posted, sizes: &Sizes, limits: &Limits) -> Option<Box<[u8]>> {
    let outcome = crate::payload::encode_outcome(&posted.outcome, sizes)?;
    let length = size(&outcome)?.checked_add(13)?.checked_add(if posted.head.is_some() { 32 } else { 0 })?;
    let mut payload = writer(length, limits.payload_bytes)?;
    payload.put(&posted.attempt.to_be_bytes()).ok()?;
    payload.put(&size(&outcome)?.to_be_bytes()).ok()?;
    payload.put(&outcome).ok()?;
    match posted.head {
        Some(head) => {
            payload.put(&[1]).ok()?;
            payload.put(&head).ok()?;
        }
        None => payload.put(&[0]).ok()?,
    }
    let length = size(OUTCOME)?.checked_add(block_len(length)?)?;
    let mut out = writer(length, limits.body_bytes)?;
    out.put(OUTCOME).ok()?;
    out.put(&block(&payload.finish(), limits)?).ok()?;
    Some(out.finish())
}

/// Reads an outcome only after a valid creation key marker and outcome line.
#[must_use]
pub fn posted_of(body: &[u8], sizes: &Sizes, limits: &Limits) -> Option<Posted> {
    bounded(body, limits.body_bytes)?;
    let (_, _, rest) = parse_key(body, limits)?;
    let payload = unblock(rest.strip_prefix(OUTCOME)?, limits)?;
    let mut input = Reader::new(&payload);
    let attempt = input.u64()?;
    let length = input.u32()?;
    let outcome = crate::payload::decode_outcome(input.bytes(length)?, sizes)?;
    let head = match input.u8()? {
        0 => None,
        1 => {
            let mut head = [0; 32];
            head.copy_from_slice(input.bytes(32)?);
            Some(head)
        }
        _ => return None,
    };
    if !input.is_empty() {
        return None;
    }
    Some(Posted { attempt, outcome, head })
}

/// A readable note. Only author/reference metadata is digested: a person's
/// correction of its description or body remains readable as the same note.
#[must_use]
pub fn note(page: &notes::Page, nonce: Option<u64>, limits: &Limits) -> Option<Box<[u8]>> {
    bounded(&page.description, limits.description_bytes)?;
    bounded(&page.body, limits.body_bytes)?;
    for &byte in &page.description {
        if byte == b'\n' || byte == b'\r' {
            return None;
        }
    }
    let count = u32::try_from(page.references.len()).ok()?;
    if count > limits.references {
        return None;
    }
    let author = match page.author {
        notes::Author::Person(_) => 9_u32,
        notes::Author::Run { .. } => 13,
    };
    let meta_len = author.checked_add(4)?.checked_add(count.checked_mul(12)?)?;
    let encoded_len = encoded_len(meta_len)?;
    let mut length = size(&page.description)?
        .checked_add(1)?
        .checked_add(size(NOTE)?)?
        .checked_add(encoded_len)?
        .checked_add(size(HEAD_END)?)?
        .checked_add(size(&page.body)?)?;
    if let Some(nonce) = nonce {
        length = length.checked_add(nonce_len(nonce)?)?;
    }
    if meta_len > limits.payload_bytes || length > limits.body_bytes {
        return None;
    }
    let mut out = writer(length, limits.body_bytes)?;
    let mut meta = writer(meta_len, limits.payload_bytes)?;
    match page.author {
        notes::Author::Person(person) => {
            meta.put(&[0]).ok()?;
            meta.put(&person.to_be_bytes()).ok()?;
        }
        notes::Author::Run { repository, number } => {
            meta.put(&[1]).ok()?;
            meta.put(&repository.to_be_bytes()).ok()?;
            meta.put(&number.to_be_bytes()).ok()?;
        }
    }
    meta.put(&count.to_be_bytes()).ok()?;
    for reference in &page.references {
        meta.put(&reference.repository.to_be_bytes()).ok()?;
        meta.put(&reference.number.to_be_bytes()).ok()?;
    }
    let protected = protect(&meta.finish(), limits.payload_bytes)?;
    let encoded = binary::base64_encode(&protected, limits.body_bytes).ok()?;
    if let Some(nonce) = nonce {
        out.put(NONCE).ok()?;
        put_decimal(&mut out, nonce)?;
        out.put(HEAD_END).ok()?;
    }
    out.put(&page.description).ok()?;
    out.put(b"\n").ok()?;
    out.put(NOTE).ok()?;
    out.put(&encoded).ok()?;
    out.put(HEAD_END).ok()?;
    out.put(&page.body).ok()?;
    Some(out.finish())
}

#[must_use]
pub fn note_of(content: &[u8], limits: &Limits) -> Option<Note> {
    bounded(content, limits.body_bytes)?;
    let (nonce, content) = match content.strip_prefix(NONCE) {
        Some(rest) => {
            let (line, content) = head(rest)?;
            (Some(number(line)?), content)
        }
        None => (None, content),
    };
    let end = bytes::find(content, b"\n")?;
    let description = content.get(..end)?;
    bounded(description, limits.description_bytes)?;
    let (encoded, body) = head(content.get(end.checked_add(1)?..)?.strip_prefix(NOTE)?)?;
    let metadata = unprotect(encoded, limits.payload_bytes)?;
    let mut input = Reader::new(&metadata);
    let author = match input.u8()? {
        0 => notes::Author::Person(input.u64()?),
        1 => notes::Author::Run { repository: input.u32()?, number: input.u64()? },
        _ => return None,
    };
    let count = input.u32()?;
    if count > limits.references || count.checked_mul(12)? > input.remaining() {
        return None;
    }
    let mut references = List::with_capacity(count);
    for _ in 0..count {
        references.push(notes::Reference { repository: input.u32()?, number: input.u64()? }).ok()?;
    }
    if !input.is_empty() {
        return None;
    }
    Some(Note {
        nonce,
        page: notes::Page {
            description: bytes::copy_of(description),
            author,
            references: references.into_boxed(),
            body: bytes::copy_of(body),
        },
    })
}

fn protect(payload: &[u8], cap: u32) -> Option<Box<[u8]>> {
    let length = bounded(payload, cap)?.checked_add(1)?;
    let mut versioned = writer(length, length)?;
    versioned.put(&[VERSION]).ok()?;
    versioned.put(payload).ok()?;
    let versioned = versioned.finish();
    let digest = binary::digest(&versioned);
    let total = length.checked_add(16)?;
    let mut out = writer(total, total)?;
    out.put(&versioned).ok()?;
    out.put(digest.get(..16)?).ok()?;
    Some(out.finish())
}
fn unprotect(encoded: &[u8], cap: u32) -> Option<Box<[u8]>> {
    let protected = binary::base64_decode(encoded, cap.checked_add(17)?).ok()?;
    let split = protected.len().checked_sub(16)?;
    let versioned = protected.get(..split)?;
    let digest = binary::digest(versioned);
    if protected.get(split..)? != digest.get(..16)? {
        return None;
    }
    let (&version, payload) = versioned.split_first()?;
    if version != VERSION {
        return None;
    }
    bounded(payload, cap)?;
    Some(bytes::copy_of(payload))
}
fn parse_record(body: &[u8]) -> Option<(Position, u64, &[u8])> {
    let (line, rest) = head(body.strip_prefix(RECORD_V1)?)?;
    let (comment, line) = field(line)?;
    let (pull_comment, line) = field(line)?;
    let (reviews, line) = field(line)?;
    let (head, line) = field(line)?;
    let (ci, nonce) = field(line)?;
    let commit = if head == b"-" {
        None
    } else {
        let mut commit = [0; 32];
        binary::unhex(head, &mut commit).ok()?;
        Some(commit)
    };
    let position = Position {
        comment: number(comment)?,
        pull_comment: number(pull_comment)?,
        reviews: number(reviews)?,
        head: commit,
        ci: ci_from(number(ci)?)?,
    };
    Some((position, number(nonce)?, rest))
}
fn parse_key(body: &[u8], limits: &Limits) -> Option<(Box<[u8]>, Option<u64>, &[u8])> {
    let (line, rest) = head(body.strip_prefix(KEY_V1)?)?;
    let (key, person) = match bytes::find(line, b" for ") {
        Some(at) => (line.get(..at)?, Some(number(line.get(at.checked_add(5)?..)?)?)),
        None => (line, None),
    };
    if !key.len().is_multiple_of(2) {
        return None;
    }
    let count = u32::try_from(key.len().checked_div(2)?).ok()?;
    if count > limits.key_bytes {
        return None;
    }
    let mut decoded = bytes::zeroed(usize::try_from(count).ok()?);
    binary::unhex(key, &mut decoded).ok()?;
    Some((decoded, person, rest))
}
fn head(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = bytes::find(input, HEAD_END)?;
    Some((input.get(..end)?, input.get(end.checked_add(HEAD_END.len())?..)?))
}
fn field(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = bytes::find(input, b" ")?;
    Some((input.get(..end)?, input.get(end.checked_add(1)?..)?))
}
fn number(input: &[u8]) -> Option<u64> {
    if input.is_empty() || input.len() > 20 {
        return None;
    }
    let mut number = 0_u64;
    for &byte in input {
        let digit = match byte {
            b'0'..=b'9' => byte.wrapping_sub(b'0'),
            _ => return None,
        };
        number = number.checked_mul(10)?.checked_add(u64::from(digit))?;
    }
    Some(number)
}
fn put_decimal(out: &mut Writer, mut number: u64) -> Option<()> {
    let mut digits = [0; 20];
    let mut start = digits.len();
    for _ in 0..20 {
        start = start.checked_sub(1)?;
        let digit = u8::try_from(number.checked_rem(10)?).ok()?;
        *digits.get_mut(start)? = b'0'.wrapping_add(digit);
        number = number.checked_div(10)?;
        if number == 0 {
            break;
        }
    }
    out.put(digits.get(start..)?).ok()
}
fn decimal_len(mut number: u64) -> u32 {
    let mut length = 1_u32;
    for _ in 0..19 {
        if number < 10 {
            break;
        }
        number = number.checked_div(10).expect("positive divisor");
        length = length.saturating_add(1);
    }
    length
}
fn record_head_len(position: Position, nonce: u64) -> Option<u32> {
    let mut length = size(RECORD_V1)?.checked_add(size(HEAD_END)?)?;
    for number in [position.comment, position.pull_comment, position.reviews, ci_to(position.ci)] {
        length = length.checked_add(decimal_len(number))?.checked_add(1)?;
    }
    length.checked_add(if position.head.is_some() { 65 } else { 2 })?.checked_add(decimal_len(nonce))
}
fn nonce_len(nonce: u64) -> Option<u32> {
    size(NONCE)?.checked_add(decimal_len(nonce))?.checked_add(size(HEAD_END)?)
}
fn block_len(payload: u32) -> Option<u32> {
    let encoded = encoded_len(payload)?;
    let lines = encoded.checked_add(75)?.checked_div(76)?;
    size(BLOCK)?.checked_add(encoded)?.checked_add(lines)?.checked_add(size(BLOCK_END)?)
}
fn encoded_len(payload: u32) -> Option<u32> {
    payload.checked_add(17)?.checked_add(2)?.checked_div(3)?.checked_mul(4)
}
fn size(bytes: &[u8]) -> Option<u32> {
    u32::try_from(bytes.len()).ok()
}
fn bounded(bytes: &[u8], cap: u32) -> Option<u32> {
    let length = size(bytes)?;
    if length <= cap { Some(length) } else { None }
}
fn writer(length: u32, cap: u32) -> Option<Writer> {
    if length > cap {
        return None;
    }
    Some(Writer::new(usize::try_from(length).ok()?))
}
const fn ci_to(ci: Ci) -> u64 {
    match ci {
        Ci::None => 0,
        Ci::Pending => 1,
        Ci::Passed => 2,
        Ci::Failed => 3,
    }
}
const fn ci_from(ci: u64) -> Option<Ci> {
    match ci {
        0 => Some(Ci::None),
        1 => Some(Ci::Pending),
        2 => Some(Ci::Passed),
        3 => Some(Ci::Failed),
        _ => None,
    }
}

/// Includes text, versioned/digested and base64 temporaries, decoded owned
/// metadata, and a copied parent payload. It excludes the caller's input.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let body = u64::from(limits.body_bytes).checked_mul(5)?;
    let payload = u64::from(limits.payload_bytes).checked_add(17)?.checked_mul(6)?;
    let references =
        u64::from(limits.references).checked_mul(u64::try_from(core::mem::size_of::<notes::Reference>()).ok()?)?;
    body.checked_add(payload)?.checked_add(references)?.checked_add(512)
}
