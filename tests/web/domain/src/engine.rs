//! A scripted engine at the client's typed boundary. Keys become durable
//! before their answers; volatile decisions vanish on restart.
use crate::scenario::Scenario;
use skein_lib::Wall;
use std::collections::BTreeMap;
use temper_web_domain::{
    Answer, Ask, ChatLine, Chip, Choice, Decision, EndKind, Key, Outcome, PersonSnapshot, ReadResult, Refusal,
    TaskPhase, TaskResult, TaskSnapshot, Waiting,
};

#[derive(Debug)]
pub struct Engine {
    pub signed_in: bool,
    pub online: bool,
    pub person: temper_web_domain::Person,
    pub projects: Vec<temper_web_domain::Project>,
    pub chats: Vec<ChatLine>,
    pub tasks: BTreeMap<u64, TaskSnapshot>,
    pub durable: BTreeMap<Key, (Ask, Answer)>,
    pub decisions: BTreeMap<u64, (temper_web_domain::Person, Choice, Wall)>,
    pub reasons: BTreeMap<u64, Box<[u8]>>,
    pub next_task: u64,
    pub creations: u64,
    pub decision_count: u64,
}

impl Engine {
    #[must_use]
    pub fn new(scenario: Scenario) -> Engine {
        let next_task = scenario.chats.iter().map(|chat| chat.task).max().unwrap_or(0).saturating_add(1);
        Engine {
            signed_in: scenario.signed_in,
            online: true,
            person: scenario.person,
            projects: scenario.projects,
            chats: scenario.chats,
            tasks: scenario.tasks.into_iter().map(|task| (task.chip.task, task)).collect(),
            durable: BTreeMap::new(),
            decisions: BTreeMap::new(),
            reasons: BTreeMap::new(),
            next_task,
            creations: 0,
            decision_count: 0,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> PersonSnapshot {
        PersonSnapshot {
            person: self.person.clone(),
            projects: self.projects.clone().into_boxed_slice(),
            inbox_count: 0,
        }
    }

    #[must_use]
    pub fn read_chats(&self, project: Option<u32>, window: u32) -> ReadResult {
        if !self.signed_in {
            return ReadResult::SignedOut;
        }
        if !self.online {
            return ReadResult::Unreachable;
        }
        let rows = self
            .chats
            .iter()
            .filter(|row| project.is_none_or(|p| row.project == p))
            .take(usize::try_from(window).expect("window fits"))
            .cloned()
            .collect::<Vec<_>>();
        ReadResult::Chats { rows: rows.into_boxed_slice(), older: None }
    }

    #[must_use]
    pub fn task_snapshot(&self, number: u64) -> Option<TaskSnapshot> {
        if let Some(task) = self.tasks.get(&number) {
            return Some(task.clone());
        }
        let row = self.chats.iter().find(|row| row.task == number)?;
        Some(TaskSnapshot {
            chip: Chip {
                task: number,
                project: row.project,
                title: row.title.clone(),
                phase: TaskPhase::Running,
                spent: 0,
                budget: 100,
                revision: 1,
            },
            first_words: row.title.clone(),
            escalation: None,
            result: None,
        })
    }

    pub fn commit(&mut self, key: Key, ask: Ask, now: Wall) -> Answer {
        if let Some((old, answer)) = self.durable.get(&key) {
            if *old == ask {
                return answer.clone();
            }
            return Answer::Refused(Refusal::KeyConflict);
        }
        if !self.signed_in {
            return Answer::SignedOut;
        }
        if !self.online {
            return Answer::Unreachable;
        }
        let answer = match &ask {
            Ask::StartChat { project, words } => {
                if !self.projects.iter().any(|item| item.number == *project) {
                    Answer::Refused(Refusal::Role)
                } else if words.is_empty() {
                    Answer::Refused(Refusal::Unknown)
                } else {
                    let task = self.next_task;
                    self.next_task = self.next_task.saturating_add(1);
                    self.chats.insert(
                        0,
                        ChatLine { task, project: *project, title: words.clone(), live: true, last_activity: now },
                    );
                    self.creations += 1;
                    Answer::Done(Outcome::Started { task })
                }
            }
            Ask::Decide { waiting: Waiting::Escalation { task }, revision, decision } => {
                self.decide(*task, *revision, decision, now)
            }
        };
        self.durable.insert(key, (ask, answer.clone()));
        answer
    }

    fn decide(&mut self, task: u64, revision: u64, decision: &Decision, now: Wall) -> Answer {
        if let Some((person, choice, at)) = self.decisions.get(&task) {
            return Answer::Done(Outcome::DecidedBefore { by: person.clone(), choice: *choice, at: *at });
        }
        let Some(snapshot) = self.tasks.get_mut(&task) else {
            return Answer::Refused(Refusal::Unknown);
        };
        if snapshot.chip.revision != revision {
            return Answer::Refused(Refusal::Moved { revision: snapshot.chip.revision });
        }
        if snapshot.escalation.is_none() {
            return Answer::Refused(Refusal::Ended);
        }
        let choice = match decision {
            Decision::Release => Choice::Released,
            Decision::Reject { reason } => {
                self.reasons.insert(task, reason.clone());
                Choice::Rejected
            }
            Decision::Pass => Choice::Passed,
        };
        if choice == Choice::Released {
            snapshot.chip.phase = TaskPhase::Running;
        }
        snapshot.chip.revision = snapshot.chip.revision.saturating_add(1);
        snapshot.escalation = None;
        self.decisions.insert(task, (self.person.clone(), choice, now));
        self.decision_count += 1;
        Answer::Done(Outcome::Decided { choice })
    }

    pub fn move_revision(&mut self, task: u64) {
        let snapshot = self.tasks.get_mut(&task).expect("task exists");
        snapshot.chip.revision = snapshot.chip.revision.saturating_add(1);
        if let Some(escalation) = &mut snapshot.escalation {
            escalation.revision = snapshot.chip.revision;
        }
    }

    pub fn end(&mut self, task: u64, words: &[u8]) {
        let snapshot = self.tasks.get_mut(&task).expect("task exists");
        snapshot.chip.phase = TaskPhase::Ended(EndKind::Done);
        snapshot.chip.revision = snapshot.chip.revision.saturating_add(1);
        snapshot.escalation = None;
        snapshot.result =
            Some(TaskResult { task, kind: EndKind::Done, words: Box::from(words), revision: snapshot.chip.revision });
    }
}
