//! Notes' write machine (jig's domain/engine.md, sections 5.3, 5.6 and 10).
//! One store load is in flight at a time; the parent echoes its token. The
//! parent commits each emitted `Save` and `Erase` with that decision.

use skein_lib::{Env, Queue, Token};

use crate::boundary::{Author, Change, Entry, Event, Key, Line, New, Range, Record, Refusal, Request, Rows, Scope};
use crate::limits::Limits;

/// The largest number of requests one step emits.
pub const MAX_OUT: u32 = 3;

/// One write waiting for the store.
#[derive(Debug)]
pub(crate) struct Pending {
    owner: Token,
    load_owner: Token,
    action: Action,
    phase: Phase,
}

#[derive(Debug)]
enum Action {
    Write { entry: New, recalled: Option<u32> },
    Edit { party: u64, name: u64, change: Change },
}

#[derive(Debug)]
enum Phase {
    Entry,
    Count { scope: Scope, count: u32, after: Option<u64> },
}

/// The notes child keeps only a pending write; entries live in the store.
#[derive(Debug)]
pub struct Domain {
    pending: Option<Pending>,
    next_load: u64,
}

impl Domain {
    /// Create an empty notes child.
    #[must_use]
    pub const fn new(_limits: &Limits) -> Domain {
        Domain { pending: None, next_load: 1 }
    }

    /// Whether a call is waiting for a store page.
    #[must_use]
    pub const fn busy(&self) -> bool {
        self.pending.is_some()
    }
}

/// Room the parent reserves before stepping the child.
#[must_use]
pub const fn max_out(_limits: &Limits) -> u32 {
    MAX_OUT
}

/// Decide one event and emit writes or a store load.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Write { owner, entry, recalled } => {
            if domain.busy() {
                push(out, Request::Refused { owner, why: Refusal::Busy });
            } else if !valid_new(&entry, &env.limits) {
                push(out, Request::Refused { owner, why: Refusal::Oversized });
            } else {
                begin(domain, owner, Action::Write { entry, recalled }, out);
            }
        }
        Event::Edit { owner, party, name, change } => {
            if domain.busy() {
                push(out, Request::Refused { owner, why: Refusal::Busy });
            } else if !valid_change(&change, &env.limits) {
                push(out, Request::Refused { owner, why: Refusal::Oversized });
            } else {
                begin(domain, owner, Action::Edit { party, name, change }, out);
            }
        }
        Event::Loaded { owner, rows, more } => loaded(domain, env, owner, rows, more, out),
        Event::Restore { record } => match record {
            Record::Entry(_) | Record::Line(_) => {}
        },
        Event::Restored => {}
    }
}

fn begin(domain: &mut Domain, owner: Token, action: Action, out: &mut Queue<Request>) {
    let name = match &action {
        Action::Write { entry, recalled: _ } => entry.name,
        Action::Edit { party: _, name, change: _ } => *name,
    };
    let load_owner = Token::new(domain.next_load);
    domain.next_load = domain.next_load.checked_add(1).expect("store load token space is sufficient");
    domain.pending = Some(Pending { owner, load_owner, action, phase: Phase::Entry });
    push(out, Request::Load { owner: load_owner, range: Range::Entry { name } });
}

fn loaded(domain: &mut Domain, env: &Env<Limits>, owner: Token, rows: Rows, more: bool, out: &mut Queue<Request>) {
    let mut pending = domain.pending.take().expect("a store page answers an outstanding load");
    assert!(pending.load_owner == owner, "a store page echoes its load token");
    assert!(rows.records.capacity() <= env.limits.load_rows, "the store page fits its bound");
    match pending.phase {
        Phase::Entry => {
            assert!(!more, "an entry lookup fits one page");
            let mut found = None;
            for record in rows.records.as_slice() {
                match record {
                    Record::Entry(entry) => {
                        assert!(found.is_none(), "one globally named entry");
                        found = Some(entry.clone());
                    }
                    Record::Line(_) => unreachable!("entry lookup returns entries"),
                }
            }
            entry_loaded(domain, pending, found, out);
        }
        Phase::Count { scope, mut count, mut after } => {
            assert!(!rows.records.is_empty() || !more, "a continued page advances");
            for record in rows.records.as_slice() {
                match record {
                    Record::Line(line) => {
                        assert!(line.scope == scope, "scope load returns its own lines");
                        match after {
                            None => {}
                            Some(last) => assert!(line.name > last, "scope lines are ordered"),
                        }
                        count = count.checked_add(1).expect("scope count fits u32");
                        after = Some(line.name);
                    }
                    Record::Entry(_) => unreachable!("scope load returns lines"),
                }
            }
            if more {
                pending.phase = Phase::Count { scope: scope.clone(), count, after };
                push(out, Request::Load { owner, range: Range::Lines { scope, after } });
                domain.pending = Some(pending);
            } else if count >= env.limits.entries_per_scope {
                push(out, Request::Refused { owner: pending.owner, why: Refusal::Full });
            } else {
                match pending.action {
                    Action::Write { entry, recalled: None } => save_new(pending.owner, entry, 1, out),
                    Action::Write { entry: _, recalled: Some(_) } | Action::Edit { .. } => {
                        unreachable!("only a new entry counts a scope")
                    }
                }
            }
        }
    }
}

