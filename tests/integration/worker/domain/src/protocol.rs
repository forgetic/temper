//! Between the engine and the worker, as their protocol layers would
//! translate (worker-domain.md, section 2; engine-domain.md, section 8): the
//! engine's boundary toward its workers and the worker's toward its engine,
//! each in its own terms, with what crosses opaque on the worker's side
//! encoded and decoded here, by the engine world's codecs where it has them.
//!
//! Names are packed as the engine's world packs them for every system world
//! ([`temper_engine_domain_tests::names`]): a run by its item, an attempt by
//! its item and its count. A repository is named on the worker's side by
//! its forge name (its remote) and a directory of its own (its name in the
//! deployment, less the owner), and reaches the forge as the one identity
//! the deployment gives its workers, [`IDENTITY`]. A commit is the engine's
//! 32 bytes, as the worker's.
//!
//! What the worker passes through unread is encoded on its way in, and
//! framed as a protocol layer may: a charter is the codec's, after the names
//! of the run and the attempt it is for; an inbound event says what it is,
//! and which comment it is news of if it is, after its place among the
//! events sent to its attempt, then the attempt's names. So the world can
//! tell, at the agent, which attempt a message was meant for and which
//! person's message it heard, and the scripted agent that its events come
//! once each, in order. A run's outcome is the codec's; a relayed call, and its answer,
//! are this module's own small encoding ([`call`], [`call_of`], [`served`]).

use std::collections::BTreeMap;

use temper_engine_domain::fleet::Bounce;
use temper_engine_domain::forge::{News, Read};
use temper_engine_domain::notes::{Change, Recall, Scope};
use temper_engine_domain::views::Kind;
use temper_engine_domain::{
    self as engine, Answer, Call, Charter, Failure, Hello, Hosted, Inbound, Item, Landed, Served, Start, Unserved, Work,
};
use temper_engine_domain_tests::{codec, deployment};
use temper_lib::Token;
use temper_worker_domain::{self as worker, host};

/// Who a worker is to the forge, for every repository: the deployment's
/// worker user's credentials.
pub const IDENTITY: &[u8] = b"temper-worker-identity";

/// The bytes a frame adds to a charter: the run's name, and the attempt's.
pub const CHARTER_FRAME: usize = 16;

pub use temper_engine_domain_tests::names::{attempt_of, item, run};

/// A run and an attempt, as the worker names them.
pub type Names = (Token, Token);

/// The worker's names for the item's attempt `attempt`.
#[must_use]
pub fn names(item: Item, attempt: u64) -> Names {
    (run(item), temper_engine_domain_tests::names::attempt(item, attempt))
}

/// The directory a repository of the deployment sits in: its name, less
/// its owner.
#[must_use]
pub fn directory(repository: u32) -> Box<[u8]> {
    let name = deployment::name(repository);
    let at = name.iter().position(|byte| *byte == b'/').map_or(0, |at| at + 1);
    name[at..].into()
}

/// The worker's assignment for the engine's, and the deployment's indexes
/// of its repositories in its order, for its answer's work.
#[must_use]
pub fn assignment(assignment: &engine::Assignment) -> (host::Assignment, Vec<u32>) {
    let engine::Assignment { item, attempt, workspace, save, charter, snapshot } = assignment;
    let (run, attempt) = names(*item, *attempt);
    let repositories = workspace.repositories.iter().map(|checkout| {
        let start = match &checkout.start {
            Start::Base { branch } => host::Start::Base { branch: branch.clone() },
            Start::Branch { branch } => host::Start::Branch { branch: branch.clone() },
            Start::Commit { commit } => host::Start::Commit { commit: *commit },
            Start::Saved { branch } => host::Start::Saved { branch: branch.clone() },
        };
        let access = match &checkout.push {
            Some(push) => host::Access::Writable { push: push.clone() },
            None => host::Access::ReadOnly,
        };
        host::Repository {
            name: directory(checkout.repository),
            remote: deployment::name(checkout.repository).into(),
            start,
            access,
            identity: IDENTITY.into(),
        }
    });
    let indexes = workspace.repositories.iter().map(|checkout| checkout.repository).collect();
    let assignment = host::Assignment {
        run,
        attempt,
        workspace: host::Workspace { key: workspace.key.clone(), repositories: repositories.collect() },
        save: save.clone(),
        charter: framed_charter(run, attempt, charter),
        snapshot: snapshot.clone(),
    };
    (assignment, indexes)
}

/// `charter`, encoded, framed with the names of the run and the attempt it
/// is for.
#[must_use]
pub fn framed_charter(run: Token, attempt: Token, charter: &Charter) -> Box<[u8]> {
    let encoded = codec::charter(charter);
    [run.raw().to_be_bytes().as_slice(), &attempt.raw().to_be_bytes(), &encoded].concat().into_boxed_slice()
}

