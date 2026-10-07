//! Walking-story obligations derived from the scripted person and worker,
//! checked against durable fake-store rows, never the root's private state
//! (domain/engine.md, section 15).

use jig_core_brief as brief;
use std::collections::{BTreeMap, BTreeSet};
use temper_engine_domain::engine::{BriefBody as Body, BriefKind as Kind};
use temper_engine_domain::{Key, Record, Write, engine::Assignment};
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;

/// The person's question, supplied independently of the root's brief route.
pub const QUESTION: &[u8] = b"say hello";

/// The worker's final report, supplied independently of task closing.
pub const REPORT: &[u8] = b"hello";

/// Cumulative prices and concrete transcripts selected by the worker script.
pub const TURNS: &[(u32, u64, &[u8])] = &[(1, 3, b"first"), (2, 8, b"second")];

/// Terminal price includes expense after the last transcript turn.
pub const FINAL_SPEND: u64 = 12;

/// An outside observer of the complete walking story (domain/engine.md, 15).
/// Prices, words and logical deliveries come from the script, not root facts.
#[derive(Clone, Debug, Default)]
pub struct WalkingReferee {
    person: Option<u64>,
    task: Option<u64>,
    sign_in_replies: u32,
    start_replies: u32,
    attempt: Option<u64>,
    saved_turns: BTreeSet<(u64, u64, u32)>,
    acknowledged_turns: BTreeSet<u32>,
    assignments: u32,
    answer_acknowledgements: u32,
    terminal_commits: u32,
    replayed_answer_acknowledgements: u32,
    results: u32,
}

fn task_record(rows: &BTreeMap<Key, Record>, number: u64) -> Option<&tasks::TaskRecord> {
    for key in [tasks::Key::Live(number), tasks::Key::Ended(number)] {
        if let Some(Record::Tasks(tasks::Stored::Live(record) | tasks::Stored::Ended(record))) =
            rows.get(&Key::Tasks(key))
        {
            return Some(record);
        }
    }
    None
}

