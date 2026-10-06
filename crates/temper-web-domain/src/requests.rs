//! Keyed requests persist through navigation, reload and link loss.
use crate::{Ask, Key};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Sending {
    InFlight { attempt: u32 },
    Backoff { attempt: u32 },
    Parked,
}

impl Sending {
    pub(crate) const fn waiting(self) -> bool {
        match self {
            Sending::Backoff { .. } | Sending::Parked => true,
            Sending::InFlight { .. } => false,
        }
    }

    pub(crate) const fn in_flight(self) -> bool {
        match self {
            Sending::InFlight { .. } => true,
            Sending::Backoff { .. } | Sending::Parked => false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Pending {
    pub key: Key,
    pub ask: Ask,
    pub state: Sending,
}