/// The names of the run and the attempt a framed charter is for, and the
/// charter.
#[must_use]
pub fn charter_of(framed: &[u8]) -> (Names, Charter) {
    let (frame, encoded) = framed.split_at(CHARTER_FRAME);
    let charter = codec::charter_of(encoded).expect("a charter decodes as it was encoded");
    (frame_names(frame), charter)
}

fn frame_names(frame: &[u8]) -> Names {
    let word = |at: usize| u64::from_be_bytes(frame[at..at + 8].try_into().expect("eight bytes"));
    (Token::new(word(0)), Token::new(word(8)))
}

/// The inbound `event` for the run `run`'s attempt `attempt`, framed with its
/// place among the events sent to it: what it is, in a word, and the
/// comment it is news of, if it is one.
#[must_use]
pub fn framed_event(place: u64, (run, attempt): Names, event: &Inbound) -> Box<[u8]> {
    let (word, comment): (&[u8], Option<u64>) = match event {
        Inbound::News(News::Comment { id, .. }) => (COMMENT, Some(*id)),
        Inbound::News(News::Reviews { .. } | News::Pull { .. }) => (b"news", None),
        Inbound::Finished { .. } => (b"finished", None),
        Inbound::Held { .. } => (b"held", None),
        Inbound::Decided { accepted: true } => (b"accepted", None),
        Inbound::Decided { accepted: false } => (b"rejected", None),
    };
    let frame = [place.to_be_bytes(), run.raw().to_be_bytes(), attempt.raw().to_be_bytes()].concat();
    let comment = comment.map(u64::to_be_bytes).unwrap_or_default();
    [frame.as_slice(), word, &comment[..]].concat().into_boxed_slice()
}

/// The word of an inbound event that is news of a comment.
const COMMENT: &[u8] = b"comment";

/// The names of the attempt a framed inbound event was sent to.
#[must_use]
pub fn event_names(event: &[u8]) -> Names {
    frame_names(event.get(8..24).expect("an event begins with its frame"))
}

/// The comment a framed inbound event is news of, if it is.
#[must_use]
pub fn event_comment(event: &[u8]) -> Option<u64> {
    let id = event.get(24..)?.strip_prefix(COMMENT)?;
    Some(u64::from_be_bytes(id.try_into().expect("a comment's id")))
}

/// The engine's hello for the worker's.
#[must_use]
pub fn hello(hello: &worker::Hello) -> Hello {
    let hosting = hello.hosting.iter().map(|hosted| Hosted {
        item: item(hosted.run),
        attempt: attempt_of(hosted.attempt).1,
        phase: phase(hosted.phase),
    });
    Hello { slots: hello.slots, workstreams: hello.workstreams.clone(), hosting: hosting.collect() }
}

fn phase(phase: worker::Phase) -> engine::fleet::Phase {
    use engine::fleet::Phase;
    match phase {
        worker::Phase::Preparing => Phase::Preparing,
        worker::Phase::Starting => Phase::Starting,
        worker::Phase::Active => Phase::Active,
        worker::Phase::Waiting => Phase::Waiting,
        worker::Phase::Ending => Phase::Ending,
        worker::Phase::Answered => Phase::Answered,
    }
}

/// The engine's answer for the worker's, the work it left named by the
/// deployment's `repositories` in the assignment's order. An outcome that
/// does not decode is the agent's failure: it said what no engine reads.
#[must_use]
pub fn answer(answer: &host::Answer, repositories: &[u32]) -> Answer {
    let work = |work: &host::Work| {
        let landed = work.landed.iter().map(|landed| Landed {
            repository: repositories[usize::try_from(landed.repository).expect("a place")],
            commit: landed.commit,
        });
        Work { landed: landed.collect() }
    };
    match answer {
        host::Answer::Refused(host::Refusal::Busy) => Answer::Busy,
        host::Answer::Refused(host::Refusal::Invalid(_)) => Answer::Invalid,
        host::Answer::Ended { outcome, work: done } => match codec::outcome_of(outcome) {
            Some(outcome) => Answer::Ended { outcome, work: work(done) },
            None => Answer::Failed { failure: Failure::Agent, work: work(done) },
        },
        host::Answer::Parked { snapshot, work: done } => {
            Answer::Parked { snapshot: snapshot.clone(), work: work(done) }
        }
        host::Answer::Failed { failure, detail: _, work: done } => {
            Answer::Failed { failure: self::failure(*failure), work: work(done) }
        }
    }
}

