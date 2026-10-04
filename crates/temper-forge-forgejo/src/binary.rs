//! Forgejo ids and the SHA256/base64 primitives also used by engine blocks.
use crate::{Error, types::ObjectFormat};
use alloc::boxed::Box;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use skein_lib::{Writer, bytes};

#[must_use]
pub fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
pub fn base64_encode(input: &[u8], cap: u32) -> Result<Box<[u8]>, Error> {
    let length = base64::encoded_len(input.len(), true).ok_or(Error::TooLarge)?;
    if length > usize::try_from(cap).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    let mut out = bytes::zeroed(length);
    let wrote = STANDARD.encode_slice(input, &mut out).expect("the measured base64 fits");
    assert!(wrote == length, "base64 was measured exactly");
    Ok(out)
}
pub fn base64_decode(input: &[u8], cap: u32) -> Result<Box<[u8]>, Error> {
    // Decoding into the bounded final buffer avoids library allocating first.
    let estimated = base64::decoded_len_estimate(input.len());
    let allowed = usize::try_from(cap).expect("u32 fits usize");
    if estimated > allowed.saturating_add(2) {
        return Err(Error::TooLarge);
    }
    let mut out = bytes::zeroed(estimated);
    let Ok(length) = STANDARD.decode_slice(input, &mut out) else { return Err(Error::Malformed) };
    if length > allowed {
        return Err(Error::TooLarge);
    }
    Ok(bytes::copy_of(out.get(..length).expect("decoder length is within its buffer")))
}
pub fn unhex(input: &[u8], output: &mut [u8]) -> Result<(), Error> {
    if input.len() != output.len().checked_mul(2).ok_or(Error::TooLarge)? {
        return Err(Error::Malformed);
    }
    for (pair, out) in input.chunks_exact(2).zip(output) {
        let a = *pair.first().expect("chunk has two bytes");
        let b = *pair.get(1).expect("chunk has two bytes");
        *out = nibble(a)?.checked_mul(16).expect("a nibble fits").checked_add(nibble(b)?).expect("two nibbles fit");
    }
    Ok(())
}
fn nibble(byte: u8) -> Result<u8, Error> {
    match byte {
        b'0'..=b'9' => Ok(byte.wrapping_sub(b'0')),
        b'a'..=b'f' => Ok(byte.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Ok(byte.wrapping_sub(b'A').wrapping_add(10)),
        _ => Err(Error::Malformed),
    }
}
pub fn commit(input: &[u8], format: ObjectFormat) -> Result<[u8; 32], Error> {
    let length = match format {
        ObjectFormat::Sha1 => 20,
        ObjectFormat::Sha256 => 32,
    };
    let mut out = [0; 32];
    unhex(input, out.get_mut(..length).expect("object id fits"))?;
    Ok(out)
}
#[must_use]
pub fn hex(input: &[u8]) -> Box<[u8]> {
    let mut out = Writer::new(input.len().checked_mul(2).expect("bounded hex length"));
    for &byte in input {
        out.put(&[digit(byte >> 4), digit(byte & 15)]).expect("hex was measured");
    }
    out.finish()
}
pub fn commit_hex(input: &[u8; 32], format: ObjectFormat) -> Result<Box<[u8]>, Error> {
    match format {
        ObjectFormat::Sha1 => {
            for &byte in input.get(20..).expect("padded id") {
                if byte != 0 {
                    return Err(Error::Malformed);
                }
            }
            Ok(hex(input.get(..20).expect("sha1 bytes")))
        }
        ObjectFormat::Sha256 => Ok(hex(input)),
    }
}
const fn digit(n: u8) -> u8 {
    if n < 10 { b'0'.wrapping_add(n) } else { b'a'.wrapping_add(n.wrapping_sub(10)) }
}
