//! The protocol layer, as the world plays it: the forge child domain's
//! operations as calls to the fake forge, and the fake's answers as the
//! child domain's, with what a Forgejo protocol layer would put inside what the
//! engine creates and find again when it reads.
//!
//! - **Markers.** A key goes at the head of what it keys (an issue's body, a
//!   comment, a review), with the person a comment is written for if it is
//!   a person's, and a record at the head of its comment, its inbox position
//!   and its write's nonce written out; a body that starts as a record and
//!   does not decode is a record mangled. A wiki page the engine writes
//!   starts with its write's nonce. A person's marker is read like any
//!   other: the child domain decides whose it is.
//! - **What the fake does not keep** is answered as Forgejo would answer
//!   what has none: its statuses describe themselves by their state and
//!   point nowhere, and its reviews have no inline comments.
//! - **Payloads** the child domain names by tokens are filled in by the parent
//!   as the call goes out ([`Fill`]).
//! - **Commits** are the fake's counts, in the first 8 bytes, big-endian.
//! - **A comment's revision** is a digest of its body, so it changes whenever
//!   the body does, at any resolution of the forge's clock.
//! - **Pages** are the child domain's: a listing asks the fake for a page of
//!   the child domain's size, and a page of comments is cut to it, and reviews
//!   and statuses are paged by number out of all the fake shows; and so are
//!   texts, to their limits, once what is marked at their heads is read.
//! - **CI** on a commit is combined over its contexts as Forgejo combines it:
//!   failed if any failed, pending if any is pending, passed if all passed.
//! - **Times** the fake shows are its own clock's; a rate limit's reset is
//!   told as the wait from the fake's time as it answers.

use std::collections::BTreeMap;

use temper_engine_domain_forge::api as engine;
use temper_engine_domain_forge::{Ci, Limits, Position};
use temper_forge_domain::api as forge;
use temper_lib::{Duration, Time, Token};

const KEY: &[u8] = b"<!-- temper:key ";
const RECORD: &[u8] = b"<!-- temper:record ";
const NONCE: &[u8] = b"<!-- temper:nonce ";
const END: &[u8] = b" -->\n";

/// The payloads the parent names, by token: what it fills in as a call
/// carrying one goes out.
pub type Fill = BTreeMap<u64, Vec<u8>>;

/// What an engine call asked, which says how its answer reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Asked {
    Items,
    Item,
    Comment {
        number: u64,
    },
    /// A pull request; the `page`th page of its reviews; of the statuses on
    /// a commit; a review's inline comments.
    Pull,
    Remarks,
    Reviews {
        page: u32,
    },
    Statuses {
        page: u32,
    },
    Permission,
    Branch,
    Pages,
    Page,
    Create,
    /// A comment posted, with the digest of its body.
    Post {
        revision: u64,
    },
    /// A comment edited, likewise.
    Edit {
        revision: u64,
    },
    Merge,
    Review,
    Revision,
    Done,
}

/// A commit's name in the engine.
#[must_use]
pub fn commit(count: u64) -> [u8; 32] {
    let mut name = [0; 32];
    name[..8].copy_from_slice(&count.to_be_bytes());
    name
}

/// The fake's count of a commit the engine names.
#[must_use]
pub fn count(commit: [u8; 32]) -> u64 {
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&commit[..8]);
    u64::from_be_bytes(bytes)
}

