//! Concurrent HTTP gates in front of Jig's sequential request server.
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::Control;

pub(in crate::live_manifest) struct JigRouter {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    control: Arc<Control>,
}

impl JigRouter {
    pub(super) fn start(upstream: &str, control: Arc<Control>) -> Result<Self, String> {
        let upstream: SocketAddr = upstream
            .strip_prefix("http://")
            .ok_or("Jig must be local HTTP")?
            .parse()
            .map_err(|_| "invalid Jig address")?;
        if !upstream.ip().is_loopback() {
            return Err("Jig must be loopback".into());
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let gates = Arc::clone(&control);
        let thread = thread::spawn(move || {
            let mut handlers: Vec<JoinHandle<()>> = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if handlers.len() >= 32 {
                            gates.fail("too many concurrent Jig connections".into());
                            break;
                        }
                        let gates = Arc::clone(&gates);
                        handlers.push(thread::spawn(move || {
                            if let Err(error) = forward(stream, upstream, &gates) {
                                gates.fail(error);
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => {
                        gates.fail("Jig gate accept failed".into());
                        break;
                    }
                }
                let mut index = 0;
                while index < handlers.len() {
                    if handlers[index].is_finished() {
                        let _ = handlers.swap_remove(index).join();
                    } else {
                        index += 1;
                    }
                }
            }
            for handler in handlers {
                let _ = handler.join();
            }
        });
        Ok(Self {
            address,
            stop,
            thread: Some(thread),
            control,
        })
    }

    pub(in crate::live_manifest) fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }
}

impl Drop for JigRouter {
    fn drop(&mut self) {
        self.control.release(true);
        self.control.release(false);
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn forward(mut client: TcpStream, upstream: SocketAddr, control: &Control) -> Result<(), String> {
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    client
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let (request, header_end) = read_request(&mut client)?;
    let body: serde_json::Value =
        serde_json::from_slice(&request[header_end..]).map_err(|_| "malformed Jig gate request")?;
    let messages = body["messages"].as_array().ok_or("missing Jig messages")?;
    let first = !messages.iter().any(|message| message["role"] == "tool");
    if first {
        let a = messages.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains("LIFECYCLE_JOB_A"))
        });
        if !a
            && !messages.iter().any(|message| {
                message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("LIFECYCLE_JOB_B"))
            })
        {
            return Err("missing gated issue identity".into());
        }
        if !control.gate(a) {
            return Err("Jig HTTP gate expired".into());
        }
    }
    // Forward even if A's client disconnected: Jig must still create its late
    // response. A closed socket cannot restore cancelled worker authority.
    let mut server = TcpStream::connect_timeout(&upstream, Duration::from_secs(5))
        .map_err(|_| "Jig upstream connect failed")?;
    server
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    server
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    server
        .write_all(&request)
        .map_err(|_| "Jig upstream write failed")?;
    let mut response = Vec::new();
    server
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut response)
        .map_err(|_| "Jig upstream response failed")?;
    if response.len() > 2 * 1024 * 1024 {
        return Err("oversized Jig response".into());
    }
    let _ = client.write_all(&response);
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> Result<(Vec<u8>, usize), String> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        if bytes.len() > 1024 * 1024 {
            return Err("oversized Jig request".into());
        }
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = end + 4;
            let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "invalid HTTP header")?;
            let length: usize = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then_some(value.trim())
                })
                .ok_or("Jig gate requires Content-Length")?
                .parse()
                .map_err(|_| "invalid request length")?;
            if length > 1024 * 1024 || header_end + length > 1024 * 1024 {
                return Err("oversized Jig request".into());
            }
            if bytes.len() == header_end + length {
                return Ok((bytes, header_end));
            }
            if bytes.len() > header_end + length {
                return Err("unexpected HTTP pipeline".into());
            }
        }
        let count = stream
            .read(&mut chunk)
            .map_err(|_| "Jig request read failed")?;
        if count == 0 {
            return Err("incomplete Jig request".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}
