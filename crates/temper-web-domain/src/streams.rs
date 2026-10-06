//! Watch state, its terminal and heartbeat.
use crate::Watch;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Following {
    Opening,
    Waiting,
    Live,
    Reopening,
    Closing,
    Backoff { attempt: u32 },
}

impl Following {
    pub(crate) const fn accepts_event(self) -> bool {
        match self {
            Following::Waiting | Following::Live => true,
            Following::Opening | Following::Reopening | Following::Closing | Following::Backoff { .. } => false,
        }
    }

    pub(crate) const fn backoff(self) -> bool {
        match self {
            Following::Backoff { .. } => true,
            Following::Opening | Following::Waiting | Following::Live | Following::Reopening | Following::Closing => {
                false
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct Stream {
    pub watch: Watch,
    pub state: Following,
}
