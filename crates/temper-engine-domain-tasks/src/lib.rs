//! The tasks child: bounded live lifecycle, immutable dependencies and authentic
//! finite funding (programming-model.md, section 4.5; domain/tasks.md, sections
//! 2–5, 10 and 14). It keeps live task rows and name indices, retry timers,
//! configured agent charters, period/pool ledgers and optional diagnostic facts.
//! It never knows protocol bytes, worker channels, connector internals, people
//! authentication or the authority child's policy tables. Root supplies those
//! decisions through independent semantic types.
//!
//! `Domain::new` starts restoring. Only `Stored::Live` and `Stored::Ledger`
//! enter `Restore`; `Restored` validates the complete live graph and authentic
//! financial reservations before bounded activation/adoption outputs. Historical
//! ended rows remain in root storage and cannot re-enter live state
//! (domain/tasks.md, section 14).
//!
//! `step` receives root-authorized batches, finite-source inputs and actual
//! lifecycle notifications. Reply-bearing inputs have one terminal reply;
//! notifications can be ignored when stale. Priced turns/terminals preflight
//! the whole lifecycle/financial admission before posting the new cumulative
//! expense delta. Root owns exact transport replay proof and transcript rows;
//! tasks keeps no replay receipt, inbox, historical stub or result-delivery credit
//! (domain/tasks.md, section 14; domain/engine.md, section 7.5).
//!
//! `Request::Activate` carries a temporary bounded `RunContext`, not rendered
//! bytes or a second mutable task ledger. Root gathers the actual brief and
//! claims the task. `Request::Close` awaits root's actual closing obligations;
//! `Ended`, the historical task row, and original-source
//! posting share one atomic decision. The current root exposes person result notices and
//! delays outward replies/effects until durability; historical result reads
//! remain the root's route (domain/tasks.md, sections 5.6 and 14;
//! domain/engine.md, sections 5.6, 7.1, 7.5 and 9).
//!
//! `fire` expires at most one projected retry timer per call and advances bounded
//! cascades. Caller reserves free `Request` room from `max_out`; `worst_case`
//! counts retained containers/payloads and bounded scratch, while callers count
//! queues and owned output copies. `stored_bytes` measures existing row heap
//! without cloning; `Domain::funding` borrows authentic finite accounting without
//! creating a mutable root shadow (domain/tasks.md, sections 10 and 14).
//! Current chat construction supplies a report contract and empty parameters.
//! A held person chat owns one bounded semantic escalation revision/recipient
//! or rejected reason. Root authenticates its actual read/decision route, resolves
//! eligibility and owns immutable transport history; this child accepts/rejects/
//! passes only the authorized exact revision (domain/tasks.md, 15).
//! Agent execution is the current executor route; nonempty historical inputs,
//! inbox/wake/proposal, general amend/cancel/release/move and source-retirement
//! entrypoints are absent from this contracted API (domain/tasks.md, section 14).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
mod admission;
mod batch;
mod boundary;
mod closing;
mod domain;
mod escalation;
mod facts;
mod failures;
mod funders;
mod limits;
mod owned;
mod run;
mod stored;
#[cfg(test)]
mod tests;
mod value;
pub use boundary::{
    Accepted, Active, Cause, Closing, Contract, End, Ending, Event, Executor, Hold, Key, New, Parameter, Party, Phase,
    Problem, Refusal, Request, RunContext, Spec, Stage, Status, Stored, TaskRecord, TaskResult, Verdict, Was,
};
pub use domain::{Domain, fire, max_out, step};
pub use escalation::{Escalation, EscalationContext, EscalationDecision, EscalationHolder, EscalationOutcome};
pub use facts::Fact;
pub use failures::{Class, Retries, Retry, Tries};
pub use funders::FundingRecord;
pub use limits::{Limits, worst_case};
pub use owned::stored_bytes;
pub use value::{
    Authority, AuthorityExecutor, Budget, Delegation, Funder, Grant, Last, Numbers, Pattern, Scopes, Tools,
};
