//! Independent escalation obligations from the two people and worker scripts.
//! Reads only submitted boundaries and atomic store evidence, never root/child
//! private state (domain/engine.md, section 7.7; domain/tasks.md, section 15).

use skein_lib::Token;
use std::collections::{BTreeMap, BTreeSet};
use temper_engine_domain::{EscalationDecisionRecord, Key, Record, TerminalRecord, Write, engine::Assignment};
use temper_engine_domain_brief::{Body, Kind};
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;

/// Independently supplied opening words (domain/engine.md, section 7.7).
pub const QUESTION: &[u8] = b"retry held chat";

/// Independently supplied successful second-attempt result (domain/tasks.md, section 15).
pub const REPORT: &[u8] = b"released report";

/// Bounded rejection script, retained while held (domain/tasks.md, section 15).
pub const REASON: &[u8] = b"keep held";

/// Finite outside choices, without live role administration or delegates
/// (domain/engine.md, section 7.7; domain/tasks.md, section 15).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Story {
    /// Requester releases revision one (domain/tasks.md, section 15).
    Release,

    /// Requester passes; second owner releases final-role revision two (domain/tasks.md, section 15).
    PassRelease,

    /// Requester rejects and leaves the chat held (domain/tasks.md, section 15).
    Reject,

    /// Final-role release wins over the other owner's rejection (domain/engine.md, section 7.7).
    RaceRelease,

    /// Final-role rejection wins over the other owner's release (domain/engine.md, section 7.7).
    RaceReject,
}

impl Story {
    /// Whether the outside script ends with a report or a durable rejection
    /// (domain/tasks.md, section 15).
    #[must_use]
    pub const fn succeeds(self) -> bool {
        matches!(self, Story::Release | Story::PassRelease | Story::RaceRelease)
    }

    /// Whether revision one moves to the final policy role (domain/tasks.md, section 15).
    #[must_use]
    pub const fn passes(self) -> bool {
        matches!(self, Story::PassRelease | Story::RaceRelease | Story::RaceReject)
    }
}

#[derive(Clone, Debug)]
struct AskReply {
    person: u64,
    key: [u8; 16],
    ask: people::Ask,
    terminal: ExpectedTerminal,
}

#[derive(Clone, Debug)]
enum ExpectedTerminal {
    Outcome(people::Outcome),
    KeyConflict { saved_ask: people::Ask, saved_outcome: people::Outcome },
}

/// Outside observer of exactly the scripted people, decisions and worker costs.
/// It owns obligations, not a second live task/authority model
/// (domain/engine.md, section 7.7; domain/tasks.md, section 15).
#[derive(Clone, Debug)]
pub struct Referee {
    story: Story,
    people: [Option<u64>; 2],
    task: Option<u64>,
    assignments: Vec<u64>,
    claims: BTreeSet<u64>,
    unplaced_claims: BTreeSet<u64>,
    offers: BTreeMap<u64, (u64, people::EscalationDecision)>,
    archives: BTreeMap<u64, EscalationDecisionRecord>,
    terminals: BTreeSet<u64>,
    acknowledgements: BTreeSet<u64>,
    asks: BTreeMap<Token, AskReply>,
    reads: BTreeMap<Token, (u64, tasks::Escalation)>,
    read_terminals: BTreeSet<Token>,
    busy_reads: BTreeSet<Token>,
    start_replies: u32,
    results: u32,
}

fn task(rows: &BTreeMap<Key, Record>, number: u64) -> Option<&tasks::TaskRecord> {
    for key in [tasks::Key::Live(number), tasks::Key::Ended(number)] {
        if let Some(Record::Tasks(tasks::Stored::Live(record) | tasks::Stored::Ended(record))) =
            rows.get(&Key::Tasks(key))
        {
            return Some(record.as_ref());
        }
    }
    None
}

