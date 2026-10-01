//! Owned bytes (6.1): a `Box<[u8]>` allocated at its final length, moved from
//! owner to owner, never shared.

use alloc::boxed::Box;

/// A copy of `bytes` in a box of exactly their length.
///
/// This is "copy at emission" (6.3): data a layer keeps and also sends goes out
/// as a copy made when the request is emitted.
#[must_use]
pub fn copy_of(bytes: &[u8]) -> Box<[u8]> {
    Box::from(bytes)
}
