//! Host-local wake bus for production workers.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
use temper_runner::ChangeHint;

#[derive(Clone, Eq, PartialEq)]
pub struct WakeConfig {
    pub socket: PathBuf,
    pub secret: Option<String>,
}

impl fmt::Debug for WakeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WakeConfig")
            .field("socket", &self.socket)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl WakeConfig {
    pub fn from_files(socket: PathBuf, secret_file: Option<PathBuf>) -> Result<Self, WakeError> {
        let secret = secret_file
            .map(|path| read_secret(&path))
            .transpose()?
            .filter(|secret| !secret.is_empty());
        Ok(Self { socket, secret })
    }
}

#[derive(Debug)]
pub enum WakeError {
    Io(std::io::Error),
    Unsupported,
    Unauthorized,
    InvalidPayload(String),
}

impl fmt::Display for WakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WakeError::Io(error) => write!(formatter, "wake socket I/O failed: {error}"),
            WakeError::Unsupported => write!(formatter, "wake sockets require a Unix platform"),
            WakeError::Unauthorized => write!(formatter, "wake message authentication failed"),
            WakeError::InvalidPayload(error) => {
                write!(formatter, "wake payload was invalid: {error}")
            }
        }
    }
}

impl std::error::Error for WakeError {}

impl From<std::io::Error> for WakeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn read_secret(path: &Path) -> Result<String, WakeError> {
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

#[cfg(unix)]
pub struct WakeListener {
    socket: skein::net::unix::UnixDatagram,
    path: PathBuf,
    secret: Option<String>,
}

#[cfg(unix)]
impl WakeListener {
    pub fn bind(config: WakeConfig) -> Result<Self, WakeError> {
        if let Some(parent) = config.socket.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&config.socket);
        let socket = skein::net::unix::UnixDatagram::bind(&config.socket)?;
        Ok(Self {
            socket,
            path: config.socket,
            secret: config.secret,
        })
    }

    pub async fn recv(&mut self) -> Result<Option<ChangeHint>, WakeError> {
        let mut buf = [0_u8; 2048];
        let size = self.socket.recv(&mut buf).await?;
        decode_payload(&buf[..size], self.secret.as_deref())
    }

    pub fn try_recv(&self) -> Result<Option<Option<ChangeHint>>, WakeError> {
        let mut buf = [0_u8; 2048];
        // The engine's datagram socket is non-blocking; a raw std recv gives
        // the same try semantics the previous runtime offered.
        match self.socket.as_std().recv(&mut buf) {
            Ok(size) => decode_payload(&buf[..size], self.secret.as_deref()).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(WakeError::Io(error)),
        }
    }
}

#[cfg(unix)]
impl Drop for WakeListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(not(unix))]
pub struct WakeListener;

#[cfg(not(unix))]
impl WakeListener {
    pub fn bind(_config: WakeConfig) -> Result<Self, WakeError> {
        Err(WakeError::Unsupported)
    }

    pub async fn recv(&mut self) -> Result<Option<ChangeHint>, WakeError> {
        Err(WakeError::Unsupported)
    }

