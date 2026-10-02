//! The codecs of the engine's protocol layer, as the worlds play it
//! (engine-model.md, sections 4.1, 4.4 and 13): what the engine's typed
//! payloads look like as bytes on the forge and on a worker's channel, and
//! how they are read back. The engine's world uses them in place of the
//! protocol layer, and the system worlds reuse them.
//!
//! - **A record** is a comment the engine owns: the head the forge
//!   sub-model reads (its inbox position and its write's nonce, as
//!   [`temper_engine_model_forge_tests::translate::recorded`] writes it), then a typed
//!   block.
//! - **An outcome posted** is a comment keyed by its run and attempt: the
//!   key's marker, a line naming it an outcome, then a typed block.
//! - **A note's page** is a wiki page a person can read and correct: its
//!   description on the first line, who wrote it and what it refers to on
//!   the next two, then its body. A page the engine writes starts with its
//!   write's nonce.
//! - **A charter and an outcome** cross a worker's channel as opaque bytes:
//!   the engine encodes the charter it assigns, and decodes the outcome a
//!   run answers with.
//!
//! A typed block is the payload's bytes in hex, followed by a digest of
//! them, so that a block a person edited does not decode: the record, or
//! the outcome, is mangled. Every decoder refuses trailing bytes, counts
//! beyond what is left, and tags it does not know.

use temper_engine_model::brief::{self, Body, Section};
use temper_engine_model::notes::{Author, Page, Reference};
use temper_engine_model::plan::{
    self, AgentSpec, Batch, Budget, ChangeSpec, Commit, Decided, Decision, Envelope, Finish, Gate, Goal, Grants,
    Growth, Plan, Progress, Repair, Repository, Resume, Review, Reviewed, SessionSpec, Sources, Step, Target, WaitSpec,
    Wake, Why, Work,
};
use temper_engine_model::rules::Permission;
use temper_engine_model::views::{Capture, Policy};
use temper_engine_model::work::{Class, Failures, Hold, Lifecycle, Phase};
use temper_engine_model::{Charter, Decoded, Item, Outcome, Posted, Record, Related, Relations};
use temper_lib::{Duration, Time};

/// The markers at the head of what the engine writes, as the forge
/// sub-model's world writes them.
const KEY: &[u8] = b"<!-- temper:key ";
const RECORD: &[u8] = b"<!-- temper:record ";
const NONCE: &[u8] = b"<!-- temper:nonce ";
const END: &[u8] = b" -->\n";
/// The line that names a keyed comment an outcome.
const OUTCOME: &[u8] = b"temper outcome\n";

/// The bytes of a record's block, which go after its head.
#[must_use]
pub fn record_block(record: &Record) -> Vec<u8> {
    let mut out = Out::default();
    put_record(&mut out, record);
    block(&out.0)
}

/// The bytes of an outcome's comment, which go after its key's marker.
#[must_use]
pub fn posted_block(posted: &Posted) -> Vec<u8> {
    let mut out = Out::default();
    put_posted(&mut out, posted);
    let mut body = OUTCOME.to_vec();
    body.extend_from_slice(&block(&out.0));
    body
}

/// What the engine's comment `id`, of body `body`, holds: its record, or an
/// outcome posted; `None` if it holds neither, or one that does not decode.
#[must_use]
pub fn comment(id: u64, body: &[u8]) -> Option<Decoded> {
    if let Some(rest) = body.strip_prefix(RECORD) {
        let record = record_of(&unblock(after_head(rest)?)?)?;
        return Some(Decoded::Record { comment: id, record: Box::new(record) });
    }
    let rest = after_head(body.strip_prefix(KEY)?)?;
    let posted = posted_of(&unblock(rest.strip_prefix(OUTCOME)?)?)?;
    Some(Decoded::Outcome { comment: id, posted: Box::new(posted) })
}

/// Whether `body` starts as a record and its block decodes: one that starts
/// as a record and does not is a record a person mangled.
#[must_use]
pub fn is_whole_record(body: &[u8]) -> bool {
    match comment(0, body) {
        Some(Decoded::Record { .. }) => true,
        Some(Decoded::Outcome { .. } | Decoded::Page { .. }) | None => false,
    }
}