fn saved_task(writes: &[Write], number: u64) -> Option<&tasks::TaskRecord> {
    for write in writes {
        if let Write::Save(Record::Tasks(tasks::Stored::Live(record) | tasks::Stored::Ended(record))) = write
            && record.number == number
        {
            return Some(record.as_ref());
        }
    }
    None
}

fn held(record: &tasks::TaskRecord) -> bool {
    record.phase
        == (tasks::Phase::Held {
            why: tasks::Hold::Failures(tasks::Class::Run),
            was: tasks::Was::Active(tasks::Active::Due),
        })
}

fn choice(decision: &people::EscalationDecision) -> people::EscalationChoice {
    match decision {
        people::EscalationDecision::Release => people::EscalationChoice::Released,
        people::EscalationDecision::Reject { .. } => people::EscalationChoice::Rejected,
        people::EscalationDecision::Pass => people::EscalationChoice::Passed,
    }
}

impl Referee {
    /// No identities or obligations exist until boundary stimuli/terminals occur
    /// (domain/engine.md, section 7.7).
    #[must_use]
    pub fn new(story: Story) -> Referee {
        Referee {
            story,
            people: [None, None],
            task: None,
            assignments: Vec::new(),
            claims: BTreeSet::new(),
            unplaced_claims: BTreeSet::new(),
            offers: BTreeMap::new(),
            archives: BTreeMap::new(),
            terminals: BTreeSet::new(),
            acknowledgements: BTreeSet::new(),
            asks: BTreeMap::new(),
            reads: BTreeMap::new(),
            read_terminals: BTreeSet::new(),
            busy_reads: BTreeSet::new(),
            start_replies: 0,
            results: 0,
        }
    }

    /// Register the exact keyed ask and independently expected terminal before
    /// dispatch, including stale winners and refusals (domain/people.md, section 5.1.2).
    pub fn ask(&mut self, to: Token, person: u64, key: [u8; 16], ask: people::Ask, outcome: people::Outcome) {
        assert!(
            self.asks.insert(to, AskReply { person, key, ask, terminal: ExpectedTerminal::Outcome(outcome) }).is_none(),
            "one fresh reply right per script ask"
        );
    }

    /// Register an immediate key-conflict refusal and freeze the existing saved
    /// request/outcome; no new answered key is expected (domain/people.md, section 5.1.1).
    pub fn key_conflict(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        to: Token,
        person: u64,
        key: [u8; 16],
        ask: people::Ask,
    ) {
        let Some(Record::People(people::Stored::Answer { ask: saved_ask, outcome, .. })) =
            rows.get(&Key::People(people::Key::Answer(people::RequestKey { person, key })))
        else {
            panic!("key-conflict probe follows its durable winning answer");
        };
        assert_ne!(*saved_ask, ask, "conflicting probe changes the request under the original key");
        let terminal = ExpectedTerminal::KeyConflict { saved_ask: saved_ask.clone(), saved_outcome: *outcome };
        assert!(self.asks.insert(to, AskReply { person, key, ask, terminal }).is_none(), "fresh conflict reply right");
    }

    /// Register one expected current held view before a named authenticated read
    /// (domain/engine.md, section 7.7).
    pub fn read(&mut self, to: Token, person: u64, escalation: tasks::Escalation) {
        assert!(!self.read_terminals.contains(&to), "a retry requires a fresh named read right");
        assert!(self.reads.insert(to, (person, escalation)).is_none(), "one fresh reply right per named read");
    }

    /// Register the first scripted semantic winner for one revision; race losers
    /// never become a second expected mutation (domain/engine.md, section 7.7).
    pub fn offer(&mut self, revision: u64, by: u64, decision: people::EscalationDecision) {
        assert!(self.offers.insert(revision, (by, decision)).is_none(), "one scripted winner per semantic revision");
    }

