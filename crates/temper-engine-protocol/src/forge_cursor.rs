//! Opaque Forgejo wiki page cursors (forge.md, sections 3.2 and 12).
//! The domain passes these bytes unread. A cursor binds a page number to
//! the negotiated page size, so it cannot silently skip rows after a size
//! change. No HTTP request is made for a cursor that fails decoding.
use alloc::boxed::Box;
use skein_lib::{Reader, Writer};

/// Version, page number, and page size, with no trailing extension bytes.
pub const BYTES: u32 = 9;
const VERSION: u8 = 1;

/// The next page and its effective size. Both are positive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cursor {
    pub page: u32,
    pub size: u32,
}

/// Makes an opaque cursor. The first page is selected when no cursor was
/// supplied; every encoded cursor names a positive page and size.
#[must_use]
pub fn encode(cursor: Cursor) -> Option<Box<[u8]>> {
    if cursor.page == 0 || cursor.size == 0 {
        return None;
    }
    let mut out = Writer::new(usize::try_from(BYTES).ok()?);
    out.put(&[VERSION]).ok()?;
    out.put(&cursor.page.to_be_bytes()).ok()?;
    out.put(&cursor.size.to_be_bytes()).ok()?;
    Some(out.finish())
}

/// Decodes a cursor for the currently negotiated page `size`. Unknown
/// versions, zero fields, changed size, and any trailing bytes are refused.
#[must_use]
pub fn decode(bytes: &[u8], size: u32) -> Option<Cursor> {
    if u32::try_from(bytes.len()).ok()? != BYTES || size == 0 {
        return None;
    }
    let mut input = Reader::new(bytes);
    if input.u8()? != VERSION {
        return None;
    }
    let cursor = Cursor { page: input.u32()?, size: input.u32()? };
    if cursor.page == 0 || cursor.size != size || !input.is_empty() {
        return None;
    }
    Some(cursor)
}

/// The allocation of an encoded cursor; decoding borrows its input.
#[must_use]
pub const fn worst_case() -> u64 {
    9
}
