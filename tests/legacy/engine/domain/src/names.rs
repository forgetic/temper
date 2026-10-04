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

use skein_lib::Token;
use temper_legacy_engine_domain::Item;

/// The checked production name, asserted for the world's admitted items.
#[must_use]
pub fn run(item: Item) -> Token {
    temper_engine_protocol::names::run(item).expect("the world's item fits v1")
}
#[must_use]
pub fn item(run: Token) -> Item {
    temper_engine_protocol::names::item(run).expect("the world's run name fits v1")
}
#[must_use]
pub fn attempt(item: Item, count: u64) -> Token {
    temper_engine_protocol::names::attempt(item, count).expect("the world's attempt fits v1")
}
#[must_use]
pub fn attempt_of(attempt: Token) -> (Item, u64) {
    temper_engine_protocol::names::attempt_of(attempt)
}
