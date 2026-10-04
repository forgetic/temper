//! What a scripted run does (testing.md, 5.1): its acts, drawn from
//! its charter and from what the forge shows, as an agent would read them.
//! A scenario cues each run through content: a session by its item's title,
//! a step by the instructions its plan wrote, each starting with a cue
//! (`#fix`, `#red`, ...). Every script is level-triggered: it reads what is
//! on the forge now (the item's record, its children, the messages people
//! wrote), so a run retried, resumed or started after a restart does what
//! is due from there.

use skein_lib::Duration;
use temper_legacy_engine_domain::notes::{Author, Change, Page, Recall, Scope};
use temper_legacy_engine_domain::plan::{self, ChangeSpec, Finish, Repository, Review, Step, Why, Work};
use temper_legacy_engine_domain::views::Kind;
use temper_legacy_engine_domain::{Call, Charter, Failure, Item, Outcome};

use crate::deployment::{self, BUDGET, ENGINE, GREEN, MAIN};
use crate::mirror::Mirror;

/// One thing a run does.
#[derive(Debug)]
pub enum Act {
    /// Tells a fact, best effort.
    Tell { kind: Kind, content: Vec<u8> },
    /// Calls the engine, and waits for its answer.
    Call(Call),
    /// Writes `content` into the file CI reads, on its repository's start,
    /// and pushes it to the branch it may push to.
    Push { repository: u32, content: Vec<u8> },
    /// Waits for an inbound event, at most `within`.
    Await { within: Duration },
    /// Ends the run.
    End(End),
}

/// How a run ends.
#[derive(Debug)]
pub enum End {
    Ended(Outcome),
    Parked(Option<Vec<u8>>),
    Failed(Failure),
}

/// The cue at the head of `text`: its first word, if it starts with `#`.
#[must_use]
pub fn cue(text: &[u8]) -> &[u8] {
    let word = text.split(|byte| *byte == b' ').next().unwrap_or_default();
    if word.starts_with(b"#") { word } else { b"" }
}

/// What the run of `item` on `charter` does, resuming `snapshot` if it was
/// given one.
#[must_use]
pub fn acts(item: Item, charter: &Charter, snapshot: Option<&[u8]>, mirror: &Mirror) -> Vec<Act> {
    let name = deployment::name(item.repository);
    let title = mirror.issue(name, item.number).map(|issue| issue.title.clone()).unwrap_or_default();
    let mut acts = vec![Act::Tell { kind: Kind::Progress, content: b"started".to_vec() }];
    match charter.finish {
        Finish::Turn { supervising } => session(item, cue(&title), supervising, snapshot, mirror, &mut acts),
        Finish::Report { grows } => agent(item, cue(&charter.instructions), grows, &mut acts),
        Finish::Change { .. } => change(item, cue(&charter.instructions), charter.why, &mut acts),
        Finish::Verdict => {
            let outcome = Outcome::Verdict { verdict: plan::Verdict::Approve, text: b"looks right".as_slice().into() };
            acts.push(Act::End(End::Ended(outcome)));
        }
    }
    acts
}