    /// Check one sign-in's durable identity and session before retaining its name.
    ///
    /// # Errors
    /// Rejects missing/wrong identity or session, and duplicate terminals
    /// (domain/people.md, sections 3 and 5).
    pub fn signed_in(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        index: usize,
        person: u64,
        sign_in: u64,
    ) -> Result<(), &'static str> {
        if index >= 2 || self.people[index].is_some() {
            return Err("duplicate or unscripted sign-in");
        }
        let expected = people::IdentityKey { forge: 1, user: 7 + u64::try_from(index).expect("two identities") };
        if !matches!(rows.get(&Key::People(people::Key::Person(person))), Some(Record::People(people::Stored::Person { number, identity })) if *number == person && identity.key == expected)
        {
            return Err("sign-in before exact durable identity");
        }
        if !matches!(rows.get(&Key::People(people::Key::SignIn(sign_in))), Some(Record::People(people::Stored::SignIn { person: owner, .. })) if *owner == person)
        {
            return Err("sign-in before exact durable session");
        }
        self.people[index] = Some(person);
        Ok(())
    }

    /// Check the sole chat task and atomic keyed creation reply.
    ///
    /// # Errors
    /// Rejects duplicate starts or mismatched requester/question/answer
    /// (domain/engine.md, section 7.1).
    pub fn started(&mut self, rows: &BTreeMap<Key, Record>, number: u64) -> Result<(), &'static str> {
        if self.start_replies != 0 {
            return Err("chat-start terminal twice");
        }
        let person = self.people[0].ok_or("start before requester sign-in")?;
        let record = task(rows, number).ok_or("start before durable task")?;
        if record.requester != tasks::Party::Person(person) || record.spec.words.as_ref() != QUESTION {
            return Err("start differs from requester script");
        }
        if !matches!(rows.get(&Key::People(people::Key::Answer(people::RequestKey { person, key: [5; 16] }))), Some(Record::People(people::Stored::Answer { ask, outcome, .. })) if *ask == (people::Ask::StartChat { project: 1, words: QUESTION.into() }) && *outcome == (people::Outcome::Started { task: number }))
        {
            return Err("start without its atomic keyed answer");
        }
        self.task = Some(number);
        self.start_replies += 1;
        Ok(())
    }

    /// Check real committed claims/briefs, with fresh attempt counters and no
    /// assignment while held.
    ///
    /// # Errors
    /// Rejects early, duplicate, wrong-task or stale-price assignments
    /// (domain/engine.md, sections 7.1 and 7.7).
    pub fn assigned(&mut self, rows: &BTreeMap<Key, Record>, assignment: &Assignment) -> Result<(), &'static str> {
        if self.task != Some(assignment.task) {
            return Err("assignment for another task");
        }
        let count = self.assignments.len();
        if count >= 2 || (count == 1 && !self.story.succeeds()) || self.assignments.contains(&assignment.attempt) {
            return Err("unexpected or duplicate assignment");
        }
        let record = task(rows, assignment.task).ok_or("assignment before committed task")?;
        if record.attempt != assignment.attempt
            || !matches!(record.phase, tasks::Phase::Active(tasks::Active::Claimed { attempt } | tasks::Active::Running { attempt }) if attempt == assignment.attempt)
        {
            return Err("assignment before exact durable claim");
        }
        let expected_tries = tasks::Tries {
            lost: u32::try_from(self.unplaced_claims.len()).expect("bounded claims"),
            ..tasks::Tries::NONE
        };
        if record.numbers.spent != if count == 0 { 0 } else { 3 }
            || record.run_spent != 0
            || record.turn != 0
            || record.tries != expected_tries
        {
            return Err("fresh attempt reused expense or failure counters");
        }
        if !matches!(rows.get(&Key::RunProof { task: assignment.task }), Some(Record::RunProof(proof)) if proof.attempt == assignment.attempt && proof.turn.is_none() && proof.terminal.is_none())
        {
            return Err("assignment without fresh root proof");
        }
        if assignment.charter != 1 || assignment.grant.account != 1 || assignment.grant.generation != 1 {
            return Err("assignment without configured grant");
        }
        let section = assignment.sections.first().ok_or("missing real task brief")?;
        let Body::Text(bytes) = &section.body else {
            return Err("brief task section unread");
        };
        if section.kind != Kind::Task
            || !bytes.windows(QUESTION.len()).any(|part| part == QUESTION)
            || !bytes.windows(b"Report: at most 128 bytes".len()).any(|part| part == b"Report: at most 128 bytes")
        {
            return Err("brief differs from actual chat contract");
        }
        if self.assignments.last().is_some_and(|old| assignment.attempt <= *old) {
            return Err("attempt identity did not advance");
        }
        self.assignments.push(assignment.attempt);
        Ok(())
    }

    /// Check atomic cohorts, prices and one immutable archive per offered winner.
    ///
    /// # Errors
    /// Rejects split hold/decision/terminal transactions, duplicate keys/effects,
    /// bad prices or unscripted decision evidence (domain/engine.md, section 7.7).
    #[expect(clippy::too_many_lines, reason = "one outside pass checks each independent atomic evidence family")]
    pub fn commit(&mut self, writes: &[Write]) -> Result<(), &'static str> {
        let mut keys = BTreeSet::new();
        for write in writes {
            if !keys.insert(write.key()) {
                return Err("same key twice in escalation transaction");
            }
        }
        for write in writes {
            if let Write::Save(Record::People(people::Stored::Answer { outcome: people::Outcome::EscalationDecided { task, revision, .. }, .. })) = write
                && !self.archives.contains_key(revision)
                && !writes.iter().any(|write| matches!(write, Write::Save(Record::EscalationDecision(archive)) if archive.task == *task && archive.revision == *revision)) {
                return Err("keyed decision outcome without same-transaction archive");
            }
            if let Write::Save(Record::Turn(_)) = write {
                return Err("unscripted transcript in terminal-only worker story");
            }
            if let Write::Save(Record::Tasks(tasks::Stored::Live(record))) = write {
                let decided_revision = match &record.escalation {
                    tasks::Escalation::Unheld { revision } | tasks::Escalation::Rejected { revision, .. }
                        if *revision != 0 =>
                    {
                        Some(*revision)
                    }
                    tasks::Escalation::Waiting { revision, holder: tasks::EscalationHolder::Role { .. } }
                        if *revision == 2 =>
                    {
                        Some(1)
                    }
                    tasks::Escalation::Unheld { .. }
                    | tasks::Escalation::Routing { .. }
                    | tasks::Escalation::Waiting { .. }
                    | tasks::Escalation::Rejected { .. } => None,
                };
                if let Some(revision) = decided_revision
                    && self.offers.contains_key(&revision) && !self.archives.contains_key(&revision)
                    && !writes.iter().any(|write| matches!(write, Write::Save(Record::EscalationDecision(archive)) if archive.task == record.number && archive.revision == revision)) {
                    return Err("semantic decision without same-transaction archive");
                }
                if let tasks::Phase::Active(tasks::Active::Claimed { attempt }) = record.phase {
                    if !writes.iter().any(|write| matches!(write, Write::Save(Record::RunProof(proof)) if proof.task == record.number && proof.attempt == attempt && proof.turn.is_none() && proof.terminal.is_none())) {
                        return Err("claimed task without same-transaction fresh proof");
                    }
                    self.claims.insert(attempt);
                }
            }
            if let Write::Save(Record::Terminal(terminal)) = write {
                let Some(index) = self.assignments.iter().position(|attempt| *attempt == terminal.attempt) else {
                    self.unplaced_terminal(writes, terminal)?;
                    continue;
                };
                if Some(terminal.task) != self.task {
                    return Err("terminal names another task");
                }
                let record =
                    saved_task(writes, terminal.task).ok_or("terminal without same-transaction task charge")?;
                if index == 0 {
                    if terminal.cumulative != 3
                        || terminal.end != tasks::End::Failed(tasks::Class::Run)
                        || record.numbers.spent != 3
                        || record.run_spent != 3
                        || !held(record)
                        || record.escalation
                            != (tasks::Escalation::Waiting {
                                revision: 1,
                                holder: tasks::EscalationHolder::Person(self.people[0].ok_or("hold before identity")?),
                            })
                    {
                        return Err("failure charge and routed hold are not atomic");
                    }
                    if record.tries.run != 1 {
                        return Err("retry-zero failure history differs from script");
                    }
                    if !writes.iter().any(|write| matches!(write, Write::Save(Record::RunProof(proof)) if proof.task == terminal.task && proof.attempt == terminal.attempt && proof.terminal.as_ref() == Some(terminal))) { return Err("held failure and root proof are not atomic"); }
                } else {
                    if terminal.cumulative != 2
                        || terminal.end
                            != (tasks::End::Finished {
                                result: tasks::TaskResult::Report { words: REPORT.into() },
                                cancel_delegates: false,
                            })
                        || record.numbers.spent != 5
                        || record.run_spent != 2
                        || record.phase
                            != tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Report {
                                words: REPORT.into(),
                            }))
                    {
                        return Err("final attempt was not charged once on lifetime expense");
                    }
                    if !writes.iter().any(|write| matches!(write, Write::Save(Record::Tasks(tasks::Stored::Ledger(pool))) if pool.funder == record.funder && pool.numbers.spent_below == 5 && pool.numbers.reserved == 0)) { return Err("final expense and funding posting are not atomic"); }
                    if !writes.iter().any(|write| matches!(write, Write::Save(Record::Tasks(tasks::Stored::Closure(closure))) if closure.task == record.number && closure.funder == record.funder && closure.generation == record.allotment && closure.spent == 5 && closure.budget == record.numbers.budget)) || !writes.contains(&Write::Erase(Key::RunProof { task: record.number })) { return Err("final closure and proof retirement are not atomic"); }
                }
                if !self.terminals.insert(terminal.attempt) {
                    return Err("worker terminal committed twice");
                }
            }
            if let Write::Save(Record::EscalationDecision(archive)) = write {
                let (by, decision) = self.offers.get(&archive.revision).ok_or("unscripted archive decision")?;
                if Some(archive.task) != self.task
                    || archive.project != 1
                    || Some(archive.requester) != self.people[0]
                    || archive.by != *by
                    || archive.decision != *decision
                {
                    return Err("archive differs from scripted winner");
                }
                let record = saved_task(writes, archive.task).ok_or("decision archive without semantic task change")?;
                let semantic = match decision {
                    people::EscalationDecision::Release => {
                        record.escalation == (tasks::Escalation::Unheld { revision: archive.revision })
                            && !held(record)
                            && record.tries == tasks::Tries::NONE
                    }
                    people::EscalationDecision::Reject { reason } => {
                        held(record)
                            && record.escalation
                                == (tasks::Escalation::Rejected {
                                    revision: archive.revision,
                                    by: *by,
                                    reason: reason.clone(),
                                })
                    }
                    people::EscalationDecision::Pass => {
                        held(record)
                            && record.escalation
                                == (tasks::Escalation::Waiting {
                                    revision: archive.revision + 1,
                                    holder: tasks::EscalationHolder::Role { project: 1, role: 0 },
                                })
                    }
                };
                if !semantic || record.numbers.spent != 3 {
                    return Err("decision semantic change differs from archived choice");
                }
                let atomic = writes.iter().any(|write| matches!(write, Write::Save(Record::People(people::Stored::Answer { key, ask: people::Ask::DecideEscalation { project, task, revision, decision: asked }, outcome, .. })) if key.person == *by && *project == 1 && *task == archive.task && *revision == archive.revision && *asked == *decision && *outcome == (people::Outcome::EscalationDecided { task: archive.task, revision: archive.revision, by: *by, choice: choice(decision) })));
                if !atomic {
                    return Err("archive semantic change and keyed outcome are not one transaction");
                }
                if self.archives.insert(archive.revision, archive.clone()).is_some() {
                    return Err("semantic revision archived twice");
                }
            }
        }
        Ok(())
    }

    fn unplaced_terminal(&mut self, writes: &[Write], terminal: &TerminalRecord) -> Result<(), &'static str> {
        if Some(terminal.task) != self.task
            || !self.claims.contains(&terminal.attempt)
            || terminal.cumulative != 0
            || terminal.end != tasks::End::Failed(tasks::Class::Lost)
        {
            return Err("unassigned claim did not retire as an unpriced loss");
        }
        let record = saved_task(writes, terminal.task).ok_or("unassigned loss without atomic task state")?;
        if record.attempt != terminal.attempt
            || record.numbers.spent != 3
            || record.run_spent != 0
            || record.turn != 0
            || record.tries != (tasks::Tries { lost: 1, ..tasks::Tries::NONE })
            || record.last_answer != Some(terminal.attempt)
            || !matches!(record.phase, tasks::Phase::Active(tasks::Active::BackingOff { .. }))
        {
            return Err("unassigned loss did not spend one try or changed accepted expense");
        }
        if !writes.iter().any(|write| matches!(write, Write::Save(Record::RunProof(proof)) if proof.task == terminal.task && proof.attempt == terminal.attempt && proof.turn.is_none() && proof.terminal.as_ref() == Some(terminal))) {
            return Err("unassigned loss and canonical proof are not atomic");
        }
        if writes.iter().any(|write| {
            matches!(write, Write::Save(Record::Tasks(tasks::Stored::Ledger(_) | tasks::Stored::Closure(_))))
        }) {
            return Err("unassigned loss changed the authentic funding ledger");
        }
        if !self.unplaced_claims.insert(terminal.attempt) {
            return Err("unassigned claim retired twice");
        }
        Ok(())
    }

    /// Count durable claims whose assignment never escaped and whose canonical
    /// loss preserved expense and spent one try (domain/engine.md, section 7.4;
    /// domain/tasks.md, section 5.2).
    #[must_use]
    pub fn unplaced_claims(&self) -> usize {
        self.unplaced_claims.len()
    }

    /// Check outcome replies against their durable answers and immediate key-conflict
    /// refusals against the unchanged original answer; historical losers retain the winner.
    ///
    /// # Errors
    /// Rejects unsolicited/duplicate replies, changed winner or missing durability
    /// (domain/engine.md, section 7.7; domain/people.md, section 5.1.2).
    pub fn replied(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        to: Token,
        reply: people::Reply,
    ) -> Result<(), &'static str> {
        let expected = self.asks.get(&to).ok_or("unsolicited or duplicate keyed terminal")?;
        let Some(Record::People(people::Stored::Answer { ask, outcome: saved, .. })) = rows
            .get(&Key::People(people::Key::Answer(people::RequestKey { person: expected.person, key: expected.key })))
        else {
            return Err("keyed terminal before durable answer");
        };
        match &expected.terminal {
            ExpectedTerminal::KeyConflict { saved_ask, saved_outcome } => {
                if reply != people::Reply::Refused(people::Refusal::KeyConflict) {
                    return Err("key conflict requires the immediate refusal wrapper");
                }
                if *ask == expected.ask || ask != saved_ask || saved != saved_outcome {
                    return Err("key conflict changed the original durable winner");
                }
            }
            ExpectedTerminal::Outcome(outcome) => {
                if reply != people::Reply::Outcome(*outcome) {
                    return Err("keyed terminal differs from independently expected outcome wrapper");
                }
                if *ask != expected.ask || saved != outcome {
                    return Err("keyed terminal and durable answer differ");
                }
                if let people::Outcome::EscalationDecided { task, revision, by, choice: answer } = *outcome {
                    let Some(Record::EscalationDecision(archive)) =
                        rows.get(&Key::EscalationDecision { task, revision })
                    else {
                        return Err("decision reply before durable archive");
                    };
                    if archive.by != by || choice(&archive.decision) != answer {
                        return Err("reply changed archived race winner");
                    }
                }
            }
        }
        self.asks.remove(&to);
        Ok(())
    }

    /// Consume an immediate `Busy` terminal for an outstanding current-view read.
    /// The person must retry using a fresh right; no keyed outcome is saved
    /// (domain/engine.md, section 7.7; domain/people.md, section 5.1.2).
    ///
    /// # Errors
    /// Rejects unsolicited/duplicate terminals and a different wrapper or refusal.
    pub fn read_refused(&mut self, to: Token, reply: people::Reply) -> Result<(), &'static str> {
        if !self.reads.contains_key(&to) {
            return Err("unsolicited or duplicate read-pressure terminal");
        }
        if reply != people::Reply::Refused(people::Refusal::Busy) {
            return Err("read pressure requires immediate retryable Busy refusal");
        }
        self.reads.remove(&to);
        self.read_terminals.insert(to);
        self.busy_reads.insert(to);
        Ok(())
    }

    /// Count independently consumed pressure terminals; successful retried views
    /// remain separate obligations (domain/engine.md, section 7.7).
    #[must_use]
    pub fn busy_reads(&self) -> usize {
        self.busy_reads.len()
    }

    /// Check one authenticated held view and its durable semantic record.
    ///
    /// # Errors
    /// Rejects duplicate/wrong reader, wrong revision/recipient or premature view
    /// (domain/engine.md, section 7.7; domain/tasks.md, section 15).
    pub fn viewed(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        to: Token,
        person: u64,
        context: &tasks::EscalationContext,
    ) -> Result<(), &'static str> {
        let (expected_person, expected) = self.reads.get(&to).ok_or("unsolicited or duplicate escalation view")?;
        if person != *expected_person
            || Some(context.task) != self.task
            || context.project != 1
            || Some(context.requester) != self.people[0]
            || context.why != tasks::Hold::Failures(tasks::Class::Run)
            || context.escalation != *expected
        {
            return Err("held view differs from scripted authenticated revision");
        }
        let record = task(rows, context.task).ok_or("view without durable held task")?;
        if !held(record)
            || record.escalation != context.escalation
            || record.numbers.spent != 3
            || record.run_spent != 3
        {
            return Err("held view preceded routed priced durability");
        }
        self.reads.remove(&to);
        self.read_terminals.insert(to);
        Ok(())
    }

    /// Check one ACK per scripted worker terminal after durable evidence.
    ///
    /// # Errors
    /// Rejects duplicates, wrong fences or an ACK preceding its charge
    /// (domain/engine.md, sections 7.4 and 7.7).
    pub fn acknowledged(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        number: u64,
        attempt: u64,
    ) -> Result<(), &'static str> {
        if Some(number) != self.task
            || !self.terminals.contains(&attempt)
            || !rows.contains_key(&Key::Terminal { task: number, attempt })
        {
            return Err("worker ACK before exact durable terminal");
        }
        let index =
            self.assignments.iter().position(|known| *known == attempt).ok_or("ACK names unscripted attempt")?;
        let expected_end = if index == 0 {
            tasks::End::Failed(tasks::Class::Run)
        } else {
            tasks::End::Finished { result: tasks::TaskResult::Report { words: REPORT.into() }, cancel_delegates: false }
        };
        let expected_cumulative = if index == 0 { 3 } else { 2 };
        if !matches!(rows.get(&Key::Terminal { task: number, attempt }), Some(Record::Terminal(terminal)) if terminal.task == number && terminal.attempt == attempt && terminal.cumulative == expected_cumulative && terminal.end == expected_end)
        {
            return Err("worker ACK has changed durable priced terminal");
        }
        if !self.acknowledgements.insert(attempt) {
            return Err("worker terminal ACK twice");
        }
        Ok(())
    }

    /// Check the sole successful report after total expense and final funding.
    ///
    /// # Errors
    /// Rejects wrong words/reader, missing closure, duplicate result or bad spend
    /// (domain/engine.md, section 7.7; domain/tasks.md, section 15).
    pub fn result(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        person: u64,
        number: u64,
        words: &[u8],
    ) -> Result<(), &'static str> {
        if !self.story.succeeds() || self.results != 0 {
            return Err("unscripted or duplicate final report");
        }
        if Some(person) != self.people[0] || Some(number) != self.task || words != REPORT {
            return Err("report differs from requester script");
        }
        if self.terminals.len() != 2 {
            return Err("report before two independently observed terminal commits");
        }
        self.final_state(rows)?;
        self.results += 1;
        Ok(())
    }

    /// Verify finite reservation conservation at final success or rejection.
    ///
    /// # Errors
    /// Rejects lost/doubled prices, held reopening or wrong final funding
    /// (domain/tasks.md, sections 12 and 15).
    pub fn final_state(&self, rows: &BTreeMap<Key, Record>) -> Result<(), &'static str> {
        let number = self.task.ok_or("no script task")?;
        let record = task(rows, number).ok_or("missing final task")?;
        let expected_funder =
            tasks::Funder::Pool { project: 1, person: self.people[0].ok_or("no requester identity")?, period: 1 };
        if record.funder != expected_funder || record.numbers.budget != 100 {
            return Err("final task changed its authentic funding source or budget");
        }
        let Some(Record::Tasks(tasks::Stored::Ledger(pool))) =
            rows.get(&Key::Tasks(tasks::Key::Ledger(expected_funder)))
        else {
            return Err("missing authentic requester funding pool");
        };
        let Some(Record::Tasks(tasks::Stored::Ledger(period))) =
            rows.get(&Key::Tasks(tasks::Key::Ledger(tasks::Funder::Period { project: 1, period: 1 })))
        else {
            return Err("missing authentic deployment period");
        };
        if pool.numbers.budget != 500
            || period.numbers != (tasks::Numbers { budget: 1000, spent: 0, spent_below: 0, reserved: 500 })
        {
            return Err("finite person carve changed deployment conservation");
        }
        let succeeds = self.story.succeeds();
        if record.numbers.spent != if succeeds { 5 } else { 3 }
            || pool.numbers.spent != 0
            || pool.numbers.spent_below != if succeeds { 5 } else { 0 }
            || pool.numbers.reserved != if succeeds { 0 } else { 100 }
        {
            return Err("expense lost doubled or posted to wrong funding source");
        }
        if !succeeds
            && (!held(record)
                || !matches!(record.escalation, tasks::Escalation::Rejected { ref reason, .. } if reason.as_ref() == REASON))
        {
            return Err("rejected chat reopened or lost bounded reason");
        }
        if succeeds {
            if !matches!(record.phase, tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Report { ref words })) if words.as_ref() == REPORT)
            {
                return Err("success did not end with actual report");
            }
            if rows.contains_key(&Key::Tasks(tasks::Key::Live(number)))
                || rows.contains_key(&Key::RunProof { task: number })
            {
                return Err("ended report retained live task or proof");
            }
            if !matches!(rows.get(&Key::Tasks(tasks::Key::Closure { task: number, generation: record.allotment })), Some(Record::Tasks(tasks::Stored::Closure(closure))) if closure.task == number && closure.funder == expected_funder && closure.spent == 5 && closure.budget == 100)
            {
                return Err("report before exact durable financial closure");
            }
        }
        Ok(())
    }

    /// All outside obligations complete; the driver separately checks pending
    /// queues, real root quiescence and durable final state (domain/engine.md, section 7.7).
    #[must_use]
    pub fn done(&self) -> bool {
        self.people.iter().all(Option::is_some)
            && self.start_replies == 1
            && self.assignments.len() == if self.story.succeeds() { 2 } else { 1 }
            && self.acknowledgements.len() == self.assignments.len()
            && self.asks.is_empty()
            && self.reads.is_empty()
            && self.archives.len() == if self.story.passes() { 2 } else { 1 }
            && self.results == u32::from(self.story.succeeds())
    }
}
