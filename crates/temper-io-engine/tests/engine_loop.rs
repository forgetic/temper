// SPDX-License-Identifier: MPL-2.0

//! End-to-end exercise of the completion engine: a pure machine served over
//! real HTTP with a real timer, plus a pure-machine unit test that needs no
//! runtime at all.

use std::time::Duration;

use temper_io_engine::http::{
    serve_http, HttpCall, HttpRequestData, HttpResponder, HttpResponseData,
};
use temper_io_engine::{arm_timer, channel, drive, CqSender, EngineTime, Executor, Machine};

/// Completions the demo service can observe.
enum Completion {
    HttpInbound(HttpRequestData, HttpResponder),
    TimerFired { token: u64 },
    Shutdown,
}

/// Requests the demo service can issue.
enum Request {
    Respond(HttpResponder, HttpResponseData),
    StartTimer { token: u64, delay: Duration },
}

/// Pure logic: answer `POST /ping` with a counter of timer ticks seen so far,
/// re-arming a tick timer each time one fires.
#[derive(Default)]
struct PingMachine {
    ticks: u64,
    stopped: bool,
}

impl Machine for PingMachine {
    type Completion = Completion;
    type Request = Request;

    fn on_start(&mut self, _now: EngineTime) -> Vec<Request> {
        vec![Request::StartTimer {
            token: 0,
            delay: Duration::from_millis(10),
        }]
    }

    fn on_completion(&mut self, _now: EngineTime, completion: Completion) -> Vec<Request> {
        match completion {
            Completion::HttpInbound(request, responder) => {
                let response = if request.method == "POST" && request.uri == "/ping" {
                    HttpResponseData {
                        status: 200,
                        headers: vec![("content-type".into(), "text/plain".into())],
                        body: format!("ticks={}", self.ticks).into_bytes(),
                    }
                } else {
                    HttpResponseData::status_only(404)
                };
                vec![Request::Respond(responder, response)]
            }
            Completion::TimerFired { token } => {
                self.ticks += 1;
                vec![Request::StartTimer {
                    token: token + 1,
                    delay: Duration::from_millis(10),
                }]
            }
            Completion::Shutdown => {
                self.stopped = true;
                Vec::new()
            }
        }
    }

    fn is_stopped(&self) -> bool {
        self.stopped
    }
}

#[test]
fn machine_is_pure_and_deterministic() {
    // No runtime, no sockets, no sleeps: completions in, requests out.
    let mut machine = PingMachine::default();
    let t = EngineTime::from_nanos(7);
    let started = machine.on_start(t);
    assert!(matches!(
        started[..],
        [Request::StartTimer { token: 0, .. }]
    ));

    let requests = machine.on_completion(t, Completion::TimerFired { token: 0 });
    assert!(matches!(
        requests[..],
        [Request::StartTimer { token: 1, .. }]
    ));

    let (probe_tx, _probe_rx) = temper_io_engine::oneshot();
    let requests = machine.on_completion(
        t,
        Completion::HttpInbound(
            HttpRequestData {
                method: "POST".into(),
                uri: "/ping".into(),
                headers: Vec::new(),
                body: Vec::new(),
            },
            // A responder is just a capability token to the machine; it never
            // looks inside.
            HttpResponder::from_oneshot(probe_tx),
        ),
    );
    match &requests[..] {
        [Request::Respond(_, response)] => {
            assert_eq!(response.status, 200);
            assert_eq!(response.body, b"ticks=1");
        }
        _ => panic!("expected respond request"),
    }

    assert!(!machine.is_stopped());
    let requests = machine.on_completion(t, Completion::Shutdown);
    assert!(requests.is_empty());
    assert!(machine.is_stopped());
}

/// Shell: executes requests on the skein runtime.
struct PingExecutor {
    handle: skein::runtime::RuntimeHandle,
    cq: CqSender<Completion>,
}

impl Executor<PingMachine> for PingExecutor {
    fn execute(&self, request: Request) {
        match request {
            Request::Respond(responder, response) => responder.respond(response),
            Request::StartTimer { token, delay } => {
                arm_timer(&self.handle, &self.cq, delay, move || {
                    Completion::TimerFired { token }
                });
            }
        }
    }
}

#[test]
fn engine_serves_http_with_timers() {
    let runtime = temper_io_engine::build_runtime().expect("runtime");
    let handle = runtime.handle();
    let (done_tx, done_rx) = temper_io_engine::oneshot::<(u16, Vec<u8>)>();

    // All async work runs inside a spawned task so it has an ambient Cx; the
    // main future only waits for the result.
    let task_handle = handle.clone();
    handle.spawn_with_cx(move |cx| async move {
        let handle = task_handle;
        let (cq_tx, cq_rx) = channel::<Completion>();
        let server = serve_http(
            &handle,
            "127.0.0.1:0".parse().expect("addr"),
            cq_tx.clone(),
            Completion::HttpInbound,
        )
        .await
        .expect("bind engine http server");
        let addr = server.local_addr();

        let executor = PingExecutor {
            handle: handle.clone(),
            cq: cq_tx.clone(),
        };
        let drive_cx = cx.clone();
        let driver = handle
            .spawn(async move { drive(drive_cx, PingMachine::default(), &executor, cq_rx).await });

        // Give a few timer ticks a chance to land, then call the service.
        skein::time::sleep(
            temper_io_engine::runtime::timer_now(&cx),
            Duration::from_millis(50),
        )
        .await;

        let client = temper_io_engine::http::build_http_client();
        let response = temper_io_engine::http::http_call(
            &client,
            HttpCall {
                method: "POST".into(),
                url: format!("http://{addr}/ping"),
                headers: Vec::new(),
                body: Vec::new(),
            },
        )
        .await
        .expect("http call");

        // Shut the machine down and wait for the engine loop to exit.
        let _ = cq_tx.send(Completion::Shutdown);
        let machine = driver.await;
        assert!(machine.is_stopped());
        server.begin_drain(Duration::from_millis(200));

        done_tx.send((response.status, response.body));
    });

    let result = runtime
        .block_on(done_rx.recv())
        .expect("engine task finished");
    assert_eq!(result.0, 200);
    let body = String::from_utf8(result.1).expect("utf8 body");
    assert!(body.starts_with("ticks="), "unexpected body {body}");
    let ticks: u64 = body["ticks=".len()..].parse().expect("tick count");
    assert!(ticks >= 1, "expected at least one timer tick, got {ticks}");
}