/// A session's turn.
fn session(item: Item, cue: &[u8], supervising: bool, snapshot: Option<&[u8]>, mirror: &Mirror, acts: &mut Vec<Act>) {
    let name = deployment::name(item.repository);
    let record = mirror.record(name, item.number);
    let children = record.as_ref().map(|record| record.relations.children.to_vec()).unwrap_or_default();
    let messages = messages(mirror, item);
    let outcome = match cue {
        b"#fix" => {
            if children.is_empty() {
                Outcome::Tasks { tasks: Box::new([fix(item)]), text: text(b"on it: a change will follow") }
            } else if children.iter().all(|child| child.done.is_some()) {
                Outcome::Finished { text: text(b"fixed") }
            } else {
                Outcome::Reply { text: text(b"the change is under way") }
            }
        }
        b"#chat" => {
            if snapshot.is_some() || messages > 1 {
                // Resumed, or started afresh once it has chatted.
                acts.push(Act::Call(Call::Comment { text: text(b"welcome back") }));
                Outcome::Finished { text: text(b"bye") }
            } else {
                acts.push(Act::Call(Call::Comment { text: text(b"hi, what can I do?") }));
                acts.push(Act::Await { within: Duration::from_secs(120) });
                acts.push(Act::Call(Call::Comment { text: text(b"noted") }));
                acts.push(Act::Await { within: Duration::from_secs(5) });
                acts.push(Act::End(End::Parked(Some(b"chat: noted".to_vec()))));
                return;
            }
        }
        b"#note" => note(item, mirror, acts),
        b"#plan" | b"#grow" | b"#reject" | b"#burst" => {
            // Rejected, as its record counts until a release, or as its
            // person said after.
            let rejected = record.as_ref().is_some_and(|record| record.step.progress.rejections > 0)
                || (cue == b"#reject" && messages > 0);
            if supervising {
                // The goal's supervisor: done once its plan's steps are.
                if !children.is_empty() && children.iter().all(|child| child.done.is_some()) {
                    Outcome::Finished { text: text(b"the plan is done") }
                } else {
                    Outcome::Reply { text: text(b"the plan goes on") }
                }
            } else if rejected {
                Outcome::Finished { text: text(b"dropped, as you wish") }
            } else {
                let plan = proposal(item, cue);
                Outcome::Plan { plan, text: text(b"here is a plan") }
            }
        }
        // A session that says hello, and finishes once it is answered; one
        // to be stopped takes its time first.
        _ => {
            if cue == b"#stop" {
                acts.push(Act::Await { within: Duration::from_secs(60) });
            }
            if messages > 0 {
                Outcome::Finished { text: text(b"glad to help") }
            } else {
                Outcome::Reply { text: text(b"hello to you") }
            }
        }
    };
    acts.push(Act::End(End::Ended(outcome)));
}

/// A notes session: its first run writes a note, a later one recalls it.
fn note(item: Item, mirror: &Mirror, acts: &mut Vec<Act>) -> Outcome {
    let scope = Scope::Repository(item.repository);
    let name = deployment::name(item.repository);
    if mirror.pages.contains_key(&(name.to_vec(), NOTE.to_vec())) {
        acts.push(Act::Call(Call::Recall(Recall::Name { scope, name: NOTE.into() })));
        return Outcome::Finished { text: text(b"recalled") };
    }
    let page = Page {
        description: text(b"the build is slow"),
        author: Author::Run { repository: item.repository, number: item.number },
        references: Box::new([]),
        body: text(b"cache it"),
    };
    acts.push(Act::Call(Call::Note { scope, name: NOTE.into(), change: Change::New(page) }));
    Outcome::Reply { text: text(b"noted") }
}

/// The note a notes session writes.
pub const NOTE: &[u8] = b"slow-build";

/// An agent step: a report, or the steps a growing one adds.
fn agent(item: Item, cue: &[u8], grows: bool, acts: &mut Vec<Act>) {
    let outcome = if grows {
        // Two changes: within `#plan`'s envelope, beyond `#grow`'s.
        let steps = (0..2).map(|at| change_step(item, &[b'c', b'0' + at], b"#part", &[])).collect();
        Outcome::Steps { steps, text: text(b"the changes it takes") }
    } else {
        // A slow spike takes a minute longer than its sibling.
        if cue == b"#slow" {
            acts.push(Act::Await { within: Duration::from_secs(60) });
        }
        Outcome::Report { text: text(b"done: it fits") }
    };
    acts.push(Act::End(End::Ended(outcome)));
}

