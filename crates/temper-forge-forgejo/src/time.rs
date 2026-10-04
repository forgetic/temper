//! Source RFC3339 timestamps, never monotonic deadlines.
use crate::{Error, json};
use alloc::boxed::Box;
use skein_lib::Writer;

const SECOND: u64 = 1_000_000_000;
const DAY: u64 = 86_400;
fn part(text: &[u8], start: usize, length: usize) -> Result<u64, Error> {
    json::decimal(text.get(start..start.checked_add(length).ok_or(Error::TooLarge)?).ok_or(Error::Malformed)?)
}
const fn leap(year: u64) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}
const fn month_days(year: u64, month: u64) -> u64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}
fn before_year(year: u64) -> Result<u64, Error> {
    let prior = year.checked_sub(1).ok_or(Error::Malformed)?;
    prior
        .checked_mul(365)
        .ok_or(Error::TooLarge)?
        .checked_add(prior / 4)
        .ok_or(Error::TooLarge)?
        .checked_sub(prior / 100)
        .ok_or(Error::Malformed)?
        .checked_add(prior / 400)
        .ok_or(Error::TooLarge)
}
pub fn parse(text: &[u8]) -> Result<u64, Error> {
    if text.len() < 20
        || text.get(4) != Some(&b'-')
        || text.get(7) != Some(&b'-')
        || text.get(10) != Some(&b'T')
        || text.get(13) != Some(&b':')
        || text.get(16) != Some(&b':')
    {
        return Err(Error::Malformed);
    }
    let year = part(text, 0, 4)?;
    let month = part(text, 5, 2)?;
    let day = part(text, 8, 2)?;
    let hour = part(text, 11, 2)?;
    let minute = part(text, 14, 2)?;
    let second = part(text, 17, 2)?;
    if year < 1970
        || month == 0
        || month > 12
        || day == 0
        || day > month_days(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(Error::Malformed);
    }
    let mut days = before_year(year)?.checked_sub(before_year(1970)?).ok_or(Error::Malformed)?;
    for prior in 1..month {
        days = days.checked_add(month_days(year, prior)).ok_or(Error::TooLarge)?;
    }
    days = days.checked_add(day.checked_sub(1).expect("day is nonzero")).ok_or(Error::TooLarge)?;
    let mut seconds = days
        .checked_mul(DAY)
        .ok_or(Error::TooLarge)?
        .checked_add(hour.checked_mul(3600).expect("hour bounded"))
        .ok_or(Error::TooLarge)?
        .checked_add(minute.checked_mul(60).expect("minute bounded"))
        .ok_or(Error::TooLarge)?
        .checked_add(second)
        .ok_or(Error::TooLarge)?;
    let mut at: usize = 19;
    let mut fraction: u64 = 0;
    if text.get(at) == Some(&b'.') {
        at = at.checked_add(1).expect("within timestamp");
        let start = at;
        for _ in 0_u32..9 {
            match text.get(at) {
                Some(byte) if byte.is_ascii_digit() => {
                    fraction = fraction
                        .checked_mul(10)
                        .expect("nine decimal digits")
                        .checked_add(u64::from(byte.wrapping_sub(b'0')))
                        .expect("nine digits");
                    at = at.checked_add(1).expect("within timestamp");
                }
                Some(_) | None => break,
            }
        }
        let digits = at.checked_sub(start).expect("cursor advances");
        if digits == 0 {
            return Err(Error::Malformed);
        }
        for _ in digits..9 {
            fraction = fraction.checked_mul(10).expect("nine decimal digits");
        }
    }
    match text.get(at..) {
        Some(b"Z") => {}
        Some([sign, h1, h2, b':', m1, m2]) if *sign == b'+' || *sign == b'-' => {
            let hours = json::decimal(&[*h1, *h2])?;
            let minutes = json::decimal(&[*m1, *m2])?;
            if hours > 23 || minutes > 59 {
                return Err(Error::Malformed);
            }
            let offset = hours
                .checked_mul(3600)
                .expect("offset hours bounded")
                .checked_add(minutes.checked_mul(60).expect("minutes bounded"))
                .expect("offset bounded");
            seconds = if *sign == b'+' {
                seconds.checked_sub(offset).ok_or(Error::Malformed)?
            } else {
                seconds.checked_add(offset).ok_or(Error::TooLarge)?
            };
        }
        Some(_) | None => return Err(Error::Malformed),
    }
    seconds.checked_mul(SECOND).ok_or(Error::TooLarge)?.checked_add(fraction).ok_or(Error::TooLarge)
}
fn fixed(out: &mut Writer, n: u64, width: u32) {
    let mut divisor: u64 = 1;
    for _ in 1..width {
        divisor = divisor.checked_mul(10).expect("four digit fields");
    }
    for _ in 0..width {
        let digit = u8::try_from(n.checked_div(divisor).expect("decimal divisor stays nonzero") % 10)
            .expect("one decimal digit");
        out.put(&[b'0'.wrapping_add(digit)]).expect("timestamp measured");
        divisor /= 10;
    }
}
/// UTC seconds, matching Forgejo's source resolution; subsecond input is floored.
#[must_use]
pub fn format(nanos: u64) -> Box<[u8]> {
    let seconds = nanos / SECOND;
    let mut days = seconds / DAY;
    let mut year: u64 = 1970;
    for _ in 1970_u32..=2554 {
        let span = if leap(year) { 366 } else { 365 };
        if days < span {
            break;
        }
        days = days.checked_sub(span).expect("whole year remains");
        year = year.checked_add(1).expect("epoch nanoseconds bound year");
    }
    let mut month: u64 = 1;
    for _ in 1_u32..=12 {
        let span = month_days(year, month);
        if days < span {
            break;
        }
        days = days.checked_sub(span).expect("whole month remains");
        month = month.checked_add(1).expect("one year");
    }
    let mut out = Writer::new(20);
    fixed(&mut out, year, 4);
    out.put(b"-").expect("measured");
    fixed(&mut out, month, 2);
    out.put(b"-").expect("measured");
    fixed(&mut out, days.checked_add(1).expect("day bounded"), 2);
    out.put(b"T").expect("measured");
    fixed(&mut out, (seconds % DAY) / 3600, 2);
    out.put(b":").expect("measured");
    fixed(&mut out, (seconds % 3600) / 60, 2);
    out.put(b":").expect("measured");
    fixed(&mut out, seconds % 60, 2);
    out.put(b"Z").expect("measured");
    out.finish()
}
