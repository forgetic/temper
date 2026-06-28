// SPDX-License-Identifier: MPL-2.0

//! Local client for the worker-owned `submit_for_pr` side channel.
//!
//! The out-of-process agent receives the address as a non-secret CLI flag. Each
//! tool call opens one loopback TCP connection, writes a JSON
//! [`SubmitForPrRequest`], half-closes the write side, then reads the JSON
//! [`SubmitForPrResponse`]. Transport failures are returned to the model as a
//! host rejection so the live run remains intact.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use temper_agent::SubmitForPrHost;
use temper_protocol_agent::{SubmitForPrRequest, SubmitForPrResponse};

const SUBMIT_IO_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn host_for_address(address: String) -> SubmitForPrHost {
    Arc::new(move |request, _context, _cwd| submit_once(&address, request))
}

fn submit_once(address: &str, request: SubmitForPrRequest) -> SubmitForPrResponse {
    match submit_once_result(address, &request) {
        Ok(response) => response,
        Err(error) => {
            SubmitForPrResponse::rejected(format!("submit_for_pr host channel failed: {error}"))
        }
    }
}

fn submit_once_result(
    address: &str,
    request: &SubmitForPrRequest,
) -> std::io::Result<SubmitForPrResponse> {
    let mut stream = TcpStream::connect(address)?;
    stream.set_read_timeout(Some(SUBMIT_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SUBMIT_IO_TIMEOUT))?;
    let bytes = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    stream.write_all(&bytes)?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    serde_json::from_slice(&response).map_err(std::io::Error::other)
}
