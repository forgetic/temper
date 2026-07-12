//! Async agent-side client for the worker-owned Forge read side channel.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use temper_agent::ForgeContextHost;
use temper_protocol_agent::{
    ForgeContextErrorCode, ForgeContextOperation, ForgeContextRequest, ForgeContextResponse,
    ForgeContextToolOutcome, PROTOCOL_VERSION,
};

const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

pub(crate) fn host_for_address(address: String) -> ForgeContextHost {
    Arc::new(move |operation| {
        let address = address.clone();
        Box::pin(async move {
            skein::runtime::spawn_blocking(move || fetch_once(&address, operation)).await
        })
    })
}

fn fetch_once(
    address: &str,
    operation: ForgeContextOperation,
) -> Result<temper_protocol_agent::ForgeContextResult, ForgeContextErrorCode> {
    let request = ForgeContextRequest {
        protocol_version: PROTOCOL_VERSION,
        operation,
    };
    let response =
        fetch_once_io(address, &request).map_err(|_| ForgeContextErrorCode::ForgeUnavailable)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(ForgeContextErrorCode::InvalidRequest);
    }
    match response.outcome {
        ForgeContextToolOutcome::Success { result } => Ok(result),
        ForgeContextToolOutcome::Error { code } => Err(code),
    }
}

fn fetch_once_io(
    address: &str,
    request: &ForgeContextRequest,
) -> std::io::Result<ForgeContextResponse> {
    let mut stream = connect(address)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    let bytes = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    if bytes.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Forge request exceeds hard limit",
        ));
    }
    stream.write_all(&bytes)?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = Vec::new();
    stream
        .take(MAX_MESSAGE_BYTES + 1)
        .read_to_end(&mut response)?;
    if response.len() as u64 > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Forge response exceeds hard limit",
        ));
    }
    serde_json::from_slice(&response).map_err(std::io::Error::other)
}

fn connect(address: &str) -> std::io::Result<TcpStream> {
    let mut last_error = None;
    for socket_address in address.to_socket_addrs()? {
        match TcpStream::connect_timeout(&socket_address, IO_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Forge host address resolved to no socket addresses",
        )
    }))
}
