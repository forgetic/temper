//! Version-two reliable output. Turns own their bytes until commitment ACK;
//! only emission copies them (programming-model.md, 6.2; domain/worker.md, 8).

use crate::boundary::{Hosted, Phase, Request};
use crate::limits::Limits;
use skein_lib::bytes::copy_of;
use skein_lib::{Deadlines, Env, Map, Queue, Time, Token};
use temper_worker_domain_host as host;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct Name {
    pub(crate) run: Token,
    pub(crate) attempt: Token,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct TurnName {
    run: Token,
    attempt: Token,
    turn: u32,
}
#[derive(Debug)]
pub(crate) struct Pending {
    agent: Token,
    turn: host::Turn,
}
#[derive(Debug)]
pub(crate) struct Answer {
    answer: host::AnswerV2,
    acknowledged: bool,
}
#[derive(Debug)]
pub(crate) struct Turns {
    pending: Map<TurnName, Pending>,
    answers: Map<Name, Answer>,
    retry: Deadlines<TurnName>,
    abandoned: u64,
}

impl Turns {
    pub(crate) fn new(limits: &Limits) -> Turns {
        let capacity = limits.host.slots.checked_mul(limits.turns).expect("worst_case accepted turn capacity");
        Turns {
            pending: Map::with_capacity(capacity),
            answers: Map::with_capacity(limits.host.slots),
            retry: Deadlines::with_capacity(capacity),
            abandoned: 0,
        }
    }
    pub(crate) fn held(&self) -> u32 {
        self.answers.len()
    }
    pub(crate) fn pending(&self) -> u32 {
        self.pending.len()
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
    /// A credit reserves room for a maximum turn, not merely today's body.
    pub(crate) fn credit(&self, run: Token, attempt: Token, limits: &Limits) -> bool {
        let mut count = 0_u32;
        let mut bytes = 0_u64;
        for (name, pending) in &self.pending {
            if name.run == run && name.attempt == attempt {
                count = count.checked_add(1).expect("bounded turns");
                bytes = bytes.checked_add(len(&pending.turn.body)).expect("bounded retained bytes");
            }
        }
        count < limits.turns
            && match bytes.checked_add(limits.host.turn_bytes) {
                Some(next) => next <= limits.turn_queue_bytes,
                None => false,
            }
    }
    #[expect(clippy::too_many_arguments, reason = "ownership, fencing and admission are separate inputs")]
    pub(crate) fn retain(
        &mut self,
        agent: Token,
        run: Token,
        attempt: Token,
        turn: host::Turn,
        limits: &Limits,
        up: bool,
        out: &mut Queue<Request>,
    ) -> bool {
        assert!(self.credit(run, attempt, limits), "an agent turn consumes credit granted before its read");
        let name = TurnName { run, attempt, turn: turn.turn };
        if up {
            emit(name, &turn, out);
        }
        let old = self.pending.insert(name, Pending { agent, turn }).expect("turn credit checked capacity");
        assert!(old.is_none(), "the host validates consecutive turn numbers");
        self.credit(run, attempt, limits)
    }
    pub(crate) fn acknowledge(&mut self, run: Token, attempt: Token, turn: u32) -> Option<Token> {
        let name = TurnName { run, attempt, turn };
        self.retry.cancel(name);
        let pending = self.pending.remove(&name)?;
        self.release_answer(run, attempt);
        Some(pending.agent)
    }
    pub(crate) fn busy(&mut self, run: Token, attempt: Token, turn: u32, env: &Env<Limits>) {
        let name = TurnName { run, attempt, turn };
        if self.pending.contains_key(&name) {
            self.retry.arm(name, env.now.saturating_add(env.limits.turn_backoff)).expect("one retry per retained turn");
        }
    }
    pub(crate) fn fire(&mut self, now: Time, up: bool, out: &mut Queue<Request>) {
        let Some(name) = self.retry.expire(now) else {
            return;
        };
        if up && let Some(pending) = self.pending.get(&name) {
            emit(name, &pending.turn, out);
        }
    }
    pub(crate) fn answer(
        &mut self,
        run: Token,
        attempt: Token,
        answer: host::AnswerV2,
        up: bool,
        out: &mut Queue<Request>,
    ) {
        let refused = match answer.ending {
            host::EndingV2::Refused(_) => true,
            host::EndingV2::Ended { .. } | host::EndingV2::Parked { .. } | host::EndingV2::Failed { .. } => false,
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
    pub(crate) fn answer_acknowledged(&mut self, run: Token, attempt: Token) {
        if let Some(answer) = self.answers.get_mut(&Name { run, attempt }) {
            answer.acknowledged = true;
        }
        self.release_answer(run, attempt);
    }
    fn release_answer(&mut self, run: Token, attempt: Token) {
        for (name, _) in &self.pending {
            if name.run == run && name.attempt == attempt {
                return;
            }
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
    pub(crate) fn hello(&mut self, out: &mut Queue<Request>) {
        for (name, pending) in &self.pending {
            self.retry.cancel(*name);
            emit(*name, &pending.turn, out);
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
    pub(crate) fn give_up(&mut self) {
        for _ in 0..self.pending.capacity() {
            let Some((name, _)) = self.pending.first() else {
                break;
            };
            let name = *name;
            self.retry.cancel(name);
            self.pending.remove(&name);
            self.abandoned = self.abandoned.saturating_add(1);
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
fn emit(name: TurnName, turn: &host::Turn, out: &mut Queue<Request>) {
    out.push(Request::Turn {
        run: name.run,
        attempt: name.attempt,
        turn: host::Turn { turn: turn.turn, spent: turn.spent, read: turn.read, body: copy_of(&turn.body) },
    });
}
fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("a length fits in u64")
}
fn copy_answer(answer: &host::AnswerV2) -> host::AnswerV2 {
    let ending = match &answer.ending {
        host::EndingV2::Refused(refusal) => host::EndingV2::Refused(*refusal),
        host::EndingV2::Ended { outcome, work } => {
            host::EndingV2::Ended { outcome: copy_of(outcome), work: copy_work(work) }
        }
        host::EndingV2::Parked { work } => host::EndingV2::Parked { work: copy_work(work) },
        host::EndingV2::Failed { failure, detail, work } => {
            host::EndingV2::Failed { failure: *failure, detail: copy_of(detail), work: copy_work(work) }
        }
    };
    host::AnswerV2 { turns: answer.turns, spent: answer.spent, ending }
}
fn copy_work(work: &host::Work) -> host::Work {
    host::Work { landed: work.landed.clone(), saved: work.saved.clone() }
}
