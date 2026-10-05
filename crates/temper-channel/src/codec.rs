//! The frozen eight-byte header and exact-sized frame bodies.
use crate::{
    Sizes,
    primitives::Encoder,
    sizes,
    wire::{self, Message},
};
use alloc::boxed::Box;
use skein_lib::Reader;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header {
    pub kind: u16,
    pub length: u32,
}

#[must_use]
pub fn header(bytes: &[u8], sizes: &Sizes) -> Option<Header> {
    header_version(bytes, sizes, 1)
}
#[must_use]
pub fn header_version(bytes: &[u8], sizes: &Sizes, version: u16) -> Option<Header> {
    if bytes.len() != 8 {
        return None;
    }
    let mut input = Reader::new(bytes);
    let kind = input.u16()?;
    if input.u16()? != 0 {
        return None;
    }
    let length = input.u32()?;
    if length > sizes::largest_version(kind, sizes, version)? {
        return None;
    }
    Some(Header { kind, length })
}
#[must_use]
pub fn body(kind: u16, bytes: &[u8], sizes: &Sizes) -> Option<Message> {
    body_version(kind, bytes, sizes, 1)
}
#[must_use]
pub fn body_version(kind: u16, bytes: &[u8], sizes: &Sizes, version: u16) -> Option<Message> {
    if u32::try_from(bytes.len()).ok()? > sizes::largest_version(kind, sizes, version)? {
        return None;
    }
    let mut input = Reader::new(bytes);
    let message = wire::get_version(kind, &mut input, sizes, version)?;
    if input.is_empty() { Some(message) } else { None }
}
#[must_use]
pub fn decode(bytes: &[u8], sizes: &Sizes) -> Option<Message> {
    decode_version(bytes, sizes, 1)
}
#[must_use]
pub fn decode_version(bytes: &[u8], sizes: &Sizes, version: u16) -> Option<Message> {
    let (head, rest) = bytes.split_at_checked(8)?;
    let head = header_version(head, sizes, version)?;
    if u32::try_from(rest.len()).ok()? != head.length {
        return None;
    }
    body_version(head.kind, rest, sizes, version)
}
#[must_use]
pub fn frame_len(message: &Message, sizes: &Sizes) -> Option<u32> {
    frame_len_version(message, sizes, 1)
}
#[must_use]
pub fn frame_len_version(message: &Message, sizes: &Sizes, version: u16) -> Option<u32> {
    let mut measure = Encoder::measure();
    wire::put_version(&mut measure, message, sizes, version)?;
    let length = measure.length();
    if length > sizes::largest_version(message.kind(), sizes, version)? {
        return None;
    }
    length.checked_add(8)
}
#[must_use]
pub fn encode(message: &Message, sizes: &Sizes) -> Option<Box<[u8]>> {
    encode_version(message, sizes, 1)
}
#[must_use]
pub fn encode_version(message: &Message, sizes: &Sizes, version: u16) -> Option<Box<[u8]>> {
    let length = frame_len_version(message, sizes, version)?.checked_sub(8)?;
    let mut out = Encoder::writing(length.checked_add(8)?);
    out.u16(message.kind())?;
    out.u16(0)?;
    out.u32(length)?;
    wire::put_version(&mut out, message, sizes, version)?;
    Some(out.finish())
}

/// Fixed framing before a known kind or a bounded unknown-body skip.
#[must_use]
pub fn framing(bytes: &[u8]) -> Option<Header> {
    if bytes.len() != 8 {
        return None;
    }
    let mut input = Reader::new(bytes);
    let kind = input.u16()?;
    if input.u16()? != 0 {
        return None;
    }
    Some(Header { kind, length: input.u32()? })
}