/// A digest of `body`: a comment's revision (FNV-1a).
#[must_use]
pub fn digest(body: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in body {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// `content`, keyed by `key`.
#[must_use]
pub fn keyed(key: &[u8], content: &[u8]) -> Vec<u8> {
    written_for(key, None, content)
}

/// `content`, keyed by `key`, written for `person` if it is a person's.
#[must_use]
pub fn written_for(key: &[u8], person: Option<u64>, content: &[u8]) -> Vec<u8> {
    let mut body = KEY.to_vec();
    body.extend_from_slice(&hex(key));
    if let Some(person) = person {
        body.extend_from_slice(format!(" for {person}").as_bytes());
    }
    body.extend_from_slice(END);
    body.extend_from_slice(content);
    body
}

/// A record saying `position`, written by the write of `nonce`, and the
/// parent's `payload`.
#[must_use]
pub fn recorded(position: Position, nonce: u64, payload: &[u8]) -> Vec<u8> {
    let head = match position.head {
        Some(head) => count(head).to_string(),
        None => "-".to_owned(),
    };
    let ci = match position.ci {
        Ci::None => 0,
        Ci::Pending => 1,
        Ci::Passed => 2,
        Ci::Failed => 3,
    };
    let mut body = RECORD.to_vec();
    let fields = format!("{} {} {} {head} {ci} {nonce}", position.comment, position.pull_comment, position.reviews);
    body.extend_from_slice(fields.as_bytes());
    body.extend_from_slice(END);
    body.extend_from_slice(payload);
    body
}

/// The key a body starts with, if it starts with one.
#[must_use]
pub fn key_of(body: &[u8]) -> Option<Vec<u8>> {
    Some(marker(body)?.0)
}

/// The key a body starts with, and the person it was written for, if it is
/// a person's.
fn marker(body: &[u8]) -> Option<(Vec<u8>, Option<u64>)> {
    let rest = body.strip_prefix(KEY)?;
    let end = find(rest, END)?;
    let text = std::str::from_utf8(&rest[..end]).ok()?;
    let mut fields = text.split(" for ");
    let key = unhex(fields.next()?.as_bytes())?;
    let person = match fields.next() {
        Some(person) => Some(person.parse().ok()?),
        None => None,
    };
    Some((key, person))
}

/// Whether a body starts as a record, whether or not it decodes.
#[must_use]
pub fn is_record(body: &[u8]) -> bool {
    body.starts_with(RECORD)
}

/// What a comment's body has at its head, for the child domain.
#[must_use]
pub fn mark(body: &[u8]) -> engine::Mark {
    if let Some(rest) = body.strip_prefix(RECORD) {
        return match position(rest) {
            Some((position, nonce)) => engine::Mark::Record { position, nonce },
            None => engine::Mark::Mangled,
        };
    }
    match marker(body) {
        Some((key, person)) => engine::Mark::Key { key: key.into(), person },
        None => engine::Mark::None,
    }
}

/// A wiki page's `content`, written by the write of `nonce`.
#[must_use]
pub fn paged(nonce: u64, content: &[u8]) -> Vec<u8> {
    let mut page = NONCE.to_vec();
    page.extend_from_slice(nonce.to_string().as_bytes());
    page.extend_from_slice(END);
    page.extend_from_slice(content);
    page
}

/// The nonce a wiki page starts with, if it starts with one.
#[must_use]
pub fn nonce_of(content: &[u8]) -> Option<u64> {
    let rest = content.strip_prefix(NONCE)?;
    let end = find(rest, END)?;
    std::str::from_utf8(&rest[..end]).ok()?.parse().ok()
}

/// The position and the nonce a record's head says.
fn position(rest: &[u8]) -> Option<(Position, u64)> {
    let end = find(rest, END)?;
    let text = std::str::from_utf8(&rest[..end]).ok()?;
    let mut fields = text.split(' ');
    let comment = fields.next()?.parse().ok()?;
    let pull_comment = fields.next()?.parse().ok()?;
    let reviews = fields.next()?.parse().ok()?;
    let head = match fields.next()? {
        "-" => None,
        count => Some(commit(count.parse().ok()?)),
    };
    let ci = match fields.next()? {
        "0" => Ci::None,
        "1" => Ci::Pending,
        "2" => Ci::Passed,
        "3" => Ci::Failed,
        _ => return None,
    };
    let nonce = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some((Position { comment, pull_comment, reviews, head, ci }, nonce))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn hex(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().flat_map(|byte| format!("{byte:02x}").into_bytes()).collect()
}

fn unhex(text: &[u8]) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()).collect()
}

/// The body a write carries, payloads filled in.
fn body(body: engine::Body, fill: &Fill) -> Vec<u8> {
    match body {
        engine::Body::Text(text) => text.into_vec(),
        engine::Body::Payload(token) => payload(token, fill),
        engine::Body::Record { payload: token, position, nonce } => recorded(position, nonce, &payload(token, fill)),
    }
}

fn payload(token: Token, fill: &Fill) -> Vec<u8> {
    fill.get(&token.raw()).expect("the parent names payloads it has").clone()
}

/// The fake's call for an engine operation, asking for pages of `page`
/// entries, and what it asked.
#[must_use]
pub fn op(op: engine::Op, page: u32, fill: &Fill) -> (Asked, forge::Op) {
    use forge::{Op, Read, Write};
    match op {
        engine::Op::Items { state, kind, label, author, since, page: number } => {
            let labels = match label {
                Some(label) => vec![label].into_boxed_slice(),
                None => Box::new([]),
            };
            let read = Read::Items {
                state: state.map(state_of),
                kind: kind.map(kind_of),
                labels,
                author,
                since,
                page: number,
                limit: page,
            };
            (Asked::Items, Op::Read(read))
        }
        engine::Op::Item { number, after } => (Asked::Item, Op::Read(Read::Item { number, after })),
        engine::Op::Comment { number, id } => (Asked::Comment { number }, Op::Read(Read::Comment { id })),
        engine::Op::Pull { number } => (Asked::Pull, Op::Read(Read::Pull { number })),
        engine::Op::PullFor { head, base } => (Asked::Pull, Op::Read(Read::PullFor { head, base })),
        engine::Op::Reviews { number, page } => (Asked::Reviews { page }, Op::Read(Read::Pull { number })),
        engine::Op::Statuses { commit, page } => {
            (Asked::Statuses { page }, Op::Read(Read::Statuses { commit: count(commit) }))
        }
        engine::Op::Remarks { number, review: _, page: _ } => (Asked::Remarks, Op::Read(Read::Pull { number })),
        engine::Op::Permission { user } => (Asked::Permission, Op::Read(Read::Permission { user })),
        engine::Op::Branch { branch } => (Asked::Branch, Op::Read(Read::Branch { branch })),
        engine::Op::Pages { after } => (Asked::Pages, Op::Read(Read::Pages { after })),
        engine::Op::Page { name } => (Asked::Page, Op::Read(Read::Page { name })),
        engine::Op::CreateIssue { key, title, body: content, labels } => {
            let body = keyed(&key, &body(content, fill)).into_boxed_slice();
            (Asked::Create, Op::Write(Write::CreateIssue { title, body, labels }))
        }
        engine::Op::Post { number, key, person, body: content } => {
            let content = body(content, fill);
            let body = match key {
                Some(key) => written_for(&key, person, &content),
                None => content,
            };
            let revision = digest(&body);
            (Asked::Post { revision }, Op::Write(Write::Comment { number, body: body.into_boxed_slice() }))
        }
        engine::Op::EditComment { number: _, id, body: content } => {
            let body = body(content, fill);
            let revision = digest(&body);
            (Asked::Edit { revision }, Op::Write(Write::EditComment { id, body: body.into_boxed_slice() }))
        }
        engine::Op::AddLabels { number, labels } => (Asked::Done, Op::Write(Write::AddLabels { number, labels })),
        engine::Op::Review { number, key, verdict, body: content } => {
            let body = keyed(&key, &body(content, fill)).into_boxed_slice();
            (Asked::Review, Op::Write(Write::Review { number, verdict: Some(verdict_of(verdict)), body }))
        }
        engine::Op::SetReviewers { number, reviewers } => {
            (Asked::Done, Op::Write(Write::SetReviewers { number, reviewers }))
        }
        engine::Op::SetDependencies { number, dependencies } => {
            (Asked::Done, Op::Write(Write::SetDependencies { number, dependencies }))
        }
        engine::Op::Reopen { number } => (Asked::Done, Op::Write(Write::Reopen { number })),
        engine::Op::RemoveLabels { number, labels } => (Asked::Done, Op::Write(Write::RemoveLabels { number, labels })),
        engine::Op::OpenPull { title, body: content, head, base } => {
            let body = body(content, fill).into_boxed_slice();
            (Asked::Create, Op::Write(Write::OpenPull { title, body, head, base }))
        }
        engine::Op::Merge { number, head } => (Asked::Merge, Op::Write(Write::Merge { number, head: count(head) })),
        engine::Op::Close { number } => (Asked::Done, Op::Write(Write::Close { number })),
        engine::Op::DeleteBranch { branch } => (Asked::Done, Op::Write(Write::DeleteBranch { branch })),
        engine::Op::PutPage { name, content, nonce } => {
            let content = paged(nonce, &body(content, fill)).into_boxed_slice();
            (Asked::Revision, Op::Write(Write::PutPage { name, content }))
        }
        engine::Op::DeletePage { name } => (Asked::Done, Op::Write(Write::DeletePage { name })),
    }
}

/// The child domain's answer for what the fake answered, at its time `now`, a
/// call that asked `asked`, cutting a page of comments, reviews or statuses,
/// and texts, to `limits`.
///
/// # Errors
///
/// What the call failed with, as the child domain names it.
pub fn answer(
    asked: Asked,
    result: Result<forge::Answer, forge::Error>,
    limits: &Limits,
    now: Time,
) -> Result<engine::Answer, engine::Error> {
    let answer = result.map_err(|failed| error(failed, now))?;
    let page = usize::try_from(limits.page).expect("a page fits a usize");
    let answer = match asked {
        Asked::Items => {
            let forge::Answer::Items { items, more, now } = answer else { unasked(asked, &answer) };
            engine::Answer::Items { items: items.iter().map(|item| summary(item, limits)).collect(), more, now }
        }
        Asked::Item => {
            let forge::Answer::Item { item, comments, more } = answer else { unasked(asked, &answer) };
            let cut = comments.len() > page;
            let comments = comments.iter().take(page).map(|found| comment(found, limits)).collect();
            engine::Answer::Item { item: summary(&item, limits), comments, more: more || cut }
        }
        Asked::Comment { number } => {
            let forge::Answer::Comment { number: on, comment: found } = answer else { unasked(asked, &answer) };
            if on != number {
                return Err(engine::Error::Missing);
            }
            engine::Answer::Comment(comment(&found, limits))
        }
        Asked::Pull => {
            let forge::Answer::Pull(found) = answer else { unasked(asked, &answer) };
            engine::Answer::Pull(pull(&found))
        }
        Asked::Reviews { page: number } => {
            let forge::Answer::Pull(found) = answer else { unasked(asked, &answer) };
            let (reviews, more) = page_of(&found.reviews, number, page);
            engine::Answer::Reviews { reviews: reviews.iter().map(|found| review(found, limits)).collect(), more }
        }
        Asked::Statuses { page: number } => {
            let forge::Answer::Statuses(statuses) = answer else { unasked(asked, &answer) };
            let (shown, more) = page_of(&statuses, number, page);
            engine::Answer::Statuses { ci: combined(&statuses), statuses: shown.iter().map(status).collect(), more }
        }
        Asked::Remarks => {
            let forge::Answer::Pull(_) = answer else { unasked(asked, &answer) };
            engine::Answer::Remarks { remarks: Box::new([]), more: false }
        }
        Asked::Permission => {
            let forge::Answer::Permission(permission) = answer else { unasked(asked, &answer) };
            engine::Answer::Permission(match permission {
                forge::Permission::None => engine::Permission::None,
                forge::Permission::Read => engine::Permission::Read,
                forge::Permission::Write => engine::Permission::Write,
                forge::Permission::Admin => engine::Permission::Admin,
            })
        }
        Asked::Branch => {
            let forge::Answer::Commit(found) = answer else { unasked(asked, &answer) };
            engine::Answer::Commit(commit(found))
        }
        Asked::Pages => {
            let forge::Answer::Pages { pages, next } = answer else { unasked(asked, &answer) };
            let pages = pages
                .iter()
                .map(|name| engine::PageName { name: name.name.clone(), revision: name.revision })
                .collect();
            engine::Answer::Pages { pages, next }
        }
        Asked::Page => {
            let forge::Answer::Page(found) = answer else { unasked(asked, &answer) };
            let nonce = nonce_of(&found.content);
            let content = cut(&found.content, limits.body_bytes);
            engine::Answer::Page(engine::Page { name: found.name, content, revision: found.revision, nonce })
        }
        Asked::Create => {
            let forge::Answer::Created(number) = answer else { unasked(asked, &answer) };
            engine::Answer::Created(number)
        }
        Asked::Post { revision } => {
            let forge::Answer::Commented(id) = answer else { unasked(asked, &answer) };
            engine::Answer::Commented { id, revision }
        }
        Asked::Edit { revision } => {
            let forge::Answer::Done = answer else { unasked(asked, &answer) };
            engine::Answer::Edited { revision }
        }
        Asked::Merge => {
            let forge::Answer::Merged(made) = answer else { unasked(asked, &answer) };
            engine::Answer::Merged(commit(made))
        }
        Asked::Review => {
            let forge::Answer::Reviewed(id) = answer else { unasked(asked, &answer) };
            engine::Answer::Reviewed(id)
        }
        Asked::Revision => {
            let forge::Answer::Revision(revision) = answer else { unasked(asked, &answer) };
            engine::Answer::Revision(revision)
        }
        Asked::Done => {
            let forge::Answer::Done = answer else { unasked(asked, &answer) };
            engine::Answer::Done
        }
    };
    Ok(answer)
}

/// The fake answered otherwise than asked: the world's bug.
fn unasked(asked: Asked, answer: &forge::Answer) -> ! {
    panic!("the fake answers {asked:?} as asked: {answer:?}")
}

/// The child domain's error for the fake's, answered at its time `now`.
#[must_use]
pub fn error(error: forge::Error, now: Time) -> engine::Error {
    match error {
        forge::Error::Unavailable => engine::Error::Unavailable,
        forge::Error::Timeout => engine::Error::Timeout,
        forge::Error::RateLimited { reset } => {
            engine::Error::RateLimited { after: Duration::from_nanos(reset.as_nanos().saturating_sub(now.as_nanos())) }
        }
        forge::Error::Forbidden => engine::Error::Forbidden,
        forge::Error::Missing(_) => engine::Error::Missing,
        forge::Error::TooLarge => engine::Error::TooLarge,
        forge::Error::Full => engine::Error::Full,
        forge::Error::Exists => engine::Error::Exists,
        forge::Error::NothingToMerge => engine::Error::NothingToMerge,
        forge::Error::Empty => engine::Error::Empty,
        forge::Error::Closed => engine::Error::Closed,
        forge::Error::Stale => engine::Error::Stale,
        forge::Error::Conflict => engine::Error::Conflict,
        forge::Error::Protected => engine::Error::Protected,
        forge::Error::Circular => engine::Error::Circular,
        forge::Error::Unreachable | forge::Error::Refused => {
            panic!("the engine's calls are refused so only for what it does not ask: {error:?}")
        }
    }
}

fn state_of(state: engine::State) -> forge::State {
    match state {
        engine::State::Open => forge::State::Open,
        engine::State::Closed => forge::State::Closed,
    }
}

fn kind_of(kind: engine::Kind) -> forge::Kind {
    match kind {
        engine::Kind::Issue => forge::Kind::Issue,
        engine::Kind::Pull => forge::Kind::Pull,
    }
}

/// `text`, cut to `most` bytes.
fn cut(text: &[u8], most: u32) -> Box<[u8]> {
    let most = usize::try_from(most).expect("fits");
    text[..text.len().min(most)].into()
}

fn summary(item: &forge::Summary, limits: &Limits) -> engine::Summary {
    engine::Summary {
        number: item.number,
        kind: match item.kind {
            forge::Kind::Issue => engine::Kind::Issue,
            forge::Kind::Pull => engine::Kind::Pull,
        },
        state: match item.state {
            forge::State::Open => engine::State::Open,
            forge::State::Closed => engine::State::Closed,
        },
        author: item.author,
        key: key_of(&item.body).map(Vec::into_boxed_slice),
        labels: item.labels.clone(),
        title: cut(&item.title, limits.title_bytes),
        body: cut(&item.body, limits.body_bytes),
        updated: item.updated,
    }
}

fn comment(comment: &forge::Comment, limits: &Limits) -> engine::Comment {
    engine::Comment {
        id: comment.id,
        author: comment.author,
        created: comment.created,
        revision: digest(&comment.body),
        mark: mark(&comment.body),
        body: cut(&comment.body, limits.body_bytes),
    }
}

/// A pull request, CI on its head combined.
fn pull(pull: &forge::Pull) -> engine::Pull {
    engine::Pull {
        number: pull.number,
        state: match pull.state {
            forge::State::Open => engine::State::Open,
            forge::State::Closed => engine::State::Closed,
        },
        head: pull.head.clone(),
        base: pull.base.clone(),
        commit: commit(pull.commit),
        base_commit: pull.base_commit.map(commit),
        merged: pull.merged.map(commit),
        mergeable: pull.mergeable,
        ci: combined(&pull.statuses),
    }
}

/// The `number`th page (from 1) of `size` of `all`, and whether more follow.
fn page_of<T>(all: &[T], number: u32, size: usize) -> (&[T], bool) {
    let skip = usize::try_from(number.saturating_sub(1)).expect("fits").saturating_mul(size);
    let rest = all.get(skip..).unwrap_or(&[]);
    (&rest[..rest.len().min(size)], rest.len() > size)
}

/// CI over every context's latest status, as Forgejo combines it.
#[must_use]
pub fn combined(statuses: &[forge::Status]) -> Ci {
    if statuses.is_empty() {
        return Ci::None;
    }
    if statuses.iter().any(|status| status.state == forge::Check::Failed) {
        return Ci::Failed;
    }
    if statuses.iter().any(|status| status.state == forge::Check::Pending) {
        return Ci::Pending;
    }
    Ci::Passed
}

fn review(review: &forge::Review, limits: &Limits) -> engine::Review {
    engine::Review {
        id: review.id,
        author: review.author,
        verdict: match review.verdict {
            forge::Verdict::Approve => engine::Verdict::Approve,
            forge::Verdict::RequestChanges => engine::Verdict::RequestChanges,
            forge::Verdict::Comment => engine::Verdict::Comment,
        },
        commit: commit(review.commit),
        key: key_of(&review.body).map(Vec::into_boxed_slice),
        body: cut(&review.body, limits.body_bytes),
    }
}

fn verdict_of(verdict: engine::Verdict) -> forge::Verdict {
    match verdict {
        engine::Verdict::Approve => forge::Verdict::Approve,
        engine::Verdict::RequestChanges => forge::Verdict::RequestChanges,
        engine::Verdict::Comment => forge::Verdict::Comment,
    }
}

fn status(status: &forge::Status) -> engine::Status {
    let (check, description): (engine::Check, &[u8]) = match status.state {
        forge::Check::Pending => (engine::Check::Pending, b"pending"),
        forge::Check::Passed => (engine::Check::Passed, b"passed"),
        forge::Check::Failed => (engine::Check::Failed, b"failed"),
    };
    engine::Status { context: status.context.clone(), check, description: description.into(), url: Box::new([]) }
}
