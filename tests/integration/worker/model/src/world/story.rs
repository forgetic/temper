//! What a scripted agent's run is about: its story, drawn from its charter
//! and the forge as observed, as the engine world's runs draw theirs
//! ([`temper_engine_model_tests::script`]). The agent world's script decides
//! when the agent speaks and how its run ends (it works, calls, pushes,
//! waits, parks, fails or misbehaves), taking the beats its story calls for
//! among its own steps and ending as its story does when its fate is to end
//! or park ([`Plot`]); this decides what its calls ask, what its pushes hold
//! in the file CI reads, and the outcome it ends with, so that the engine
//! reads a run that makes sense of what it was given.

use std::collections::VecDeque;

use temper_engine_model::forge::Read;
use temper_engine_model::plan::{self, Finish};
use temper_engine_model::{Call, Charter, Item, Outcome};
use temper_engine_model_tests::mirror::Mirror;
use temper_engine_model_tests::script::{self, Act, End};
use temper_worker_model_agent_tests::script::{Beat, Ending, Plot};

/// A run's content.
#[derive(Debug)]
pub(super) struct Story {
    item: Item,
    /// The calls its script makes, in order.
    calls: VecDeque<Call>,
    /// What its pushes write in the file CI reads, if it pushes.
    pub(super) cue: Option<Vec<u8>>,
    outcome: Option<Outcome>,
    /// What its agent is to do of it.
    pub(super) plot: Plot,
}

impl Story {
    /// The content of the run of `item` on `charter`, resuming `snapshot` if
    /// it was given one; the changes it asks for land into `base`.
    pub(super) fn new(item: Item, charter: &Charter, snapshot: Option<&[u8]>, mirror: &Mirror, base: &[u8]) -> Story {
        let mut calls = VecDeque::new();
        let mut cue = None;
        let mut outcome = None;
        let mut beats = VecDeque::new();
        let mut ending = Ending::Ended;
        for act in script::acts(item, charter, snapshot, mirror) {
            match act {
                Act::Call(call) => {
                    calls.push_back(call);
                    beats.push_back(Beat::Call { push: false });
                }
                Act::Push { content, .. } => {
                    cue = Some(content);
                    beats.push_back(Beat::Call { push: true });
                }
                Act::Await { within } => beats.push_back(Beat::Await { within }),
                Act::End(End::Ended(ended)) => outcome = Some(into(ended, base)),
                Act::End(End::Parked(snapshot)) => {
                    outcome = Some(fallback(charter.finish));
                    ending = Ending::Parked { snapshot };
                }
                Act::End(End::Failed(_)) => outcome = Some(fallback(charter.finish)),
                Act::Tell { .. } => {}
            }
        }
        Story { item, calls, cue, outcome, plot: Plot { beats, ending } }
    }

    /// What the run's next call asks: its script's next, then a read of its
    /// item.
    pub(super) fn call(&mut self) -> Call {
        let Item { repository, number } = self.item;
        let item = temper_engine_model::forge::Item { repository, number };
        self.calls.pop_front().unwrap_or(Call::Read(Read::Item { item, after: 0 }))
    }

    /// The outcome the run ends with.
    pub(super) fn outcome(&self) -> &Outcome {
        self.outcome.as_ref().expect("a script ends")
    }
}

/// `outcome`, the changes it asks for landing into `base`.
fn into(outcome: Outcome, base: &[u8]) -> Outcome {
    let Outcome::Tasks { tasks, text } = outcome else { return outcome };
    let tasks = tasks.into_iter().map(|step| {
        let work = match step.work {
            plan::Work::Change(change) => plan::Work::Change(plan::ChangeSpec { base: base.into(), ..change }),
            work @ (plan::Work::Agent(_) | plan::Work::Wait(_) | plan::Work::Session(_)) => work,
        };
        plan::Step { work, ..step }
    });
    Outcome::Tasks { tasks: tasks.collect(), text }
}

/// What a run whose script parks or fails ends with when its agent ends it
/// all the same: what its finish allows, saying the least.
fn fallback(finish: Finish) -> Outcome {
    let text: Box<[u8]> = b"nothing more".as_slice().into();
    match finish {
        Finish::Turn { .. } => Outcome::Reply { text },
        Finish::Report { .. } => Outcome::Report { text },
        Finish::Change { .. } => Outcome::Change { message: text },
        Finish::Verdict => Outcome::Verdict { verdict: plan::Verdict::Approve, text },
    }
}
