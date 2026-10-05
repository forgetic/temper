use crate::{Ending, Hold, Party};
use alloc::boxed::Box;
use skein_lib::{Duration, Wall};
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MessageKey {
    pub task: u64,
    pub number: u64,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NewsClass {
    Wakes,
    Kept,
    Dropped,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Rule {
    Never,
    Immediate,
    Batch { count: u32, age: Duration },
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ResultsWake {
    Never,
    Each,
    LastOrFailure,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WakePolicy {
    pub results: ResultsWake,
    pub questions: bool,
    pub answers: bool,
    pub words: Rule,
    pub news: Rule,
    pub notices: Rule,
    pub timers: bool,
    pub news_ceiling: NewsClass,
}
impl WakePolicy {
    pub const DEFAULT: WakePolicy = WakePolicy {
        results: ResultsWake::LastOrFailure,
        questions: true,
        answers: true,
        words: Rule::Immediate,
        news: Rule::Immediate,
        notices: Rule::Immediate,
        timers: true,
        news_ceiling: NewsClass::Wakes,
    };
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum UserMessage {
    Words { words: Box<[u8]> },
    Question { words: Box<[u8]> },
    Answer { question: u64, words: Box<[u8]> },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Notice {
    Held(Hold),
    Ended(Ending),
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Message {
    Amendment { revision: u64, reason: Box<[u8]> },
    Words { words: Box<[u8]> },
    Question { words: Box<[u8]> },
    Answer { question: u64, words: Box<[u8]> },
    Result { task: u64, ending: Ending },
    News { subscription: u64, class: NewsClass, words: Box<[u8]> },
    Notice { subscription: u64, target: u64, notice: Notice },
    Timer { subscription: u64, at: Wall },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Envelope {
    pub number: u64,
    pub task: u64,
    pub from: Party,
    pub message: Message,
    pub at: Wall,
    pub hits: u32,
    /// A batching threshold was reached in a durable decision.
    pub eligible: bool,
}
impl Envelope {
    #[must_use]
    pub const fn key(&self) -> MessageKey {
        MessageKey { task: self.task, number: self.number }
    }
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Receipt {
    pub number: u64,
    pub task: u64,
    pub from: Party,
    pub message: UserMessage,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Offer {
    pub attempt: u64,
    pub envelope: Envelope,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Question {
    pub number: u64,
    pub asker: u64,
    pub answerer: u64,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Interest {
    State,
    Result,
    StateAndResult,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SubscriptionKind {
    Task { target: u64, interest: Interest },
    Timer { at: Wall, period: Option<Duration> },
    Topic { connector: u16, topic: u64 },
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Subscription {
    pub number: u64,
    pub task: u64,
    pub kind: SubscriptionKind,
    pub pending: bool,
}