/// The plan a session proposes (engine-domain.md, 5.2): two spikes in
/// parallel, a person's decision, a design change, a build that grows the
/// plan with two changes, and a check. `#grow`'s envelope allows one change
/// only, so its build grows beyond it.
fn proposal(item: Item, cue: &[u8]) -> plan::Plan {
    let agent = |name: &[u8], instructions: &[u8], grows: bool, after: &[&[u8]]| Step {
        name: name.into(),
        repository: Repository(item.repository),
        work: Work::Agent(plan::AgentSpec { charter: charter(instructions), grows }),
        after: after.iter().map(|name| (*name).into()).collect(),
        gates: Box::new([]),
    };
    let decide = Step {
        name: text(b"decide"),
        repository: Repository(item.repository),
        work: Work::Wait(plan::WaitSpec::Decision),
        after: Box::new([text(b"spike-a"), text(b"spike-b")]),
        gates: Box::new([]),
    };
    // `#burst`'s second spike is slow: the spikes end a minute apart.
    let by_hand: &[u8] = if cue == b"#burst" { b"#slow spike by hand" } else { b"#spike by hand" };
    let steps = vec![
        agent(b"spike-a", b"#spike with a library", false, &[]),
        agent(b"spike-b", by_hand, false, &[]),
        decide,
        change_step(item, b"design", b"#design write it down", &[b"decide"]),
        agent(b"build", b"#build split it", true, &[b"design"]),
        agent(b"e2e", b"#e2e run the scenario", false, &[b"build"]),
    ];
    let changes = if cue == b"#grow" { 1 } else { 4 };
    let envelope = plan::Envelope {
        agents: 4,
        changes,
        waits: 1,
        sessions: 0,
        repositories: Box::new([Repository(item.repository)]),
        into: Box::new([plan::Target { repository: Repository(item.repository), base: MAIN.into() }]),
    };
    plan::Plan { steps: steps.into(), envelope, budget: 50_000 }
}

fn charter(instructions: &[u8]) -> plan::Charter {
    plan::Charter {
        instructions: instructions.into(),
        template: None,
        grants: plan::Grants { modify: true, shell: true, forge: true, subagents: false, note: true },
        budget: BUDGET,
    }
}

/// A change step into the default branch, reviewed by a person.
fn change_step(item: Item, name: &[u8], instructions: &[u8], after: &[&[u8]]) -> Step {
    Step {
        name: name.into(),
        repository: Repository(item.repository),
        work: Work::Change(ChangeSpec {
            base: MAIN.into(),
            produce: charter(instructions),
            checks: true,
            review: Review::Person,
        }),
        after: after.iter().map(|name| (*name).into()).collect(),
        gates: Box::new([]),
    }
}

/// A change step's run: it produces the change, or repairs it.
fn change(item: Item, cue: &[u8], why: Why, acts: &mut Vec<Act>) {
    // A change cued red fails CI when it is made, and its repairs pass.
    let content = if why == Why::Produce && cue == b"#red" { b"red".to_vec() } else { GREEN.to_vec() };
    acts.push(Act::Push { repository: item.repository, content });
    acts.push(Act::End(End::Ended(Outcome::Change { message: text(b"the change") })));
}

/// The change a `#fix` session asks for: one whose first push fails CI.
fn fix(item: Item) -> Step {
    let produce = plan::Charter {
        instructions: text(b"#red fix the build"),
        template: None,
        grants: plan::Grants { modify: true, shell: true, forge: true, subagents: false, note: false },
        budget: BUDGET,
    };
    Step {
        name: text(b"fix"),
        repository: Repository(item.repository),
        work: Work::Change(ChangeSpec { base: MAIN.into(), produce, checks: true, review: Review::Person }),
        after: Box::new([]),
        gates: Box::new([]),
    }
}

/// How many comments on the item are people's.
fn messages(mirror: &Mirror, item: Item) -> usize {
    mirror.messages(deployment::name(item.repository), item.number, ENGINE)
}

fn text(bytes: &[u8]) -> Box<[u8]> {
    bytes.into()
}