/// A note's page as the wiki shows it.
#[must_use]
pub fn page(page: &Page) -> Vec<u8> {
    let mut text = page.description.to_vec();
    text.push(b'\n');
    match page.author {
        Author::Person(person) => text.extend_from_slice(format!("by person {person}\n").as_bytes()),
        Author::Run { repository, number } => {
            text.extend_from_slice(format!("by run {repository}#{number}\n").as_bytes());
        }
    }
    text.extend_from_slice(b"refs");
    for reference in &page.references {
        text.extend_from_slice(format!(" {}#{}", reference.repository, reference.number).as_bytes());
    }
    text.push(b'\n');
    text.extend_from_slice(&page.body);
    text
}

/// The note a wiki page's `content` holds, the nonce of the write that made
/// it left out; `None` if it is not a note's page.
#[must_use]
pub fn page_of(content: &[u8]) -> Option<Page> {
    let content = match content.strip_prefix(NONCE) {
        Some(rest) => after_head(rest)?,
        None => content,
    };
    let (description, rest) = line(content)?;
    let (author, rest) = line(rest)?;
    let (references, body) = line(rest)?;
    let author = std::str::from_utf8(author).ok()?;
    let author = if let Some(person) = author.strip_prefix("by person ") {
        Author::Person(person.parse().ok()?)
    } else {
        let (repository, number) = author.strip_prefix("by run ")?.split_once('#')?;
        Author::Run { repository: repository.parse().ok()?, number: number.parse().ok()? }
    };
    let references = std::str::from_utf8(references.strip_prefix(b"refs")?).ok()?;
    let mut found = Vec::new();
    for reference in references.split(' ').skip(1) {
        let (repository, number) = reference.split_once('#')?;
        found.push(Reference { repository: repository.parse().ok()?, number: number.parse().ok()? });
    }
    Some(Page { description: description.into(), author, references: found.into(), body: body.into() })
}

/// A charter, as a worker carries it.
#[must_use]
pub fn charter(charter: &Charter) -> Vec<u8> {
    let mut out = Out::default();
    put_charter(&mut out, charter);
    out.0
}

/// The charter `bytes` carry, if they decode.
#[must_use]
pub fn charter_of(bytes: &[u8]) -> Option<Charter> {
    whole(bytes, get_charter)
}

/// An outcome, as a run answers with it.
#[must_use]
pub fn outcome(outcome: &Outcome) -> Vec<u8> {
    let mut out = Out::default();
    put_outcome(&mut out, outcome);
    out.0
}

/// The outcome `bytes` carry, if they decode.
#[must_use]
pub fn outcome_of(bytes: &[u8]) -> Option<Outcome> {
    whole(bytes, get_outcome)
}

fn record_of(bytes: &[u8]) -> Option<Record> {
    whole(bytes, get_record)
}

fn posted_of(bytes: &[u8]) -> Option<Posted> {
    whole(bytes, get_posted)
}

/// What follows the head `rest` starts with, up to its end marker.
fn after_head(rest: &[u8]) -> Option<&[u8]> {
    let end = rest.windows(END.len()).position(|window| window == END)?;
    Some(&rest[end + END.len()..])
}

/// The first line of `text`, and what follows it.
fn line(text: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = text.iter().position(|byte| *byte == b'\n')?;
    Some((&text[..end], &text[end + 1..]))
}

/// A typed block: `payload` and its digest, in hex.
fn block(payload: &[u8]) -> Vec<u8> {
    let mut bytes = payload.to_vec();
    bytes.extend_from_slice(&digest(payload).to_be_bytes());
    bytes.iter().flat_map(|byte| format!("{byte:02x}").into_bytes()).collect()
}

/// The payload of a typed block, if its digest holds.
fn unblock(text: &[u8]) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for pair in text.chunks(2) {
        bytes.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
    }
    let split = bytes.len().checked_sub(8)?;
    let (payload, sum) = bytes.split_at(split);
    let mut expected = [0; 8];
    expected.copy_from_slice(sum);
    if digest(payload) != u64::from_be_bytes(expected) {
        return None;
    }
    Some(payload.to_vec())
}

