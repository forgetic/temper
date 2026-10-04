/// Content-free observations; keeping or dropping them changes no decision.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    SignedIn { person: u64, sign_in: u64 },
    SignedOut { sign_in: u64 },
    Routed { person: u64 },
    Answered { person: u64 },
}
