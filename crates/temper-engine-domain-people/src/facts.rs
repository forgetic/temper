//! Optional content-free child observations (domain/people.md, section 12.1).
//! People emits these through its bounded diagnostic queue; dropping them
//! changes no decision. They neither persist state nor acknowledge root commits.

/// Content-free observations; keeping or dropping them changes no decision.
/// Optional child-to-parent content-free observation in a bounded queue; it is not a durable-write
/// or request-terminal acknowledgement. (domain/people.md, section 12.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// A new sign-in was admitted, with persistence requested from the parent. (domain/people.md,
    /// section 12.1).
    SignedIn {
        /** Stable authenticated person number. (domain/people.md, section 12.1). */
        person: u64,
        /** Newly admitted root-issued sign-in number. (domain/people.md, section 12.1). */
        sign_in: u64,
    },
    /// An existing sign-in was removed and its erase requested. (domain/people.md, section 12.1).
    SignedOut {
        /** Root-issued sign-in number that was removed or expired. (domain/people.md, section 12.1). */
        sign_in: u64,
    },
    /// A new eligible keyed request routed once to the root. (domain/people.md, section 12.1).
    Routed {
        /** Authenticated requester whose new flight routed. (domain/people.md, section 12.1). */
        person: u64,
    },
    /// A permanent keyed outcome was retained and its save requested; parent durability still
    /// governs replies. (domain/people.md, section 12.1).
    Answered {
        /** Person whose permanent keyed answer was saved in the child. (domain/people.md, section 12.1). */
        person: u64,
    },
}