impl WalkingReferee {
    /// Check unique rows in a decision and count transcript writes independently.
    ///
    /// # Errors
    /// Names duplicate keys, repeated transcript effects or unexpected turn data.
    pub fn commit(&mut self, writes: &[Write]) -> Result<(), &'static str> {
        let mut keys = BTreeSet::new();
        for write in writes {
            if !keys.insert(write.key()) {
                return Err("same key written twice in one decision");
            }
        }
        for write in writes {
            if let Write::Save(Record::Tasks(tasks::Stored::Live(record))) = write
                && matches!(record.phase, tasks::Phase::Active(tasks::Active::Claimed { .. }))
            {
                let claimed = writes.iter().any(|write| matches!(write,
                        Write::Save(Record::RunProof(proof)) if proof.task == record.number && proof.attempt == record.attempt && proof.turn.is_none() && proof.terminal.is_none()));
                if !claimed {
                    return Err("claim and reserved root proof are not one transaction");
                }
            }
            if let Write::Save(Record::Turn(turn)) = write {
                let expected = TURNS.iter().find(|(number, _, _)| *number == turn.turn).ok_or("unexpected turn")?;
                if turn.spent != expected.1 || turn.transcript.as_ref() != expected.2 || turn.read.is_some() {
                    return Err("turn differs from worker script");
                }
                let charged = writes.iter().any(|write| {
                    matches!(write,
                    Write::Save(Record::Tasks(tasks::Stored::Live(record))) if record.number == turn.task
                        && record.attempt == turn.attempt && record.turn == turn.turn
                        && record.run_spent == expected.1 && record.numbers.spent == expected.1)
                });
                let proven = writes.iter().any(|write| matches!(write,
                    Write::Save(Record::RunProof(proof)) if proof.task == turn.task && proof.attempt == turn.attempt
                        && proof.turn == Some(temper_engine_domain::TurnProof { turn: turn.turn, cumulative: turn.spent, read: turn.read })));
                if !charged {
                    return Err("transcript and accepted charge are not one transaction");
                }
                if !proven {
                    return Err("turn and root proof are not one transaction");
                }
                if !self.saved_turns.insert((turn.task, turn.attempt, turn.turn)) {
                    return Err("transcript committed twice");
                }
            }
            if let Write::Save(Record::Tasks(tasks::Stored::Ended(record))) = write {
                let posted = writes.iter().any(|write| {
                    matches!(write,
                    Write::Save(Record::Tasks(tasks::Stored::Ledger(ledger)))
                    if ledger.funder == record.funder && ledger.numbers.spent_below == FINAL_SPEND
                        && ledger.numbers.reserved == 0)
                });
                let retired = writes.contains(&Write::Erase(Key::RunProof { task: record.number }));
                let terminal = writes.iter().any(|write| matches!(write,
                    Write::Save(Record::Terminal(proof)) if proof.task == record.number && proof.attempt == record.attempt && proof.cumulative == FINAL_SPEND
                        && proof.end == (tasks::End::Finished { result: tasks::TaskResult::Report { words: REPORT.into() }, cancel_delegates: false })));
                if record.numbers.spent != FINAL_SPEND || !posted {
                    return Err("terminal and funding posting are not one transaction");
                }
                if !retired || !terminal {
                    return Err("ended task and root terminal proof retirement are not one transaction");
                }
                if self.terminal_commits != 0 {
                    return Err("terminal committed twice");
                }
                self.terminal_commits += 1;
            }
        }
        Ok(())
    }

    /// A sign-in reply must rest on the identity and session in the atomic store.
    ///
    /// # Errors
    /// Names a reply without its durable authenticated person/session records.
    pub fn signed_in(&mut self, rows: &BTreeMap<Key, Record>, person: u64, sign_in: u64) -> Result<(), &'static str> {
        let Some(Record::People(people::Stored::Person { number, identity })) =
            rows.get(&Key::People(people::Key::Person(person)))
        else {
            return Err("sign-in reply before durable identity");
        };
        if *number != person || identity.key != (people::IdentityKey { forge: 1, user: 7 }) {
            return Err("wrong authenticated person");
        }
        let Some(Record::People(people::Stored::SignIn { person: authenticated, .. })) =
            rows.get(&Key::People(people::Key::SignIn(sign_in)))
        else {
            return Err("sign-in reply before durable session");
        };
        if *authenticated != person {
            return Err("session belongs to another person");
        }
        if self.sign_in_replies != 0 {
            return Err("person received sign-in reply twice");
        }
        self.sign_in_replies += 1;
        self.person = Some(person);
        Ok(())
    }

    /// A keyed start reply is committed with its real task and exact question.
    ///
    /// # Errors
    /// Names missing durable keyed answers, wrong task words or requester.
    pub fn started(&mut self, rows: &BTreeMap<Key, Record>, task: u64) -> Result<(), &'static str> {
        let person = self.person.ok_or("chat before sign-in")?;
        let key = people::RequestKey { person, key: [5; 16] };
        let Some(Record::People(people::Stored::Answer { ask, outcome, .. })) =
            rows.get(&Key::People(people::Key::Answer(key)))
        else {
            return Err("chat reply before keyed answer commit");
        };
        if ask.as_ref() != &(people::Ask::StartChat { project: 1, words: QUESTION.into() })
            || *outcome != (people::Outcome::Started { task })
        {
            return Err("keyed answer differs from person request");
        }
        let record = task_record(rows, task).ok_or("chat reply before task commit")?;
        if record.requester != tasks::Party::Person(person) || record.spec.words.as_ref() != QUESTION {
            return Err("wrong task requester or question");
        }
        if self.start_replies != 0 {
            return Err("person received chat-start reply twice");
        }
        self.start_replies += 1;
        self.task = Some(task);
        Ok(())
    }

    /// The real brief and claim must be committed before a worker is assigned.
    ///
    /// # Errors
    /// Names an early/duplicate assignment, wrong claim, brief or account grant.
    pub fn assigned(&mut self, rows: &BTreeMap<Key, Record>, assignment: &Assignment) -> Result<(), &'static str> {
        if self.task != Some(assignment.task) {
            return Err("assignment names another person's task");
        }
        let record = task_record(rows, assignment.task).ok_or("assignment before durable task")?;
        if record.attempt != assignment.attempt || assignment.attempt == 0 {
            return Err("assignment before durable claim");
        }
        if !matches!(record.phase, tasks::Phase::Active(tasks::Active::Claimed { attempt } | tasks::Active::Running { attempt }) if attempt == assignment.attempt)
        {
            return Err("assignment without committed claim phase");
        }
        if assignment.charter != 1 || assignment.grant.account != 1 || assignment.grant.generation != 1 {
            return Err("assignment lost configured charter or real account grant");
        }
        let Some(section) = assignment.sections.first() else {
            return Err("missing required task section");
        };
        if section.kind != Kind::Core(brief::Core::Task) {
            return Err("task section is not first");
        }
        let Body::Text(bytes) = &section.body else {
            return Err("required task section was unread");
        };
        if !bytes.windows(QUESTION.len()).any(|part| part == QUESTION) {
            return Err("brief omitted the person's task words");
        }
        let contract = b"Report: at most 128 bytes";
        let requester = format!("Requested by person {}", self.person.ok_or("assignment before sign-in")?);
        if !bytes.windows(contract.len()).any(|part| part == contract)
            || !bytes.windows(requester.len()).any(|part| part == requester.as_bytes())
        {
            return Err("brief omitted the result contract or requester");
        }
        if self.assignments != 0 {
            return Err("one claim assigned twice across restart");
        }
        self.assignments += 1;
        self.attempt = Some(assignment.attempt);
        Ok(())
    }

    /// A turn ACK must follow its transcript and atomic charged task state.
    ///
    /// # Errors
    /// Names an early/out-of-order ACK, wrong fence or missing accepted charge.
    pub fn turn_ack(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        task: u64,
        attempt: u64,
        turn: u32,
    ) -> Result<(), &'static str> {
        if self.task != Some(task) || self.attempt != Some(attempt) {
            return Err("turn ACK has wrong claim fence");
        }
        let expected = TURNS.iter().find(|(number, _, _)| *number == turn).ok_or("unexpected turn ACK")?;
        let Some(Record::Turn(record)) = rows.get(&Key::Turn { task, attempt, turn }) else {
            return Err("turn ACK before durable transcript");
        };
        if record.spent != expected.1 || record.transcript.as_ref() != expected.2 {
            return Err("durable transcript differs from worker script");
        }
        let record = task_record(rows, task).ok_or("turn ACK without durable task")?;
        if record.turn < turn || record.run_spent < expected.1 || record.numbers.spent < expected.1 {
            return Err("turn ACK before atomic charge");
        }
        if turn > 1 && !self.acknowledged_turns.contains(&(turn - 1)) {
            return Err("turns acknowledged out of order");
        }
        self.acknowledged_turns.insert(turn);
        Ok(())
    }

    /// Worker answer ACK follows its priced terminal in the durable task row.
    ///
    /// # Errors
    /// Names an early/duplicate answer ACK or incorrect final accounting.
    pub fn answer_ack(&mut self, rows: &BTreeMap<Key, Record>, task: u64, attempt: u64) -> Result<(), &'static str> {
        if self.task != Some(task) || self.attempt != Some(attempt) {
            return Err("answer ACK has wrong claim fence");
        }
        let record = task_record(rows, task).ok_or("answer ACK before durable terminal")?;
        if record.last_answer != Some(attempt) || record.numbers.spent != FINAL_SPEND {
            return Err("answer ACK before exact terminal charge");
        }
        if self.acknowledged_turns.len() != TURNS.len() {
            return Err("answer ACK overtook turns");
        }
        if self.answer_acknowledgements != 0 {
            return Err("answer acknowledged twice in one scripted delivery");
        }
        self.answer_acknowledgements += 1;
        Ok(())
    }

    /// Person's result is the exact report in the committed ended record.
    ///
    /// # Errors
    /// Names a lost/duplicate/early result or a result sent to another person.
    pub fn result(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        person: u64,
        task: u64,
        words: &[u8],
    ) -> Result<(), &'static str> {
        if self.person != Some(person) || self.task != Some(task) || words != REPORT {
            return Err("wrong person result");
        }
        let Some(Record::Tasks(tasks::Stored::Ended(record))) = rows.get(&Key::Tasks(tasks::Key::Ended(task))) else {
            return Err("result before durable ended record");
        };
        let tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Report { words: report })) = &record.phase
        else {
            return Err("result is not the committed report");
        };
        if report.as_ref() != REPORT || record.numbers.spent != FINAL_SPEND {
            return Err("report or final expense changed");
        }
        if record.run_spent != FINAL_SPEND || record.numbers.spent_below != 0 || record.numbers.reserved != 0 {
            return Err("ended task retained an expense or reservation");
        }
        let pool = tasks::Funder::Pool { project: 1, person, period: 1 };
        let period = tasks::Funder::Period { project: 1, period: 1 };
        let Some(Record::Tasks(tasks::Stored::Ledger(pool_row))) = rows.get(&Key::Tasks(tasks::Key::Ledger(pool)))
        else {
            return Err("result without durable person funding");
        };
        let Some(Record::Tasks(tasks::Stored::Ledger(period_row))) = rows.get(&Key::Tasks(tasks::Key::Ledger(period)))
        else {
            return Err("result without durable project funding");
        };
        if pool_row.funder != pool
            || pool_row.parent != Some(period)
            || pool_row.closed
            || pool_row.numbers != (tasks::Numbers { budget: 500, spent: 0, spent_below: FINAL_SPEND, reserved: 0 })
            || period_row.funder != period
            || period_row.parent.is_some()
            || period_row.closed
            || period_row.numbers != (tasks::Numbers { budget: 1000, spent: 0, spent_below: 0, reserved: 500 })
        {
            return Err("final charge was lost, duplicated or posted to another source");
        }
        if self.results != 0 {
            return Err("person received result twice");
        }
        self.results += 1;
        Ok(())
    }

    /// The fake store checks the one applied terminal transaction before losing
    /// its completion. No worker ACK or person result has escaped the old root;
    /// the ended row, exact typed evidence and posted funding are durable, and
    /// the live proof is erased.
    ///
    /// # Errors
    /// Names a premature/late cut, missing terminal evidence or changed expense.
    pub fn terminal_cut(&self, rows: &BTreeMap<Key, Record>) -> Result<(), &'static str> {
        if self.terminal_commits != 1 || self.answer_acknowledgements != 0 || self.results != 0 {
            return Err("cut is not after one terminal commit before ACK/result release");
        }
        self.terminal_evidence(rows)?;
        let mut probe = self.clone();
        probe.answer_ack(
            rows,
            self.task.ok_or("terminal without task")?,
            self.attempt.ok_or("terminal without attempt")?,
        )?;
        probe.result(
            rows,
            self.person.ok_or("terminal without person")?,
            self.task.ok_or("terminal without task")?,
            REPORT,
        )
    }

    fn terminal_evidence(&self, rows: &BTreeMap<Key, Record>) -> Result<(), &'static str> {
        let task = self.task.ok_or("terminal without task")?;
        let attempt = self.attempt.ok_or("terminal without attempt")?;
        let Some(Record::Terminal(terminal)) = rows.get(&Key::Terminal { task, attempt }) else {
            return Err("durable terminal evidence missing");
        };
        if terminal.task != task
            || terminal.attempt != attempt
            || terminal.cumulative != FINAL_SPEND
            || terminal.end
                != (tasks::End::Finished {
                    result: tasks::TaskResult::Report { words: REPORT.into() },
                    cancel_delegates: false,
                })
        {
            return Err("durable terminal evidence differs from worker offer");
        }
        let _record = task_record(rows, task).ok_or("terminal without ended task")?;
        if rows.contains_key(&Key::RunProof { task }) || rows.contains_key(&Key::Tasks(tasks::Key::Live(task))) {
            return Err("ended task retained a live task or proof");
        }
        Ok(())
    }

    /// The worker resends once after its recovered ACK. This separate transport
    /// ACK is permitted only for the same ended fence and unchanged terminal;
    /// it adds no task commit, charge or person result.
    ///
    /// # Errors
    /// Names an unsolicited/repeated replay ACK or changed durable evidence.
    pub fn replayed_answer_ack(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        task: u64,
        attempt: u64,
    ) -> Result<(), &'static str> {
        if self.answer_acknowledgements != 1 || self.replayed_answer_acknowledgements != 0 {
            return Err("replay ACK is not the one explicit post-ACK resend");
        }
        self.terminal_evidence(rows)?;
        let mut probe = self.clone();
        probe.answer_acknowledgements = 0;
        probe.answer_ack(rows, task, attempt)?;
        self.replayed_answer_acknowledgements += 1;
        Ok(())
    }

    /// The outside worker has consumed the ACK for its one explicit post-ACK
    /// terminal resend; this does not count a second logical result or terminal
    #[must_use]
    pub fn terminal_replay_done(&self) -> bool {
        self.replayed_answer_acknowledgements == 1
    }

    /// The story is complete only after all independently expected effects.
    #[must_use]
    pub fn done(&self) -> bool {
        self.sign_in_replies == 1
            && self.start_replies == 1
            && self.assignments == 1
            && self.saved_turns.len() == TURNS.len()
            && self.acknowledged_turns.len() == TURNS.len()
            && self.terminal_commits == 1
            && self.answer_acknowledgements == 1
            && self.results == 1
    }
}
