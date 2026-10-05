use crate::{Class, Hold, Party, Status};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    Made { task: u64, requester: Party },
    Claimed { task: u64, attempt: u64 },
    Failed { task: u64, class: Class },
    Held { task: u64, why: Hold },
    Ended { task: u64, status: Status },
}
