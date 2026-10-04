//! Checked, reversible names shared with workers and agents.
use skein_lib::Token;
use temper_legacy_engine_domain::Item;

/// The v1 system-world layout: an eight-bit repository and a 24-bit item.
#[must_use]
pub const fn fits(item: Item) -> bool {
    item.repository < 256 && item.number < 1 << 24
}

#[must_use]
pub fn run(item: Item) -> Option<Token> {
    if !fits(item) {
        return None;
    }
    Some(Token::new(u64::from(item.repository) << 32_u32 | item.number))
}

#[must_use]
pub fn item(run: Token) -> Option<Item> {
    let raw = run.raw();
    let item = Item { repository: u32::try_from(raw >> 32_u32).ok()?, number: raw & 0xFFFF_FFFF };
    if fits(item) { Some(item) } else { None }
}

#[must_use]
pub fn attempt(item: Item, count: u64) -> Option<Token> {
    if !fits(item) || count >= 1 << 32_u32 {
        return None;
    }
    Some(Token::new(u64::from(item.repository) << 56 | item.number << 32_u32 | count))
}

#[must_use]
pub fn attempt_of(attempt: Token) -> (Item, u64) {
    let raw = attempt.raw();
    let repository = u32::try_from(raw >> 56_u32).expect("a byte fits u32");
    (Item { repository, number: (raw >> 32_u32) & 0xFF_FFFF }, raw & 0xFFFF_FFFF)
}

/// Peers supply both names. They must name the same item before an event
/// enters the domain; neither name overrides the other.
#[must_use]
pub fn pair(run: Token, attempt: Token) -> Option<(Item, u64)> {
    let item = item(run)?;
    let (other, count) = attempt_of(attempt);
    if item == other { Some((item, count)) } else { None }
}
