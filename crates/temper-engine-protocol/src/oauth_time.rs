//! Retry-After is interpreted against injected wall time, then becomes a span.
use skein_lib::{Duration, Wall};
#[must_use]
pub fn retry_after(raw: &[u8], wall: Wall) -> Duration {
    let value = trim(raw);
    if let Some(seconds) = number(value) {
        return Duration::from_secs(seconds);
    }
    match date(value) {
        Some(at) => Duration::from_nanos(at.saturating_sub(wall.as_nanos())),
        None => Duration::ZERO,
    }
}
fn trim(bytes: &[u8]) -> &[u8] {
    let mut start = 0_usize;
    let mut end = bytes.len();
    for &byte in bytes {
        if byte == b' ' || byte == b'\t' {
            start = start.saturating_add(1);
        } else {
            break;
        }
    }
    for &byte in bytes.get(start..).unwrap_or_default().iter().rev() {
        if byte == b' ' || byte == b'\t' {
            end = end.saturating_sub(1);
        } else {
            break;
        }
    }
    bytes.get(start..end).unwrap_or_default()
}
fn number(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let mut out = 0_u64;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        out = out.checked_mul(10)?.checked_add(u64::from(byte.checked_sub(b'0')?))?;
    }
    Some(out)
}
fn date(value: &[u8]) -> Option<u64> {
    // HTTP senders use IMF-fixdate, whose width is fixed (RFC 9110, 5.6.7).
    if value.len() != 29
        || value.get(3..5)? != b", "
        || value.get(7)? != &b' '
        || value.get(11)? != &b' '
        || value.get(16)? != &b' '
        || value.get(19)? != &b':'
        || value.get(22)? != &b':'
        || value.get(25..)? != b" GMT"
    {
        return None;
    }
    let mut weekday = false;
    for name in [b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat", b"Sun"] {
        if value.get(..3)? == name {
            weekday = true;
            break;
        }
    }
    if !weekday {
        return None;
    }
    let day = number(value.get(5..7)?)?;
    let year = number(value.get(12..16)?)?;
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let mut month = None;
    for (index, name) in
        [b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec"]
            .iter()
            .enumerate()
    {
        if value.get(8..11)? == *name {
            month = Some(index);
            break;
        }
    }
    let month = month?;
    let days = months(year);
    if day == 0 || day > *days.get(month)? {
        return None;
    }
    let mut elapsed = 0_u64;
    for prior in 1970..year {
        elapsed = elapsed.checked_add(if leap(prior) { 366 } else { 365 })?;
    }
    for &length in days.get(..month)? {
        elapsed = elapsed.checked_add(length)?;
    }
    elapsed = elapsed.checked_add(day.checked_sub(1)?)?;
    let hour = number(value.get(17..19)?)?;
    let minute = number(value.get(20..22)?)?;
    let second = number(value.get(23..25)?)?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    elapsed
        .checked_mul(86400)?
        .checked_add(hour.checked_mul(3600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?
        .checked_mul(1_000_000_000)
}
const fn leap(year: u64) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}
const fn months(year: u64) -> [u64; 12] {
    [31, if leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
}
