//! Submitted relays survive the link that carried them.
use skein_lib::{Map, Token};
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Relay {
    pub run: Token,
    pub attempt: Token,
}
#[derive(Debug)]
pub struct Table {
    held: Map<Token, Relay>,
}
impl Table {
    #[must_use]
    pub fn new(capacity: u32) -> Table {
        Table { held: Map::with_capacity(capacity) }
    }
    pub fn open(&mut self, call: Token, run: Token, attempt: Token) -> bool {
        if self.held.contains_key(&call) {
            return false;
        }
        self.held.insert(call, Relay { run, attempt }).is_ok()
    }
    pub fn answer(&mut self, call: Token, run: Token, attempt: Token) -> bool {
        if self.held.get(&call) != Some(&Relay { run, attempt }) {
            return false;
        }
        assert_eq!(self.held.remove(&call), Some(Relay { run, attempt }), "the checked relay remains owned");
        true
    }
    /// The caller emits one `RelayCancelled` only for an extant wait.
    pub fn cancel(&mut self, call: Token) -> bool {
        self.held.remove(&call).is_some()
    }
    #[must_use]
    pub fn len(&self) -> u32 {
        self.held.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}
#[must_use]
pub fn worst_case(capacity: u32) -> Option<u64> {
    Map::<Token, Relay>::worst_case(capacity)
}
