//! The names of runs and attempts on a worker's channel, as the protocol
//! layers pack them, which the system worlds share: on the worker's channel
//! both are tokens, and the engine names a run by its item and an attempt by
//! the item and its count (engine-domain.md, section 8).
//!
//! A run is its item's: the repository's index in the high half, the item's
//! number in the low. An attempt is its item's and its count: the repository
//! in the top byte, the number in the next three and the count in the low
//! half, so that every attempt of every item has a name of its own, and the
//! item can be read back from it.

use temper_engine_domain::Item;
use temper_lib::Token;

/// The channel's name for the item's run.
#[must_use]
pub fn run(item: Item) -> Token {
    assert!(item.number < 1 << 24, "an item's number fits its place in a name");
    Token::new(u64::from(item.repository) << 32 | item.number)
}

/// The item whose run the channel names `run`.
#[must_use]
pub fn item(run: Token) -> Item {
    let raw = run.raw();
    Item { repository: u32::try_from(raw >> 32).expect("a repository's index"), number: raw & 0xFFFF_FFFF }
}

/// The channel's name for the item's attempt `attempt`.
#[must_use]
pub fn attempt(item: Item, attempt: u64) -> Token {
    assert!(item.repository < 1 << 8 && item.number < 1 << 24, "an item fits its place in a name");
    assert!(attempt < 1 << 32, "an attempt's count fits its place in a name");
    Token::new(u64::from(item.repository) << 56 | item.number << 32 | attempt)
}

/// The item and the attempt's count an attempt's name stands for.
#[must_use]
pub fn attempt_of(attempt: Token) -> (Item, u64) {
    let raw = attempt.raw();
    let repository = u32::try_from(raw >> 56).expect("a byte");
    (Item { repository, number: (raw >> 32) & 0xFF_FFFF }, raw & 0xFFFF_FFFF)
}
