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

#[derive(Debug)]
pub(crate) struct Stream {
    pub watch: Watch,
    pub state: Following,
}