/// FNV-1a.
fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Decodes all of `bytes` with `get`.
fn whole<T>(bytes: &[u8], get: fn(&mut In<'_>) -> Option<T>) -> Option<T> {
    let mut input = In(bytes);
    let value = get(&mut input)?;
    if input.0.is_empty() { Some(value) } else { None }
}

/// Bytes written: integers big-endian, byte strings and lists after their
/// length, options and enums after a tag.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    fn len(&mut self, len: usize) {
        self.u32(u32::try_from(len).expect("a length fits a u32"));
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.len(bytes.len());
        self.0.extend_from_slice(bytes);
    }

    fn commit(&mut self, commit: &[u8; 32]) {
        self.0.extend_from_slice(commit);
    }

    fn time(&mut self, time: Time) {
        self.u64(time.as_nanos());
    }

    fn duration(&mut self, duration: Duration) {
        self.u64(duration.as_nanos());
    }

    fn option<T>(&mut self, value: Option<T>, put: fn(&mut Out, T)) {
        match value {
            Some(value) => {
                self.u8(1);
                put(self, value);
            }
            None => self.u8(0),
        }
    }

    fn list<T>(&mut self, values: &[T], put: fn(&mut Out, &T)) {
        self.len(values.len());
        for value in values {
            put(self, value);
        }
    }
}

/// Bytes being read, each read refusing what is not there.
struct In<'a>(&'a [u8]);

