//! Lengths are checked before allocation; text remains bytes.
use alloc::boxed::Box;
use skein_lib::{Reader, Writer, bytes};

pub(crate) struct Encoder {
    writer: Option<Writer>,
    length: u32,
}
impl Encoder {
    pub(crate) const fn measure() -> Encoder {
        Encoder { writer: None, length: 0 }
    }
    pub(crate) fn writing(length: u32) -> Encoder {
        Encoder { writer: Some(Writer::new(usize::try_from(length).expect("a u32 fits usize"))), length: 0 }
    }
    pub(crate) const fn length(&self) -> u32 {
        self.length
    }
    pub(crate) fn finish(self) -> Box<[u8]> {
        self.writer.expect("the writing pass has a writer").finish()
    }
    pub(crate) fn raw(&mut self, value: &[u8]) -> Option<()> {
        self.length = self.length.checked_add(u32::try_from(value.len()).ok()?)?;
        match &mut self.writer {
            Some(writer) => writer.put(value).ok(),
            None => Some(()),
        }
    }
    pub(crate) fn u8(&mut self, value: u8) -> Option<()> {
        self.raw(&[value])
    }
    pub(crate) fn u16(&mut self, value: u16) -> Option<()> {
        self.raw(&value.to_be_bytes())
    }
    pub(crate) fn u32(&mut self, value: u32) -> Option<()> {
        self.raw(&value.to_be_bytes())
    }
    pub(crate) fn u64(&mut self, value: u64) -> Option<()> {
        self.raw(&value.to_be_bytes())
    }
    pub(crate) fn bool(&mut self, value: bool) -> Option<()> {
        self.u8(u8::from(value))
    }
    pub(crate) fn bytes(&mut self, value: &[u8], max: u32) -> Option<()> {
        let length = u32::try_from(value.len()).ok()?;
        if length > max {
            return None;
        }
        self.u32(length)?;
        self.raw(value)
    }
}

pub(crate) fn bytes(input: &mut Reader<'_>, max: u32) -> Option<Box<[u8]>> {
    let length = input.u32()?;
    if length > max || length > input.remaining() {
        return None;
    }
    let value = input.bytes(length)?;
    Some(bytes::copy_of(value))
}
pub(crate) fn count(input: &mut Reader<'_>, max: u32, minimum: u32) -> Option<u32> {
    let length = input.u32()?;
    if length > max || length.checked_mul(minimum)? > input.remaining() {
        return None;
    }
    Some(length)
}
pub(crate) fn boolean(input: &mut Reader<'_>) -> Option<bool> {
    match input.u8()? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}
