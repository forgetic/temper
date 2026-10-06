//! Short facts whose deadlines are monotonic; the view supplies their words.
use crate::Refusal;
use skein_lib::Time;

/// A notice kind the view can say in words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoticeKind {
    RequestFull,
    WordsTooLong,
    Refused(Refusal),
    WentOffline,
    CameBack,
}

/// A transient notice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Notice {
    pub kind: NoticeKind,
    pub until: Time,
}