/// The engine's class for the worker's failure: a preparation that may
/// succeed later, or a cancel, is transient; one that names what the forge
/// does not have, or refuses, is permanent.
fn failure(failure: host::Failure) -> Failure {
    match failure {
        host::Failure::Unprepared(host::Preparation::Transient) | host::Failure::Cancelled(_) => Failure::Transient,
        host::Failure::Unprepared(host::Preparation::Missing { .. } | host::Preparation::Refused { .. }) => {
            Failure::Permanent
        }
        host::Failure::Run(_) => Failure::Run,
        host::Failure::Agent(_) => Failure::Agent,
    }
}

/// The engine's name for the worker's bounce.
#[must_use]
pub fn bounce(bounce: host::Bounce) -> Bounce {
    match bounce {
        host::Bounce::TooLarge => Bounce::TooLarge,
        host::Bounce::Full => Bounce::Full,
        host::Bounce::Ending => Bounce::Ending,
    }
}

/// What the engine hears of a fact a run told: its progress, as it is.
#[must_use]
pub fn told(told: &worker::Told) -> engine::Event {
    let (item, attempt) = attempt_of(told.attempt);
    engine::Event::Told { item, attempt, kind: Kind::Progress, content: told.fact.clone() }
}

/// The bytes of a run's call: one of those the world's runs make.
#[must_use]
pub fn call(call: &Call) -> Vec<u8> {
    let number = |value: u64| value.to_string().into_bytes();
    let fields: Vec<Vec<u8>> = match call {
        Call::Comment { text } => vec![b"comment".to_vec(), text.to_vec()],
        Call::Escalate { text } => vec![b"escalate".to_vec(), text.to_vec()],
        Call::Read(Read::Item { item, after }) => {
            vec![b"read".to_vec(), number(u64::from(item.repository)), number(item.number), number(*after)]
        }
        Call::Recall(Recall::Name { scope: Scope::Repository(repository), name }) => {
            vec![b"recall".to_vec(), number(u64::from(*repository)), name.to_vec()]
        }
        Call::Note { scope: Scope::Repository(repository), name, change: Change::New(page) } => {
            vec![b"note".to_vec(), number(u64::from(*repository)), name.to_vec(), codec::page(page)]
        }
        Call::Read(_) | Call::Recall(_) | Call::Note { .. } => unreachable!("the world's runs make no such call"),
    };
    fields.join(&b'\n')
}

/// The call `bytes` encode, if they encode one.
#[must_use]
pub fn call_of(bytes: &[u8]) -> Option<Call> {
    let tag = bytes.split(|byte| *byte == b'\n').next()?;
    let parts = match tag {
        b"comment" | b"escalate" => 2,
        b"recall" => 3,
        b"read" | b"note" => 4,
        _ => return None,
    };
    let fields: Vec<&[u8]> = bytes.splitn(parts, |byte| *byte == b'\n').collect();
    if fields.len() != parts {
        return None;
    }
    let number = |field: &[u8]| -> Option<u64> { std::str::from_utf8(field).ok()?.parse().ok() };
    let call = match tag {
        b"comment" => Call::Comment { text: fields[1].into() },
        b"escalate" => Call::Escalate { text: fields[1].into() },
        b"read" => {
            let repository = u32::try_from(number(fields[1])?).ok()?;
            let item = engine::forge::Item { repository, number: number(fields[2])? };
            Call::Read(Read::Item { item, after: number(fields[3])? })
        }
        b"recall" => {
            let scope = Scope::Repository(u32::try_from(number(fields[1])?).ok()?);
            Call::Recall(Recall::Name { scope, name: fields[2].into() })
        }
        _ => {
            let scope = Scope::Repository(u32::try_from(number(fields[1])?).ok()?);
            let page = codec::page_of(fields[3])?;
            Call::Note { scope, name: fields[2].into(), change: Change::New(page) }
        }
    };
    Some(call)
}

/// The bytes of the engine's answer to a call, as the run reads it: what
/// came of it, in a word.
#[must_use]
pub fn served(served: &Served) -> Box<[u8]> {
    let word: &[u8] = match served {
        Served::Read(_) => b"read",
        Served::Recalled { .. } => b"recalled",
        Served::Noted(_) => b"noted",
        Served::Posted { .. } => b"posted",
        Served::Unserved(Unserved::Ungranted) => b"ungranted",
        Served::Unserved(Unserved::Busy) => b"busy",
        Served::Unserved(Unserved::Invalid) => b"invalid",
        Served::Unserved(Unserved::Refused) => b"refused",
        Served::Unserved(Unserved::Failed) => b"failed",
    };
    word.into()
}

/// What the protocol layer answers itself, to a relayed call it cannot
/// decode.
pub const UNDECODED: &[u8] = b"undecoded";

/// Inbound events framed for each attempt, by its names: the next one's
/// place.
pub type Places = BTreeMap<Names, u64>;
