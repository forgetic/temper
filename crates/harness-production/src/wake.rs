//! Host-local wake bus for production workers.

use std::fmt;
use std::path::{Path, PathBuf};

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
}

impl fmt::Display for WakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WakeError::Io(error) => write!(formatter, "wake socket I/O failed: {error}"),
            WakeError::Unsupported => write!(formatter, "wake sockets require a Unix platform"),
            WakeError::Unauthorized => write!(formatter, "wake message authentication failed"),
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
    socket: tokio::net::UnixDatagram,
    path: PathBuf,
    secret: Option<String>,
}

#[cfg(unix)]
impl WakeListener {
    pub fn bind(config: WakeConfig) -> Result<Self, WakeError> {
        if let Some(parent) = config.socket.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let _ = std::fs::remove_file(&config.socket);
        let socket = tokio::net::UnixDatagram::bind(&config.socket)?;
        Ok(Self {
            socket,
            path: config.socket,
            secret: config.secret,
        })
    }

    pub async fn recv(&self) -> Result<(), WakeError> {
        let mut buf = [0_u8; 512];
        let size = self.socket.recv(&mut buf).await?;
        if authorized(&buf[..size], self.secret.as_deref()) {
            Ok(())
        } else {
            Err(WakeError::Unauthorized)
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

    pub async fn recv(&self) -> Result<(), WakeError> {
        Err(WakeError::Unsupported)
    }
}

#[cfg(unix)]
pub fn send_wake(path: &Path, secret: Option<&str>) -> Result<(), WakeError> {
    use std::os::unix::net::UnixDatagram;

    let socket = UnixDatagram::unbound()?;
    let payload = secret.unwrap_or("wake");
    socket.send_to(payload.as_bytes(), path)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn send_wake(_path: &Path, _secret: Option<&str>) -> Result<(), WakeError> {
    Err(WakeError::Unsupported)
}

fn authorized(payload: &[u8], secret: Option<&str>) -> bool {
    match secret {
        Some(secret) => std::str::from_utf8(payload)
            .map(|text| text.trim() == secret)
            .unwrap_or(false),
        None => true,
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
    fn payload_authentication_requires_matching_secret() {
        assert!(authorized(b"wake", None));
        assert!(authorized(b"secret\n", Some("secret")));
        assert!(!authorized(b"wrong", Some("secret")));
    }
}