fn entry_loaded(domain: &mut Domain, mut pending: Pending, found: Option<Entry>, out: &mut Queue<Request>) {
    match pending.action {
        Action::Write { entry, recalled } => match found {
            None => match recalled {
                None => {
                    let scope = entry.scope.clone();
                    pending.action = Action::Write { entry, recalled: None };
                    pending.phase = Phase::Count { scope: scope.clone(), count: 0, after: None };
                    push(out, Request::Load { owner: pending.load_owner, range: Range::Lines { scope, after: None } });
                    domain.pending = Some(pending);
                }
                Some(_) => push(out, Request::Refused { owner: pending.owner, why: Refusal::Missing }),
            },
            Some(old) => match recalled {
                None => push(out, Request::Refused { owner: pending.owner, why: Refusal::Exists }),
                Some(recalled) => {
                    assert!(old.name == entry.name, "entry lookup returns the named entry");
                    if old.revision != recalled || old.scope != entry.scope {
                        push(out, Request::Refused { owner: pending.owner, why: Refusal::Moved });
                    } else if let Some(revision) = old.revision.checked_add(1) {
                        save_new(pending.owner, entry, revision, out);
                    } else {
                        push(out, Request::Refused { owner: pending.owner, why: Refusal::RevisionExhausted });
                    }
                }
            },
        },
        Action::Edit { party, name, change } => match found {
            None => push(out, Request::Refused { owner: pending.owner, why: Refusal::Missing }),
            Some(mut old) => match change {
                Change::Correct { description, body, references, recalled } => {
                    assert!(old.name == name, "entry lookup returns the named entry");
                    if old.revision != recalled {
                        push(out, Request::Refused { owner: pending.owner, why: Refusal::Moved });
                    } else if let Some(revision) = old.revision.checked_add(1) {
                        old.description = description;
                        old.body = body;
                        old.references = references;
                        old.author = Author::Party { party };
                        old.revision = revision;
                        save_entry(pending.owner, old, out);
                    } else {
                        push(out, Request::Refused { owner: pending.owner, why: Refusal::RevisionExhausted });
                    }
                }
                Change::Delete { recalled } => {
                    assert!(old.name == name, "entry lookup returns the named entry");
                    if old.revision == recalled {
                        push(out, Request::Erase { key: Key::Entry { name } });
                        push(out, Request::Erase { key: Key::Line { scope: old.scope, name } });
                        push(out, Request::Deleted { owner: pending.owner, name });
                    } else {
                        push(out, Request::Refused { owner: pending.owner, why: Refusal::Moved });
                    }
                }
            },
        },
    }
}

fn save_new(owner: Token, new: New, revision: u32, out: &mut Queue<Request>) {
    save_entry(
        owner,
        Entry {
            name: new.name,
            scope: new.scope,
            description: new.description,
            body: new.body,
            references: new.references,
            author: new.author,
            revision,
        },
        out,
    );
}

fn save_entry(owner: Token, entry: Entry, out: &mut Queue<Request>) {
    let line = Line {
        scope: entry.scope.clone(),
        name: entry.name,
        description: entry.description.clone(),
        revision: entry.revision,
    };
    let name = entry.name;
    let revision = entry.revision;
    push(out, Request::Save { record: Record::Entry(entry) });
    push(out, Request::Save { record: Record::Line(line) });
    push(out, Request::Written { owner, name, revision });
}

fn valid_new(entry: &New, limits: &Limits) -> bool {
    valid_scope(&entry.scope, limits)
        && valid_text(&entry.description, limits.description_bytes, true)
        && valid_text(&entry.body, limits.body_bytes, false)
        && entry.references.capacity() <= limits.references
}

fn valid_change(change: &Change, limits: &Limits) -> bool {
    match change {
        Change::Correct { description, body, references, recalled: _ } => {
            valid_text(description, limits.description_bytes, true)
                && valid_text(body, limits.body_bytes, false)
                && references.capacity() <= limits.references
        }
        Change::Delete { recalled: _ } => true,
    }
}

fn valid_scope(scope: &Scope, limits: &Limits) -> bool {
    match scope {
        Scope::Deployment | Scope::Project { .. } | Scope::Goal { .. } => true,
        Scope::Resources { project: _, connector: _, pattern } => match u32::try_from(pattern.0.len()) {
            Ok(len) => len <= limits.pattern_bytes,
            Err(_) => false,
        },
    }
}

fn valid_text(bytes: &[u8], bound: u32, one_line: bool) -> bool {
    let fits = match u32::try_from(bytes.len()) {
        Ok(len) => len <= bound,
        Err(_) => false,
    };
    fits && (!one_line || (!bytes.contains(&b'\n') && !bytes.contains(&b'\r')))
}

fn push(out: &mut Queue<Request>, request: Request) {
    out.try_push(request).expect("parent reserved max_out before stepping notes");
}
