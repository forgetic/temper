//! Optional bounded content-free observations (domain/tasks.md, sections
//! 11 and 14). Facts are diagnostic; retaining or dropping them changes no
//! lifecycle, reply, financial posting or durability decision.
use crate::{Class, Hold, Party, Status};

/// Optional content-free task observation in a bounded queue; it is neither a durable
/// acknowledgement nor a requester delivery credit, and dropping it changes no decision.
/// (domain/tasks.md, sections 5 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// New task admitted and its save requested; root durability still follows. (domain/tasks.md,
    /// sections 5 and 14).
    Made {
        /** New admitted task number. (domain/tasks.md, sections 5 and 14). */
        task: u64,
        /** Root-verified creator/requester of the task. (domain/tasks.md, sections 5 and 14). */
        requester: Party,
    },
    /// New fenced activation claimed and its save requested. (domain/tasks.md, sections 5 and 14).
    Claimed {
        /** Task newly claimed for activation. (domain/tasks.md, sections 5 and 14). */
        task: u64,
        /** `Exact` root-issued `new` claim identity. (domain/tasks.md, sections 5 and 14). */
        attempt: u64,
    },
    /// One activation failure classified and counted. (domain/tasks.md, sections 5 and 14).
    Failed {
        /** Task whose classified activation failure was counted. (domain/tasks.md, sections 5 and 14). */
        task: u64,
        /** Failure category counted in its retry history. (domain/tasks.md, sections 5 and 14). */
        class: Class,
    },
    /// Task entered a hold; does not acknowledge any `Stop` or root decision. (domain/tasks.md,
    /// sections 5 and 14).
    Held {
        /** Task newly held for a decision. (domain/tasks.md, sections 5 and 14). */
        task: u64,
        /** Reason for the `new` hold. (domain/tasks.md, sections 5 and 14). */
        why: Hold,
    },
    /// Task settled and emitted its final financial/persistence/requester outputs; not a delivery
    /// or durability acknowledgement. (domain/tasks.md, sections 5 and 14).
    Ended {
        /** Task removed from the live arena after settlement. (domain/tasks.md, sections 5 and 14). */
        task: u64,
        /** Content-free final classification, separate from the bounded result bytes. (domain/tasks.md, sections 5 and 14). */
        status: Status,
    },
}
