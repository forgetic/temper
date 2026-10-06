//! Keyed requests persist through navigation, reload and link loss.
use crate::{Ask, Key};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Sending {
    InFlight { attempt: u32 },
    Backoff { attempt: u32 },
    Parked,
}

#[derive(Debug)]
pub(crate) struct Pending {
    pub key: Key,
    pub ask: Ask,
    pub state: Sending,
}
