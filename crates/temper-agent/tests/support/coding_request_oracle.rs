//! Local HTTP request capture with Jig-rendered responses, including headers
//! that `FakeLlm::requests()` does not expose.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use jig_core::render::frames_to_body;
use jig_core::{Dialect, Reply, StopReason, Turn, render_anthropic, render_codex, render_openai};
use serde_json::Value;

#[derive(Clone)]
pub struct Request {
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

pub struct CodingRequestOracle {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
    thread: Option<JoinHandle<()>>,
}

impl CodingRequestOracle {
    pub fn start(dialect: Dialect) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local request oracle");
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let thread = std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let Some(request) = read_request(&stream) else {
                    break; // Drop wakes accept with an empty connection.
                };
                let turn = {
                    let mut requests = captured.lock().unwrap();
                    let turn = requests.len() % 2;
                    requests.push(request);
                    turn
                };
                let reply = coding_reply(turn);
                let frames = match dialect {
                    Dialect::Codex => render_codex(&reply),
                    Dialect::Anthropic => render_anthropic(&reply),
                    Dialect::OpenAi => render_openai(&reply),
                };
                let body = frames_to_body(&frames);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(body.as_bytes()).unwrap();
            }
        });
        Self {
            address,
            requests,
            thread: Some(thread),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for CodingRequestOracle {
    fn drop(&mut self) {
        drop(TcpStream::connect(self.address));
        self.thread
            .take()
            .unwrap()
            .join()
            .expect("request oracle joins");
    }
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).expect("read request line") == 0 {
        return None;
    }
    let mut headers = BTreeMap::new();
    loop {
        line.clear();
        assert_ne!(reader.read_line(&mut line).expect("read header"), 0);
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':').expect("HTTP header");
        headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
    }
    let length: usize = headers["content-length"].parse().expect("content length");
    assert!(length < 2 * 1024 * 1024, "bounded fixture request");
    let mut body = vec![0; length];
    reader.read_exact(&mut body).expect("complete request body");
    Some(Request {
        headers,
        body: serde_json::from_slice(&body).expect("JSON request"),
    })
}

fn coding_reply(turn: usize) -> Reply {
    if turn == 0 {
        Reply {
            turns: vec![Turn::ToolCall {
                id: "call_write_notes".to_string(),
                name: "write".to_string(),
                args: serde_json::json!({"path": "demo/NOTES.md", "content": "project notes\n"}),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        }
    } else {
        Reply::text(r#"{"summary":"Created NOTES.md with project notes."}"#)
    }
}
