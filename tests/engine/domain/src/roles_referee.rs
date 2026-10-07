//! Independent role-administration obligations over submitted transactions and
//! durable replies. The observer owns the outside roster/holder expectations;
//! it never reads root or child state (domain/people.md, section 5.1).

use jig_core_people as people;
use jig_core_tasks as tasks;
use skein_lib::Token;
use std::collections::{BTreeMap, BTreeSet};
use temper_engine_domain::{Key, Record, Write};

/// One independently expected successful roster replacement, including the
/// exact task rows permitted to change (domain/engine.md, section 5.1).
#[derive(Clone, Debug)]
pub struct Replacement {
    /// Complete project roster supplied by the outside owner (domain/people.md, section 4).
    pub holdings: Box<[people::Holding]>,

    /// Exact expected Waiting rows; every other task field remains byte-for-byte
    /// equal to its previous durable evidence.
    pub tasks: BTreeMap<u64, tasks::TaskRecord>,
}

#[derive(Clone, Debug)]
struct Obligation {
    person: u64,
    key: [u8; 16],
    ask: people::Ask,
    reply: people::Reply,
    replacement: Option<Replacement>,
}

/// Frozen pre-transaction observer used unchanged by positive controls and
/// corruption negatives. Its ledger consists entirely of actual store rows
/// and registered outside asks.
#[derive(Clone, Debug)]
pub struct Referee {
    rows: BTreeMap<Key, Record>,
    asks: BTreeMap<Token, Obligation>,
    terminals: BTreeSet<Token>,
    /// Number of successful atomic role replacements observed
    /// (domain/people.md, section 5.1).
    pub replacements: u32,
}

impl Referee {
    /// Start from actual pre-transaction durable evidence
    #[must_use]
    pub fn new(rows: BTreeMap<Key, Record>) -> Referee {
        Referee { rows, asks: BTreeMap::new(), terminals: BTreeSet::new(), replacements: 0 }
    }

    /// Register one authenticated outside call before dispatch; duplicate keys
    /// may have distinct reply rights (domain/people.md, section 5.1).
    pub fn ask(
        &mut self,
        to: Token,
        person: u64,
        key: [u8; 16],
        ask: people::Ask,
        reply: people::Reply,
        replacement: Option<Replacement>,
    ) {
        assert!(
            self.asks.insert(to, Obligation { person, key, ask, reply, replacement }).is_none(),
            "one fresh outside role-request reply right"
        );
    }