impl In<'_> {
    fn take(&mut self, count: usize) -> Option<&[u8]> {
        if self.0.len() < count {
            return None;
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(self.take(4)?);
        Some(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Option<u64> {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(self.take(8)?);
        Some(u64::from_be_bytes(bytes))
    }

    fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    /// A length, no more than the bytes left: every entry takes one at least.
    fn len(&mut self) -> Option<usize> {
        let len = usize::try_from(self.u32()?).ok()?;
        if len > self.0.len() { None } else { Some(len) }
    }

    fn bytes(&mut self) -> Option<Box<[u8]>> {
        let len = self.len()?;
        Some(self.take(len)?.into())
    }

    fn commit(&mut self) -> Option<[u8; 32]> {
        let mut commit = [0; 32];
        commit.copy_from_slice(self.take(32)?);
        Some(commit)
    }

    fn time(&mut self) -> Option<Time> {
        Some(Time::from_nanos(self.u64()?))
    }

    fn duration(&mut self) -> Option<Duration> {
        Some(Duration::from_nanos(self.u64()?))
    }

    #[expect(clippy::option_option, reason = "what failed to decode, or an option decoded")]
    fn option<T>(&mut self, get: fn(&mut In<'_>) -> Option<T>) -> Option<Option<T>> {
        match self.u8()? {
            0 => Some(None),
            1 => Some(Some(get(self)?)),
            _ => None,
        }
    }

    fn list<T>(&mut self, get: fn(&mut In<'_>) -> Option<T>) -> Option<Box<[T]>> {
        let len = self.len()?;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(get(self)?);
        }
        Some(values.into())
    }
}

fn put_bytes(out: &mut Out, bytes: &[u8]) {
    out.bytes(bytes);
}

fn get_bytes(input: &mut In<'_>) -> Option<Box<[u8]>> {
    input.bytes()
}

fn put_u64(out: &mut Out, value: u64) {
    out.u64(value);
}

fn get_u64(input: &mut In<'_>) -> Option<u64> {
    input.u64()
}

fn put_u32(out: &mut Out, value: u32) {
    out.u32(value);
}

fn get_u32(input: &mut In<'_>) -> Option<u32> {
    input.u32()
}

fn put_time(out: &mut Out, time: Time) {
    out.time(time);
}

fn get_time(input: &mut In<'_>) -> Option<Time> {
    input.time()
}

fn put_duration(out: &mut Out, duration: Duration) {
    out.duration(duration);
}

fn get_duration(input: &mut In<'_>) -> Option<Duration> {
    input.duration()
}

fn put_commit(out: &mut Out, commit: [u8; 32]) {
    out.commit(&commit);
}

fn get_commit(input: &mut In<'_>) -> Option<[u8; 32]> {
    input.commit()
}

fn put_item(out: &mut Out, item: Item) {
    out.u32(item.repository);
    out.u64(item.number);
}

fn get_item(input: &mut In<'_>) -> Option<Item> {
    Some(Item { repository: input.u32()?, number: input.u64()? })
}

fn put_record(out: &mut Out, record: &Record) {
    put_lifecycle(out, &record.lifecycle);
    put_step_record(out, &record.step);
    put_relations(out, &record.relations);
}

fn get_record(input: &mut In<'_>) -> Option<Record> {
    Some(Record { lifecycle: get_lifecycle(input)?, step: get_step_record(input)?, relations: get_relations(input)? })
}

fn put_lifecycle(out: &mut Out, lifecycle: &Lifecycle) {
    match lifecycle.phase {
        Phase::Waiting => out.u8(0),
        Phase::Parked => out.u8(1),
        Phase::Retrying(class) => {
            out.u8(2);
            put_class(out, class);
        }
        Phase::Claimed => out.u8(3),
        Phase::Applying { outcome } => {
            out.u8(4);
            out.u64(outcome);
        }
        Phase::Held { why, outcome } => {
            out.u8(5);
            put_hold(out, why);
            out.option(outcome, put_u64);
        }
        Phase::Done => out.u8(6),
    }
    out.u64(lifecycle.attempts);
    let failures = lifecycle.failures;
    for count in [failures.transient, failures.permanent, failures.run, failures.agent, failures.lost, failures.invalid]
    {
        out.u32(count);
    }
}

fn get_lifecycle(input: &mut In<'_>) -> Option<Lifecycle> {
    let phase = match input.u8()? {
        0 => Phase::Waiting,
        1 => Phase::Parked,
        2 => Phase::Retrying(get_class(input)?),
        3 => Phase::Claimed,
        4 => Phase::Applying { outcome: input.u64()? },
        5 => Phase::Held { why: get_hold(input)?, outcome: input.option(get_u64)? },
        6 => Phase::Done,
        _ => return None,
    };
    let attempts = input.u64()?;
    let failures = Failures {
        transient: input.u32()?,
        permanent: input.u32()?,
        run: input.u32()?,
        agent: input.u32()?,
        lost: input.u32()?,
        invalid: input.u32()?,
    };
    Some(Lifecycle { phase, attempts, failures })
}

fn put_class(out: &mut Out, class: Class) {
    out.u8(match class {
        Class::Transient => 0,
        Class::Permanent => 1,
        Class::Run => 2,
        Class::Agent => 3,
        Class::Lost => 4,
        Class::Invalid => 5,
    });
}

fn get_class(input: &mut In<'_>) -> Option<Class> {
    Some(match input.u8()? {
        0 => Class::Transient,
        1 => Class::Permanent,
        2 => Class::Run,
        3 => Class::Agent,
        4 => Class::Lost,
        5 => Class::Invalid,
        _ => return None,
    })
}

fn put_hold(out: &mut Out, hold: Hold) {
    match hold {
        Hold::Plan { reason } => {
            out.u8(0);
            out.u32(reason);
        }
        Hold::Failures(class) => {
            out.u8(1);
            put_class(out, class);
        }
        Hold::Stopped => out.u8(2),
        Hold::Acceptance => out.u8(3),
        Hold::Writes => out.u8(4),
        Hold::Record => out.u8(5),
    }
}

fn get_hold(input: &mut In<'_>) -> Option<Hold> {
    Some(match input.u8()? {
        0 => Hold::Plan { reason: input.u32()? },
        1 => Hold::Failures(get_class(input)?),
        2 => Hold::Stopped,
        3 => Hold::Acceptance,
        4 => Hold::Writes,
        5 => Hold::Record,
        _ => return None,
    })
}

fn put_step_record(out: &mut Out, record: &plan::Record) {
    put_step(out, &record.step);
    put_progress(out, &record.progress);
    out.option(record.goal.as_ref(), put_goal);
}

fn get_step_record(input: &mut In<'_>) -> Option<plan::Record> {
    Some(plan::Record { step: get_step(input)?, progress: get_progress(input)?, goal: input.option(get_goal)? })
}

fn put_step(out: &mut Out, step: &Step) {
    out.bytes(&step.name);
    out.u32(step.repository.0);
    match &step.work {
        Work::Agent(spec) => {
            out.u8(0);
            put_plan_charter(out, &spec.charter);
            out.bool(spec.grows);
        }
        Work::Change(spec) => {
            out.u8(1);
            out.bytes(&spec.base);
            put_plan_charter(out, &spec.produce);
            out.bool(spec.checks);
            match &spec.review {
                Review::Person => out.u8(0),
                Review::Agent(charter) => {
                    out.u8(1);
                    put_plan_charter(out, charter);
                }
            }
        }
        Work::Wait(spec) => {
            out.u8(2);
            match spec {
                WaitSpec::Steps => out.u8(0),
                WaitSpec::Decision => out.u8(1),
                WaitSpec::Time(after) => {
                    out.u8(2);
                    out.duration(*after);
                }
            }
        }
        Work::Session(spec) => {
            out.u8(3);
            put_session(out, spec);
        }
    }
    out.list(&step.after, |out, name| out.bytes(name));
    out.list(&step.gates, |out, gate| put_gate(out, *gate));
}

fn get_step(input: &mut In<'_>) -> Option<Step> {
    let name = input.bytes()?;
    let repository = Repository(input.u32()?);
    let work = match input.u8()? {
        0 => Work::Agent(AgentSpec { charter: get_plan_charter(input)?, grows: input.bool()? }),
        1 => {
            let base = input.bytes()?;
            let produce = get_plan_charter(input)?;
            let checks = input.bool()?;
            let review = match input.u8()? {
                0 => Review::Person,
                1 => Review::Agent(get_plan_charter(input)?),
                _ => return None,
            };
            Work::Change(ChangeSpec { base, produce, checks, review })
        }
        2 => Work::Wait(match input.u8()? {
            0 => WaitSpec::Steps,
            1 => WaitSpec::Decision,
            2 => WaitSpec::Time(input.duration()?),
            _ => return None,
        }),
        3 => Work::Session(get_session(input)?),
        _ => return None,
    };
    let after = input.list(get_bytes)?;
    let gates = input.list(get_gate)?;
    Some(Step { name, repository, work, after, gates })
}

fn put_gate(out: &mut Out, gate: Gate) {
    match gate {
        Gate::Approvals(count) => {
            out.u8(0);
            out.u32(count);
        }
        Gate::Accepted => out.u8(1),
    }
}

fn get_gate(input: &mut In<'_>) -> Option<Gate> {
    Some(match input.u8()? {
        0 => Gate::Approvals(input.u32()?),
        1 => Gate::Accepted,
        _ => return None,
    })
}

fn put_session(out: &mut Out, spec: &SessionSpec) {
    put_plan_charter(out, &spec.charter);
    out.u8(match spec.resume {
        Resume::Default => 0,
        Resume::Always => 1,
        Resume::Never => 2,
    });
    let on = spec.wake.on;
    for source in [on.own, on.related, on.subscribed, on.messages] {
        out.bool(source);
    }
    out.option(spec.wake.every, put_duration);
    out.u32(spec.wake.batch.count);
    out.option(spec.wake.batch.age, put_duration);
}

fn get_session(input: &mut In<'_>) -> Option<SessionSpec> {
    let charter = get_plan_charter(input)?;
    let resume = match input.u8()? {
        0 => Resume::Default,
        1 => Resume::Always,
        2 => Resume::Never,
        _ => return None,
    };
    let on = Sources { own: input.bool()?, related: input.bool()?, subscribed: input.bool()?, messages: input.bool()? };
    let every = input.option(get_duration)?;
    let batch = Batch { count: input.u32()?, age: input.option(get_duration)? };
    Some(SessionSpec { charter, resume, wake: Wake { on, every, batch } })
}

fn put_plan_charter(out: &mut Out, charter: &plan::Charter) {
    out.bytes(&charter.instructions);
    out.option(charter.template.as_deref(), put_bytes);
    put_grants(out, charter.grants);
    put_budget(out, charter.budget);
}

fn get_plan_charter(input: &mut In<'_>) -> Option<plan::Charter> {
    Some(plan::Charter {
        instructions: input.bytes()?,
        template: input.option(get_bytes)?,
        grants: get_grants(input)?,
        budget: get_budget(input)?,
    })
}

fn put_grants(out: &mut Out, grants: Grants) {
    for grant in [grants.modify, grants.shell, grants.forge, grants.subagents, grants.note] {
        out.bool(grant);
    }
}

fn get_grants(input: &mut In<'_>) -> Option<Grants> {
    Some(Grants {
        modify: input.bool()?,
        shell: input.bool()?,
        forge: input.bool()?,
        subagents: input.bool()?,
        note: input.bool()?,
    })
}

fn put_budget(out: &mut Out, budget: Budget) {
    out.u64(budget.tokens);
    out.u32(budget.turns);
    out.duration(budget.time);
}

fn get_budget(input: &mut In<'_>) -> Option<Budget> {
    Some(Budget { tokens: input.u64()?, turns: input.u32()?, time: input.duration()? })
}

fn put_progress(out: &mut Out, progress: &Progress) {
    out.bool(progress.finished);
    out.option(progress.running, put_why);
    out.option(progress.last_run, put_time);
    for count in [progress.runs, progress.repairs, progress.rebases, progress.rejections] {
        out.u32(count);
    }
    out.option(progress.review, put_reviewed);
    out.option(progress.released, put_time);
}

fn get_progress(input: &mut In<'_>) -> Option<Progress> {
    Some(Progress {
        finished: input.bool()?,
        running: input.option(get_why)?,
        last_run: input.option(get_time)?,
        runs: input.u32()?,
        repairs: input.u32()?,
        rebases: input.u32()?,
        rejections: input.u32()?,
        review: input.option(get_reviewed)?,
        released: input.option(get_time)?,
    })
}

fn put_why(out: &mut Out, why: Why) {
    match why {
        Why::Work => out.u8(0),
        Why::Produce => out.u8(1),
        Why::Repair(repair) => {
            out.u8(2);
            out.u8(match repair {
                Repair::CiFailed => 0,
                Repair::ChangesRequested => 1,
                Repair::BaseMoved => 2,
                Repair::Conflicts => 3,
            });
        }
        Why::Review { head } => {
            out.u8(3);
            out.commit(&head.0);
        }
        Why::Turn => out.u8(4),
    }
}

fn get_why(input: &mut In<'_>) -> Option<Why> {
    Some(match input.u8()? {
        0 => Why::Work,
        1 => Why::Produce,
        2 => Why::Repair(match input.u8()? {
            0 => Repair::CiFailed,
            1 => Repair::ChangesRequested,
            2 => Repair::BaseMoved,
            3 => Repair::Conflicts,
            _ => return None,
        }),
        3 => Why::Review { head: Commit(input.commit()?) },
        4 => Why::Turn,
        _ => return None,
    })
}

fn put_reviewed(out: &mut Out, reviewed: Reviewed) {
    out.commit(&reviewed.head.0);
    put_verdict(out, reviewed.verdict);
}

fn get_reviewed(input: &mut In<'_>) -> Option<Reviewed> {
    Some(Reviewed { head: Commit(input.commit()?), verdict: get_verdict(input)? })
}

fn put_verdict(out: &mut Out, verdict: plan::Verdict) {
    out.u8(match verdict {
        plan::Verdict::Approve => 0,
        plan::Verdict::Changes => 1,
    });
}

fn get_verdict(input: &mut In<'_>) -> Option<plan::Verdict> {
    Some(match input.u8()? {
        0 => plan::Verdict::Approve,
        1 => plan::Verdict::Changes,
        _ => return None,
    })
}

fn put_goal(out: &mut Out, goal: &Goal) {
    out.list(&goal.steps, put_entry);
    put_envelope(out, &goal.envelope);
    out.u64(goal.budget);
    out.u64(goal.estimate);
    let growth = goal.growth;
    for count in [growth.agents, growth.changes, growth.waits, growth.sessions] {
        out.u32(count);
    }
}

fn get_goal(input: &mut In<'_>) -> Option<Goal> {
    Some(Goal {
        steps: input.list(get_entry)?,
        envelope: get_envelope(input)?,
        budget: input.u64()?,
        estimate: input.u64()?,
        growth: Growth { agents: input.u32()?, changes: input.u32()?, waits: input.u32()?, sessions: input.u32()? },
    })
}

fn put_entry(out: &mut Out, entry: &plan::Entry) {
    out.bytes(&entry.name);
    out.list(&entry.after, |out, name| out.bytes(name));
    out.option(entry.parent, put_u32);
    out.u32(entry.run);
}

fn get_entry(input: &mut In<'_>) -> Option<plan::Entry> {
    Some(plan::Entry {
        name: input.bytes()?,
        after: input.list(get_bytes)?,
        parent: input.option(get_u32)?,
        run: input.u32()?,
    })
}

fn put_envelope(out: &mut Out, envelope: &Envelope) {
    for count in [envelope.agents, envelope.changes, envelope.waits, envelope.sessions] {
        out.u32(count);
    }
    out.list(&envelope.repositories, |out, repository| out.u32(repository.0));
    out.list(&envelope.into, put_target);
}

fn get_envelope(input: &mut In<'_>) -> Option<Envelope> {
    Some(Envelope {
        agents: input.u32()?,
        changes: input.u32()?,
        waits: input.u32()?,
        sessions: input.u32()?,
        repositories: input.list(get_repository)?,
        into: input.list(get_target)?,
    })
}

fn get_repository(input: &mut In<'_>) -> Option<Repository> {
    Some(Repository(input.u32()?))
}

fn put_target(out: &mut Out, target: &Target) {
    out.u32(target.repository.0);
    out.bytes(&target.base);
}

fn get_target(input: &mut In<'_>) -> Option<Target> {
    Some(Target { repository: Repository(input.u32()?), base: input.bytes()? })
}

fn put_relations(out: &mut Out, relations: &Relations) {
    out.time(relations.created);
    out.option(relations.goal, put_item);
    out.option(relations.parent, put_item);
    out.option(relations.pull, put_u64);
    out.option(relations.branch, put_commit);
    out.list(&relations.dependencies, put_related);
    out.list(&relations.children, put_related);
    out.option(relations.decision, put_decided);
    out.option(relations.accepted, put_permission);
    out.bool(relations.snapshot);
    out.u64(relations.spent);
}

fn get_relations(input: &mut In<'_>) -> Option<Relations> {
    Some(Relations {
        created: input.time()?,
        goal: input.option(get_item)?,
        parent: input.option(get_item)?,
        pull: input.option(get_u64)?,
        branch: input.option(get_commit)?,
        dependencies: input.list(get_related)?,
        children: input.list(get_related)?,
        decision: input.option(get_decided)?,
        accepted: input.option(get_permission)?,
        snapshot: input.bool()?,
        spent: input.u64()?,
    })
}

fn put_related(out: &mut Out, related: &Related) {
    out.bytes(&related.name);
    put_item(out, related.item);
    out.option(related.done, put_time);
}

fn get_related(input: &mut In<'_>) -> Option<Related> {
    Some(Related { name: input.bytes()?, item: get_item(input)?, done: input.option(get_time)? })
}

fn put_decided(out: &mut Out, decided: Decided) {
    out.u8(match decided.decision {
        Decision::Accepted => 0,
        Decision::Rejected => 1,
    });
    out.time(decided.at);
}

fn get_decided(input: &mut In<'_>) -> Option<Decided> {
    let decision = match input.u8()? {
        0 => Decision::Accepted,
        1 => Decision::Rejected,
        _ => return None,
    };
    Some(Decided { decision, at: input.time()? })
}

fn put_permission(out: &mut Out, permission: Permission) {
    out.u8(match permission {
        Permission::None => 0,
        Permission::Read => 1,
        Permission::Write => 2,
        Permission::Admin => 3,
    });
}

fn get_permission(input: &mut In<'_>) -> Option<Permission> {
    Some(match input.u8()? {
        0 => Permission::None,
        1 => Permission::Read,
        2 => Permission::Write,
        3 => Permission::Admin,
        _ => return None,
    })
}

fn put_posted(out: &mut Out, posted: &Posted) {
    out.u64(posted.attempt);
    put_outcome(out, &posted.outcome);
    out.option(posted.head, put_commit);
}

fn get_posted(input: &mut In<'_>) -> Option<Posted> {
    Some(Posted { attempt: input.u64()?, outcome: get_outcome(input)?, head: input.option(get_commit)? })
}

fn put_outcome(out: &mut Out, outcome: &Outcome) {
    match outcome {
        Outcome::Change { message } => {
            out.u8(0);
            out.bytes(message);
        }
        Outcome::Verdict { verdict, text } => {
            out.u8(1);
            put_verdict(out, *verdict);
            out.bytes(text);
        }
        Outcome::Report { text } => {
            out.u8(2);
            out.bytes(text);
        }
        Outcome::Plan { plan, text } => {
            out.u8(3);
            put_plan(out, plan);
            out.bytes(text);
        }
        Outcome::Steps { steps, text } => {
            out.u8(4);
            out.list(steps, put_step);
            out.bytes(text);
        }
        Outcome::Tasks { tasks, text } => {
            out.u8(5);
            out.list(tasks, put_step);
            out.bytes(text);
        }
        Outcome::Reply { text } => {
            out.u8(6);
            out.bytes(text);
        }
        Outcome::Finished { text } => {
            out.u8(7);
            out.bytes(text);
        }
        Outcome::Release { step, text } => {
            out.u8(8);
            out.bytes(step);
            out.bytes(text);
        }
        Outcome::Escalation { text } => {
            out.u8(9);
            out.bytes(text);
        }
    }
}

fn get_outcome(input: &mut In<'_>) -> Option<Outcome> {
    Some(match input.u8()? {
        0 => Outcome::Change { message: input.bytes()? },
        1 => Outcome::Verdict { verdict: get_verdict(input)?, text: input.bytes()? },
        2 => Outcome::Report { text: input.bytes()? },
        3 => Outcome::Plan { plan: get_plan(input)?, text: input.bytes()? },
        4 => Outcome::Steps { steps: input.list(get_step)?, text: input.bytes()? },
        5 => Outcome::Tasks { tasks: input.list(get_step)?, text: input.bytes()? },
        6 => Outcome::Reply { text: input.bytes()? },
        7 => Outcome::Finished { text: input.bytes()? },
        8 => Outcome::Release { step: input.bytes()?, text: input.bytes()? },
        9 => Outcome::Escalation { text: input.bytes()? },
        _ => return None,
    })
}

fn put_plan(out: &mut Out, plan: &Plan) {
    out.list(&plan.steps, put_step);
    put_envelope(out, &plan.envelope);
    out.u64(plan.budget);
}

fn get_plan(input: &mut In<'_>) -> Option<Plan> {
    Some(Plan { steps: input.list(get_step)?, envelope: get_envelope(input)?, budget: input.u64()? })
}

fn put_charter(out: &mut Out, charter: &Charter) {
    put_why(out, charter.why);
    out.list(&charter.brief, put_section);
    out.bytes(&charter.instructions);
    put_grants(out, charter.grants);
    match charter.finish {
        Finish::Report { grows } => {
            out.u8(0);
            out.bool(grows);
        }
        Finish::Change { checks } => {
            out.u8(1);
            out.bool(checks);
        }
        Finish::Verdict => out.u8(2),
        Finish::Turn { supervising } => {
            out.u8(3);
            out.bool(supervising);
        }
    }
    put_budget(out, charter.budget);
    out.bytes(&charter.models);
    let policy = charter.policy;
    for capture in [policy.text, policy.progress, policy.calls, policy.tools, policy.usage] {
        out.u8(match capture {
            Capture::Nothing => 0,
            Capture::Shape => 1,
            Capture::Content => 2,
        });
    }
}

fn get_charter(input: &mut In<'_>) -> Option<Charter> {
    let why = get_why(input)?;
    let brief = input.list(get_section)?;
    let instructions = input.bytes()?;
    let grants = get_grants(input)?;
    let finish = match input.u8()? {
        0 => Finish::Report { grows: input.bool()? },
        1 => Finish::Change { checks: input.bool()? },
        2 => Finish::Verdict,
        3 => Finish::Turn { supervising: input.bool()? },
        _ => return None,
    };
    let budget = get_budget(input)?;
    let models = input.bytes()?;
    let policy = Policy {
        text: get_capture(input)?,
        progress: get_capture(input)?,
        calls: get_capture(input)?,
        tools: get_capture(input)?,
        usage: get_capture(input)?,
    };
    Some(Charter { why, brief, instructions, grants, finish, budget, models, policy })
}

fn get_capture(input: &mut In<'_>) -> Option<Capture> {
    Some(match input.u8()? {
        0 => Capture::Nothing,
        1 => Capture::Shape,
        2 => Capture::Content,
        _ => return None,
    })
}

fn put_section(out: &mut Out, section: &Section) {
    out.u8(match section.kind {
        brief::Kind::Item => 0,
        brief::Kind::Comments => 1,
        brief::Kind::Dependencies => 2,
        brief::Kind::Ci => 3,
        brief::Kind::Reviews => 4,
        brief::Kind::Pull => 5,
        brief::Kind::Attempts => 6,
        brief::Kind::Plan => 7,
        brief::Kind::Notes => 8,
        brief::Kind::Template => 9,
    });
    match &section.body {
        Body::Text(text) => {
            out.u8(0);
            out.bytes(text);
        }
        Body::Missing => out.u8(1),
    }
}

fn get_section(input: &mut In<'_>) -> Option<Section> {
    let kind = match input.u8()? {
        0 => brief::Kind::Item,
        1 => brief::Kind::Comments,
        2 => brief::Kind::Dependencies,
        3 => brief::Kind::Ci,
        4 => brief::Kind::Reviews,
        5 => brief::Kind::Pull,
        6 => brief::Kind::Attempts,
        7 => brief::Kind::Plan,
        8 => brief::Kind::Notes,
        9 => brief::Kind::Template,
        _ => return None,
    };
    let body = match input.u8()? {
        0 => Body::Text(input.bytes()?),
        1 => Body::Missing,
        _ => return None,
    };
    Some(Section { kind, body })
}
