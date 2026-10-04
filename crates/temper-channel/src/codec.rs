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
    if bytes.len() != 8 {
        return None;
    }
    let mut input = Reader::new(bytes);
    let kind = input.u16()?;
    if input.u16()? != 0 {
        return None;
    }
    let length = input.u32()?;
    if length > sizes::largest(kind, sizes)? {
        return None;
    }
    Some(Header { kind, length })
}
#[must_use]
pub fn body(kind: u16, bytes: &[u8], sizes: &Sizes) -> Option<Message> {
    if u32::try_from(bytes.len()).ok()? > sizes::largest(kind, sizes)? {
        return None;
    }
    let mut input = Reader::new(bytes);
    let message = wire::get_message(kind, &mut input, sizes)?;
    if input.is_empty() { Some(message) } else { None }
}
#[must_use]
pub fn decode(bytes: &[u8], sizes: &Sizes) -> Option<Message> {
    let (head, rest) = bytes.split_at_checked(8)?;
    let head = header(head, sizes)?;
    if u32::try_from(rest.len()).ok()? != head.length {
        return None;
    }
    body(head.kind, rest, sizes)
}
#[must_use]
pub fn frame_len(message: &Message, sizes: &Sizes) -> Option<u32> {
    let mut measure = Encoder::measure();
    wire::put_message(&mut measure, message, sizes)?;
    let length = measure.length();
    if length > sizes::largest(message.kind(), sizes)? {
        return None;
    }
    length.checked_add(8)
}
#[must_use]
pub fn encode(message: &Message, sizes: &Sizes) -> Option<Box<[u8]>> {
    let length = frame_len(message, sizes)?.checked_sub(8)?;
    let mut out = Encoder::writing(length.checked_add(8)?);
    out.u16(message.kind())?;
    out.u16(0)?;
    out.u32(length)?;
    wire::put_message(&mut out, message, sizes)?;
    Some(out.finish())
}