    /// Validate an atomic cohort before advancing the outside observer.
    ///
    /// # Errors
    /// Names missing cohort members, unauthorized changes, duplicate writes,
    /// altered funding, or a task field differing from the scripted expectation
    /// (domain/engine.md, section 5.1).
    #[expect(
        clippy::too_many_lines,
        reason = "one independent atomic-cohort check names each specific violated contract"
    )]
    pub fn commit(&mut self, writes: &[Write]) -> Result<(), &'static str> {
        let mut keys = BTreeSet::new();
        for write in writes {
            let key = match write {
                Write::Save(record) => record.key(),
                Write::Erase(key) => *key,
            };
            if !keys.insert(key) {
                return Err("same key twice in role transaction");
            }
        }
        let role_rows: Vec<_> = writes
            .iter()
            .filter_map(|write| match write {
                Write::Save(Record::People(people::Stored::Roles { project, holdings })) => Some((*project, holdings)),
                Write::Save(
                    Record::People(
                        people::Stored::Person { .. }
                        | people::Stored::ReadPosition { .. }
                        | people::Stored::SignIn { .. }
                        | people::Stored::Policy { .. }
                        | people::Stored::Answer { .. },
                    )
                    | Record::Tasks(_)
                    | Record::Call(_)
                    | Record::Deployment(_)
                    | Record::Turn(_)
                    | Record::RunProof(_)
                    | Record::Terminal(_)
                    | Record::EscalationDecision(_)
                    | Record::ProposalDecision(_)
                    | Record::Forge { .. },
                )
                | Write::Erase(_) => None,
            })
            .collect();
        let answers: Vec<_> = writes
            .iter()
            .filter_map(|write| match write {
                Write::Save(Record::People(people::Stored::Answer { key, ask, outcome, .. })) => {
                    if let people::Ask::SetRoles { .. } = ask.as_ref() {
                        Some((*key, ask.as_ref(), *outcome))
                    } else {
                        None
                    }
                }
                Write::Save(
                    Record::People(
                        people::Stored::Person { .. }
                        | people::Stored::ReadPosition { .. }
                        | people::Stored::SignIn { .. }
                        | people::Stored::Roles { .. }
                        | people::Stored::Policy { .. },
                    )
                    | Record::Tasks(_)
                    | Record::Call(_)
                    | Record::Deployment(_)
                    | Record::Turn(_)
                    | Record::RunProof(_)
                    | Record::Terminal(_)
                    | Record::EscalationDecision(_)
                    | Record::ProposalDecision(_)
                    | Record::Forge { .. },
                )
                | Write::Erase(_) => None,
            })
            .collect();
        let expected = answers.iter().find_map(|(key, ask, outcome)| {
            self.asks.values().find(|obligation| {
                obligation.person == key.person
                    && obligation.key == key.key
                    && obligation.ask == **ask
                    && obligation.reply == people::Reply::Outcome(*outcome)
            })
        });
        if !role_rows.is_empty() && expected.is_none() {
            return Err("roles and keyed answer are not atomic");
        }
        let replacement = expected.and_then(|obligation| obligation.replacement.as_ref());
        if let Some(replacement) = replacement {
            let Some(Obligation { ask: people::Ask::SetRoles { project, .. }, .. }) = expected else {
                return Err("role success has no outside request");
            };
            if role_rows.len() != 1 || role_rows[0].0 != *project {
                return Err("keyed role success lacks its roles row");
            }
            if role_rows[0].1.as_ref() != replacement.holdings.as_ref() {
                return Err("roles differ from outside roster");
            }
            for (number, expected_task) in &replacement.tasks {
                let found = writes.iter().find_map(|write| match write {
                    Write::Save(Record::Tasks(tasks::Stored::Live(record))) if record.number == *number => {
                        Some(record.as_ref())
                    }
                    Write::Save(
                        Record::Tasks(
                            tasks::Stored::Live(_)
                            | tasks::Stored::Ended(_)
                            | tasks::Stored::Stub(_)
                            | tasks::Stored::Ledger(_)
                            | tasks::Stored::History(_)
                            | tasks::Stored::PersonProposal(_),
                        )
                        | Record::People(_)
                        | Record::Call(_)
                        | Record::Deployment(_)
                        | Record::Turn(_)
                        | Record::RunProof(_)
                        | Record::Terminal(_)
                        | Record::EscalationDecision(_)
                        | Record::ProposalDecision(_)
                        | Record::Forge { .. },
                    )
                    | Write::Erase(_) => None,
                });
                if found.is_none() {
                    return Err("roles and changed waiting rows are not atomic");
                }
                let Some(found) = found else { return Err("roles and changed waiting rows are not atomic") };
                let mut comparable = found.clone();
                match (&mut comparable.escalation, &expected_task.escalation) {
                    (
                        tasks::Escalation::Waiting { since: actual, .. },
                        tasks::Escalation::Waiting { since: earliest, .. },
                    ) if *actual >= *earliest => *actual = *earliest,
                    (tasks::Escalation::Waiting { .. }, tasks::Escalation::Waiting { .. })
                    | (
                        tasks::Escalation::Unheld { .. }
                        | tasks::Escalation::Routing { .. }
                        | tasks::Escalation::Rejected { .. },
                        _,
                    )
                    | (
                        _,
                        tasks::Escalation::Unheld { .. }
                        | tasks::Escalation::Routing { .. }
                        | tasks::Escalation::Rejected { .. },
                    ) => {
                        return Err("reroute timestamp precedes the request");
                    }
                }
                if &comparable != expected_task {
                    return Err("reroute changed more than expected holder and revision");
                }
            }
        } else if !role_rows.is_empty() {
            return Err("refused role request changed the roster");
        }
        for write in writes {
            match write {
                Write::Save(Record::Tasks(tasks::Stored::Live(record))) => {
                    if replacement.is_none_or(|replacement| !replacement.tasks.contains_key(&record.number)) {
                        return Err("unchanged or rejected task was rewritten");
                    }
                }
                Write::Save(
                    Record::Tasks(_)
                    | Record::Call(_)
                    | Record::RunProof(_)
                    | Record::Terminal(_)
                    | Record::EscalationDecision(_)
                    | Record::ProposalDecision(_)
                    | Record::Turn(_),
                )
                | Write::Erase(
                    Key::Call(_)
                    | Key::Tasks(_)
                    | Key::RunProof { .. }
                    | Key::Terminal { .. }
                    | Key::EscalationDecision { .. }
                    | Key::ProposalDecision(_)
                    | Key::Turn { .. },
                ) => return Err("role administration changed funding or accepted work"),
                Write::Save(Record::People(_) | Record::Deployment(_)) | Write::Erase(Key::People(_)) => {}
                Write::Save(Record::Forge { .. }) | Write::Erase(Key::Forge(_)) => {
                    return Err("role administration changed forge state");
                }
                Write::Erase(Key::Deployment) => return Err("role administration erased deployment"),
            }
        }
        if replacement.is_some() {
            self.replacements += 1;
        }
        for write in writes {
            match write {
                Write::Save(record) => {
                    self.rows.insert(record.key(), record.clone());
                }
                Write::Erase(key) => {
                    self.rows.remove(key);
                }
            }
        }
        Ok(())
    }

    /// Consume exactly one expected external terminal after durable evidence.
    ///
    /// # Errors
    /// Rejects unsolicited/duplicate replies, wrong wrappers/outcomes, replies
    /// before durability, or admission refusals that replaced an existing key
    /// (domain/people.md, section 5.1; domain/engine.md, section 5.3).
    pub fn replied(
        &mut self,
        rows: &BTreeMap<Key, Record>,
        to: Token,
        reply: people::Reply,
    ) -> Result<(), &'static str> {
        let obligation = self.asks.get(&to).ok_or("unsolicited or duplicate role terminal")?;
        if reply != obligation.reply {
            return Err("role terminal differs from outside expectation");
        }
        let answer_key =
            Key::People(people::Key::Answer(people::RequestKey { person: obligation.person, key: obligation.key }));
        match reply {
            people::Reply::Outcome(people::Outcome::Refused(people::Refusal::Busy | people::Refusal::NotReady)) => {
                if rows.contains_key(&answer_key) {
                    return Err("retryable role refusal consumed an answered key");
                }
            }
            people::Reply::Outcome(outcome) => {
                if !matches!(rows.get(&answer_key), Some(Record::People(people::Stored::Answer { ask, outcome: saved, .. })) if ask.as_ref() == &obligation.ask && *saved == outcome)
                {
                    return Err("role terminal before exact durable keyed answer");
                }
            }
            people::Reply::Refused(_) => {
                if rows.get(&answer_key) != self.rows.get(&answer_key) {
                    return Err("role entrance refusal changed durable key");
                }
            }
            people::Reply::SignedIn { .. } | people::Reply::SignedOut => return Err("role ask got a session terminal"),
        }
        self.asks.remove(&to);
        self.terminals.insert(to);
        Ok(())
    }

    /// All registered terminal rights were consumed exactly once
    /// (domain/people.md, section 5.1).
    #[must_use]
    pub fn done(&self) -> bool {
        self.asks.is_empty()
    }
}
