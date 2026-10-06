//! Content-free diagnostics from the forge client (domain/forge.md, section 5).
//!
//! Facts are bounded observations, not durable decisions. This module keeps
//! no state, knows no task content, and exposes only diagnostic value types.
use crate::api::Error;
use skein_lib::Time;
/// The request-budget class of a forge call.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Priority {
    /// A fact needed for a current decision.
    Fresh,
    /// An effect ready to execute.
    Write,
    /// A read of changed live resources.
    Keep,
    /// A periodic backstop read.
    Slow,
}
/// A bounded diagnostic about client work.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// A call was sent in this budget class.
    Sent { priority: Priority },
    /// A call failed in this budget class.
    Failed { priority: Priority, error: Error },
    /// The current request window was spent.
    Spent { until: Time },
    /// The forge refused calls until its reset.
    Limited { reset: Time },
    /// Client admission refused a request.
    Refused,
}