    pub fn try_recv(&self) -> Result<Option<Option<ChangeHint>>, WakeError> {
        Err(WakeError::Unsupported)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WakeWaitOutcome {
    PollDeadline,
    Stop,
    Wake(Vec<ChangeHint>),
}

const MAX_WAKE_DRAIN: usize = 1024;
const DEFAULT_WAKE_DEBOUNCE: StdDuration = StdDuration::from_millis(500);
const WAKE_DEBOUNCE_MS_ENV: &str = "TEMPER_WAKE_DEBOUNCE_MS";
const STOP_CHECK_INTERVAL: StdDuration = StdDuration::from_millis(250);

fn wake_debounce() -> StdDuration {
    wake_debounce_from(std::env::var(WAKE_DEBOUNCE_MS_ENV).ok().as_deref())
}

fn wake_debounce_from(raw: Option<&str>) -> StdDuration {
    raw.and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|milliseconds| *milliseconds > 0)
        .map(StdDuration::from_millis)
        .unwrap_or(DEFAULT_WAKE_DEBOUNCE)
}

fn drain_wake_batch(
    listener: &WakeListener,
    first: Option<ChangeHint>,
) -> Result<Vec<ChangeHint>, WakeError> {
    let mut broad = first.is_none();
    let mut hints = first.into_iter().collect::<Vec<_>>();
    let mut drained = 0usize;
    loop {
        if drained >= MAX_WAKE_DRAIN {
            eprintln!(
                concat!(
                    "temper-wake: wake drain hit cap ({}); ",
                    "remaining queued wakes will form a later batch"
                ),
                MAX_WAKE_DRAIN
            );
            break;
        }
        match listener.try_recv() {
            Ok(Some(Some(hint))) => {
                hints.push(hint);
                drained += 1;
            }
            Ok(Some(None)) => {
                broad = true;
                drained += 1;
            }
            Ok(None) => break,
            Err(WakeError::Unauthorized) => {
                eprintln!("temper-wake: ignored unauthorized wake message");
                drained += 1;
            }
            Err(error) => return Err(error),
        }
    }
    if broad { Ok(Vec::new()) } else { Ok(hints) }
}

pub async fn wait_for_wake_or_poll(
    cx: &skein::cx::Cx,
    mut should_stop: impl FnMut() -> bool,
    interval: StdDuration,
    mut wake: Option<&mut WakeListener>,
) -> Result<WakeWaitOutcome, WakeError> {
    // Imperative-shell wait: slice the poll interval into bounded waits so the
    // stop flag is observed at least every STOP_CHECK_INTERVAL, racing the
    // datagram recv against each slice's deadline. The decisions (payload
    // auth/decode, debounce policy, batch draining) stay pure functions.
    let deadline = std::time::Instant::now() + interval;
    loop {
        if should_stop() {
            return Ok(WakeWaitOutcome::Stop);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(WakeWaitOutcome::PollDeadline);
        }
        let window = remaining.min(STOP_CHECK_INTERVAL);
        match wake.as_deref_mut() {
            Some(listener) => {
                let received = skein::time::timeout(
                    temper_engine_io::runtime::timer_now(cx),
                    window,
                    Box::pin(listener.recv()),
                )
                .await;
                match received {
                    Err(_elapsed) => {}
                    Ok(Ok(hint)) => {
                        skein::time::sleep(
                            temper_engine_io::runtime::timer_now(cx),
                            wake_debounce(),
                        )
                        .await;
                        return drain_wake_batch(listener, hint).map(WakeWaitOutcome::Wake);
                    }
                    Ok(Err(WakeError::Unauthorized)) => {
                        eprintln!("temper-wake: ignored unauthorized wake message");
                    }
                    Ok(Err(error)) => return Err(error),
                }
            }
            None => {
                skein::time::sleep(temper_engine_io::runtime::timer_now(cx), window).await;
            }
        }
    }
}

#[cfg(unix)]
pub fn send_wake(path: &Path, secret: Option<&str>) -> Result<(), WakeError> {
    use std::os::unix::net::UnixDatagram;

    let socket = UnixDatagram::unbound()?;
    let payload = encode_payload(secret, None)?;
    socket.send_to(&payload, path)?;
    Ok(())
}

#[cfg(unix)]
pub fn send_wake_with_hint(
    path: &Path,
    secret: Option<&str>,
    hint: &ChangeHint,
) -> Result<(), WakeError> {
    use std::os::unix::net::UnixDatagram;

    let socket = UnixDatagram::unbound()?;
    let payload = encode_payload(secret, Some(hint))?;
    socket.send_to(&payload, path)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn send_wake(_path: &Path, _secret: Option<&str>) -> Result<(), WakeError> {
    Err(WakeError::Unsupported)
}

#[cfg(not(unix))]
pub fn send_wake_with_hint(
    _path: &Path,
    _secret: Option<&str>,
    _hint: &ChangeHint,
) -> Result<(), WakeError> {
    Err(WakeError::Unsupported)
}

fn encode_payload(secret: Option<&str>, hint: Option<&ChangeHint>) -> Result<Vec<u8>, WakeError> {
    let mut payload = secret.unwrap_or("wake").as_bytes().to_vec();
    if let Some(hint) = hint {
        payload.push(b'\n');
        payload.extend_from_slice(
            serde_json::to_string(hint)
                .map_err(|error| WakeError::InvalidPayload(error.to_string()))?
                .as_bytes(),
        );
    }
    Ok(payload)
}

fn decode_payload(payload: &[u8], secret: Option<&str>) -> Result<Option<ChangeHint>, WakeError> {
    if !authorized(payload, secret) {
        return Err(WakeError::Unauthorized);
    }
    let Some(newline) = payload.iter().position(|byte| *byte == b'\n') else {
        return Ok(None);
    };
    let hint = &payload[newline + 1..];
    if hint.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(hint)
        .map(Some)
        .map_err(|error| WakeError::InvalidPayload(error.to_string()))
}

fn authorized(payload: &[u8], secret: Option<&str>) -> bool {
    let marker = payload
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or(payload);
    match secret {
        Some(secret) => std::str::from_utf8(marker)
            .map(|text| text.trim() == secret)
            .unwrap_or(false),
        None => marker == b"wake",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_config_debug_redacts_secret() {
        let config = WakeConfig {
            socket: PathBuf::from("worker.sock"),
            secret: Some("local-secret".into()),
        };
        assert!(!format!("{config:?}").contains("local-secret"));
        assert!(format!("{config:?}").contains("<redacted>"));
    }

    #[test]
    fn wake_debounce_uses_positive_millisecond_override() {
        assert_eq!(wake_debounce_from(None), DEFAULT_WAKE_DEBOUNCE);
        assert_eq!(wake_debounce_from(Some("75")), StdDuration::from_millis(75));
        assert_eq!(wake_debounce_from(Some("0")), DEFAULT_WAKE_DEBOUNCE);
        assert_eq!(
            wake_debounce_from(Some("not-a-number")),
            DEFAULT_WAKE_DEBOUNCE
        );
    }

    #[test]
    fn payload_authentication_requires_matching_secret() {
        assert!(authorized(b"wake", None));
        assert!(authorized(b"secret\n", Some("secret")));
        assert!(!authorized(b"wrong", Some("secret")));
        assert!(!authorized(b"not-wake", None));
    }

    #[test]
    fn payload_round_trips_repo_hint() {
        let hint = ChangeHint::repo(
            temper_forge_model::RepositoryPath::new("acme", "service"),
            temper_runner::ChangeKind::Issue,
        );
        let payload = encode_payload(Some("wake-secret"), Some(&hint)).expect("payload encodes");
        assert_eq!(
            decode_payload(&payload, Some("wake-secret")).expect("payload decodes"),
            Some(hint)
        );
    }
}
