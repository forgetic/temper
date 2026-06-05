#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TickReason {
    Initial,
    Poll,
    Wake,
    Audit,
}

impl TickReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            TickReason::Initial => "initial",
            TickReason::Poll => "poll",
            TickReason::Wake => "wake",
            TickReason::Audit => "audit",
        }
    }

    pub(crate) fn is_normal(self) -> bool {
        matches!(self, TickReason::Initial | TickReason::Poll)
    }
}

pub(crate) fn production_tick_id(worker: &str, reason: TickReason, sequence: u64) -> String {
    format!("tick/{worker}/{}/{}", reason.as_str(), sequence)
}
