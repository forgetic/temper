use std::ffi::OsString;
use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

pub(super) struct HelperArguments {
    pub(super) control_fd: RawFd,
    pub(super) emergency_fd: RawFd,
    pub(super) term_grace: Duration,
    pub(super) inspection_retry: Duration,
    pub(super) payload_program: OsString,
    pub(super) payload_arguments: Vec<OsString>,
}

pub(super) fn parse(mut arguments: impl Iterator<Item = OsString>) -> io::Result<HelperArguments> {
    let control_fd = parse_field(arguments.next(), "control fd")?;
    let emergency_fd = parse_field(arguments.next(), "emergency fd")?;
    let term_grace_ms = parse_field(arguments.next(), "TERM grace")?;
    let inspection_retry_ms = parse_field(arguments.next(), "inspection retry")?;
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Linux supervisor payload separator is missing",
        ));
    }
    let payload_program = arguments.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Linux supervisor payload program is missing",
        )
    })?;
    Ok(HelperArguments {
        control_fd,
        emergency_fd,
        term_grace: Duration::from_millis(term_grace_ms),
        inspection_retry: Duration::from_millis(inspection_retry_ms),
        payload_program,
        payload_arguments: arguments.collect(),
    })
}

fn parse_field<T: std::str::FromStr>(value: Option<OsString>, name: &str) -> io::Result<T> {
    value
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid Linux supervisor {name}"),
            )
        })
}
