//! Transport timing and translated answers for version-two turns. Jig's
//! worker host owns turn bytes and credit; this link schedules retries and
//! keeps application-shaped answers until the engine acknowledges them.

use crate::boundary::{Hosted, Phase, Request};
use crate::limits::Limits;
use crate::wire;
use jig_host as host;
use skein_lib::bytes::copy_of;
use skein_lib::{Deadlines, Env, Map, Queue, Time, Token};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct Name {
    pub(crate) run: Token,
    pub(crate) attempt: Token,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct TurnName {
    pub(crate) run: Token,
    pub(crate) attempt: Token,
    pub(crate) turn: u32,
}

#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) answer: wire::AnswerV2,
    acknowledged: bool,
}

#[derive(Debug)]
pub(crate) struct Turns {
    answers: Map<Name, Answer>,
    retry: Deadlines<TurnName>,
    abandoned: u64,
}

impl Turns {
    pub(crate) fn new(limits: &Limits) -> Turns {
        let capacity = limits.host.slots.checked_mul(limits.host.turns).expect("worst_case accepted turn capacity");
        Turns {
            answers: Map::with_capacity(limits.host.slots),
            retry: Deadlines::with_capacity(capacity),
            abandoned: 0,
        }
    }

    pub(crate) fn held(&self) -> u32 {
        self.answers.len()
    }

    pub(crate) fn holds(&self, run: Token, attempt: Token) -> bool {
        self.answers.contains_key(&Name { run, attempt })
    }

    pub(crate) const fn abandoned(&self) -> u64 {
        self.abandoned
    }

    pub(crate) fn next_deadline(&self) -> Option<Time> {
        self.retry.next()
    }

    pub(crate) fn hosting(&self, runs: &mut skein_lib::List<Hosted>) {
        for (name, _) in &self.answers {
            runs.push(Hosted { run: name.run, attempt: name.attempt, phase: Phase::Answered })
                .expect("an answer holds a slot");
        }
    }

    pub(crate) fn acknowledge_turn(&mut self, run: Token, attempt: Token, turn: u32, host: &host::Domain) {
        self.retry.cancel(TurnName { run, attempt, turn });
        self.release_answer(run, attempt, host);
    }

    pub(crate) fn busy(&mut self, run: Token, attempt: Token, turn: u32, env: &Env<Limits>, host: &host::Domain) {
        let name = TurnName { run, attempt, turn };
        if host.has_turn(run, attempt, turn) {
            self.retry.arm(name, env.now.saturating_add(env.limits.turn_backoff)).expect("one retry per retained turn");
        }
    }

    pub(crate) fn fire(&mut self, now: Time, up: bool, host: &host::Domain, out: &mut Queue<Request>) {
        let Some(name) = self.retry.expire(now) else {
            return;
        };
        if up && let Some(turn) = host.turn(name.run, name.attempt, name.turn) {
            emit(name.run, name.attempt, turn, out);
        }
    }

    pub(crate) fn answer(
        &mut self,
        run: Token,
        attempt: Token,
        answer: wire::AnswerV2,
        up: bool,
        out: &mut Queue<Request>,
    ) {
        let refused = match answer.ending {
            wire::EndingV2::Refused(_) => true,
            wire::EndingV2::Ended { .. } | wire::EndingV2::Parked { .. } | wire::EndingV2::Failed { .. } => false,
        };
        if refused {
            if up {
                out.push(Request::AnswerV2 { run, attempt, answer });
            }
            return;
        }
        if up {
            out.push(Request::AnswerV2 { run, attempt, answer: copy_answer(&answer) });
        }
        let old = self
            .answers
            .insert(Name { run, attempt }, Answer { answer, acknowledged: false })
            .expect("one answer per slot");
        assert!(old.is_none(), "one answer per admitted attempt");
    }

    pub(crate) fn answer_acknowledged(&mut self, run: Token, attempt: Token, host: &host::Domain) {
        if let Some(answer) = self.answers.get_mut(&Name { run, attempt }) {
            answer.acknowledged = true;
        }
        self.release_answer(run, attempt, host);
    }

    fn release_answer(&mut self, run: Token, attempt: Token, host: &host::Domain) {
        if host.has_turns_for(run, attempt) {
            return;
        }
        let name = Name { run, attempt };
        let released = match self.answers.get(&name) {
            Some(answer) => answer.acknowledged,
            None => false,
        };
        if released {
            self.answers.remove(&name);
        }
    }

    pub(crate) fn hello(&mut self, host: &host::Domain, out: &mut Queue<Request>) {
        for index in 0..host.retained_turns() {
            let (run, attempt, turn) = host.turn_at(index).expect("every retained position exists");
            self.retry.cancel(TurnName { run, attempt, turn: turn.turn });
            emit(run, attempt, turn, out);
        }
        for (name, answer) in &self.answers {
            if !answer.acknowledged {
                out.push(Request::AnswerV2 {
                    run: name.run,
                    attempt: name.attempt,
                    answer: copy_answer(&answer.answer),
                });
            }
        }
    }

    pub(crate) fn give_up(&mut self, host: &host::Domain) {
        for index in 0..host.retained_turns() {
            let (run, attempt, turn) = host.turn_at(index).expect("every retained position exists");
            self.retry.cancel(TurnName { run, attempt, turn: turn.turn });
        }
        for _ in 0..self.answers.capacity() {
            let Some((name, _)) = self.answers.first() else {
                break;
            };
            let name = *name;
            self.answers.remove(&name);
            self.abandoned = self.abandoned.saturating_add(1);
        }
    }
}

fn emit(run: Token, attempt: Token, turn: host::Turn, out: &mut Queue<Request>) {
    out.push(Request::Turn {
        run,
        attempt,
        turn: wire::Turn { turn: turn.turn, spent: turn.spent, read: turn.read, body: turn.body },
    });
}

fn copy_answer(answer: &wire::AnswerV2) -> wire::AnswerV2 {
    let ending = match &answer.ending {
        wire::EndingV2::Refused(refusal) => wire::EndingV2::Refused(*refusal),
        wire::EndingV2::Ended { outcome, work } => {
            wire::EndingV2::Ended { outcome: copy_of(outcome), work: copy_work(work) }
        }
        wire::EndingV2::Parked { work } => wire::EndingV2::Parked { work: copy_work(work) },
        wire::EndingV2::Failed { failure, detail, work } => {
            wire::EndingV2::Failed { failure: *failure, detail: copy_of(detail), work: copy_work(work) }
        }
    };
    wire::AnswerV2 { turns: answer.turns, spent: answer.spent, ending }
}

fn copy_work(work: &wire::Work) -> wire::Work {
    wire::Work { landed: work.landed.clone(), saved: work.saved.clone() }
}
