// SPDX-License-Identifier: MPL-2.0

//! Deterministic simulation harness: production temper code under skein's
//! lab runtime.
//!
//! The harness runs the real daemon composition — `DaemonMachine` +
//! `DaemonExecutor` + `drive()` + completion queues + timers — under
//! [`LabRuntime`]: seeded deterministic scheduling, a virtual clock that
//! auto-advances to timer deadlines (long-poll waits complete in
//! microseconds of wall time), and optional chaos injection at scheduling
//! points. HTTP traffic runs through the **production h1 connection code**
//! (`Http1Server::serve` / `Http1Client::request`) over in-memory
//! [`VirtualTcpStream`] pairs, so the only production code not exercised is
//! the OS accept loop.
//!
//! Determinism contract: the same seed and scenario produce the identical
//! schedule, trace fingerprint, and observable responses. Failures reproduce
//! from `(seed, scenario)` alone.

pub mod model;
pub mod scenarios;
pub mod worker;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use skein::cx::Cx;
use skein::http::h1::{
    Http1Client, Http1Config, Http1Server, HttpError, Method, Request as H1Request,
    Response as H1Response,
};
use skein::lab::{LabConfig, LabRunReport, LabRuntime, LabSpawner};
use skein::net::tcp::virtual_tcp::VirtualTcpStream;
use temper_io_engine::http::H1CompletionHandler;
use temper_io_engine::Spawner;
use temper_worker_protocol::WorkerProtocolMessage;

/// One deterministic simulation world.
pub struct Sim {
    pub lab: LabRuntime,
}

/// Consecutive scheduler-empty, timer-free rounds tolerated before declaring
/// the simulation stuck. One round is enough to admit queued spawns (the lab
/// drains them at step start); a second confirms nothing became runnable.
const IDLE_ROUNDS_BEFORE_STUCK: u32 = 2;

impl Sim {
    pub fn new(seed: u64) -> Self {
        Self::with_config(LabConfig::new(seed))
    }

    pub fn with_config(config: LabConfig) -> Self {
        Self {
            lab: LabRuntime::new(config),
        }
    }

    /// The lab's spawn handle (queue-admitted, FIFO-deterministic).
    pub fn spawner(&self) -> LabSpawner {
        self.lab.spawner()
    }

    /// The lab's spawn handle as the engine's [`Spawner`] capability — what
    /// `Daemon::new`/`with_applier` and the executors take.
    pub fn engine_spawner(&self) -> Arc<dyn Spawner> {
        Arc::new(self.lab.spawner())
    }

    /// Drive the lab until `done`, stepping runnable tasks and auto-advancing
    /// virtual time to the next timer deadline when idle.
    ///
    /// Panics when `max_steps` is exceeded or when the simulation can make no
    /// further progress (no runnable task, no pending timer, `done` still
    /// false) — a deterministic deadlock detector.
    pub fn run_until(&mut self, mut done: impl FnMut() -> bool, max_steps: u64) {
        let start = self.lab.steps();
        let mut idle_rounds = 0u32;
        while !done() {
            assert!(
                self.lab.steps() - start <= max_steps,
                "simulation exceeded {max_steps} steps without completing (seed-reproducible)"
            );
            // Queued-but-unadmitted spawns count as runnable work: advancing
            // virtual time past them would reorder them after timers.
            let runnable = !self.lab.scheduler.lock().is_empty() || self.lab.has_pending_spawns();
            if runnable {
                idle_rounds = 0;
                self.lab.step_for_test();
                continue;
            }
            if self.lab.next_timer_deadline().is_some() {
                idle_rounds = 0;
                self.lab.advance_to_next_timer();
                continue;
            }
            // Nothing runnable and no timers: one step polls simulated I/O;
            // repeated empty rounds mean a deadlock.
            idle_rounds += 1;
            assert!(
                idle_rounds <= IDLE_ROUNDS_BEFORE_STUCK,
                "simulation is stuck: no runnable tasks, no timers, scenario incomplete \
                 (seed-reproducible deadlock)"
            );
            self.lab.step_for_test();
        }
    }

    /// Spawn `scenario` as a lab task and drive the simulation until it
    /// returns, yielding its result.
    pub fn run_scenario<F, Fut, T>(&mut self, max_steps: u64, scenario: F) -> T
    where
        F: FnOnce(Cx) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let slot: Arc<Mutex<Option<T>>> = Arc::new(Mutex::new(None));
        let result = Arc::clone(&slot);
        self.spawner().spawn_with_cx(move |cx| async move {
            let value = scenario(cx).await;
            *result.lock().expect("scenario result slot") = Some(value);
        });
        self.run_until(
            || slot.lock().expect("scenario result slot").is_some(),
            max_steps,
        );
        let value = slot
            .lock()
            .expect("scenario result slot")
            .take()
            .expect("scenario completed");
        value
    }

    /// Drive the lab until everything parks: no runnable tasks and no pending
    /// timers (held long-polls answered, in-flight applies finished). Unlike
    /// [`Self::run_until`], reaching the parked state is the goal, not a
    /// deadlock. Panics only if `max_steps` is exceeded.
    pub fn settle(&mut self, max_steps: u64) {
        let start = self.lab.steps();
        let mut idle_rounds = 0u32;
        while idle_rounds <= IDLE_ROUNDS_BEFORE_STUCK {
            assert!(
                self.lab.steps() - start <= max_steps,
                "simulation exceeded {max_steps} steps without settling (seed-reproducible)"
            );
            if !self.lab.scheduler.lock().is_empty() || self.lab.has_pending_spawns() {
                idle_rounds = 0;
                self.lab.step_for_test();
                continue;
            }
            if self.lab.next_timer_deadline().is_some() {
                idle_rounds = 0;
                self.lab.advance_to_next_timer();
                continue;
            }
            idle_rounds += 1;
            self.lab.step_for_test();
        }
    }

    /// Structured lab report (trace fingerprint, certificate, oracles) at the
    /// current point — does not advance the simulation.
    pub fn report(&mut self) -> LabRunReport {
        self.lab.report()
    }

    /// A [`temper_daemon::WallClock`] derived from the lab's virtual clock
    /// (Unix epoch + virtual nanoseconds): deterministic, and consistent
    /// with every timer in the simulation.
    pub fn wall_clock(&self) -> temper_daemon::WallClock {
        let driver = self
            .lab
            .state
            .timer_driver_handle()
            .expect("lab runtime has a virtual timer driver");
        Arc::new(move || {
            chrono::DateTime::from_timestamp_nanos(
                i64::try_from(driver.now().as_nanos()).expect("virtual time fits i64"),
            )
        })
    }
}

fn sim_addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Perform one HTTP request through the production h1 byte path: a fresh
/// in-memory connection pair, the real `Http1Server` connection code serving
/// the engine handler on one end, the real `Http1Client` on the other.
pub async fn http_request(
    spawner: &LabSpawner,
    handler: &H1CompletionHandler,
    request: H1Request,
) -> Result<H1Response, HttpError> {
    let (client_io, server_io) = VirtualTcpStream::pair(sim_addr(1), sim_addr(2));
    let connection_handler = Arc::clone(handler);
    let server = Http1Server::with_config(
        move |request| connection_handler(request),
        Http1Config::default(),
    );
    spawner.spawn_with_cx(move |_cx| async move {
        let _ = server.serve(server_io).await;
    });
    Http1Client::request(client_io, request).await
}

/// Worker-protocol client speaking real h1 bytes to a daemon's handler.
#[derive(Clone)]
pub struct SimProtocolClient {
    spawner: LabSpawner,
    handler: H1CompletionHandler,
}

impl SimProtocolClient {
    /// Client against the given daemon (its production request handler).
    pub fn new(spawner: LabSpawner, daemon: &temper_daemon::Daemon) -> Self {
        Self {
            spawner,
            handler: temper_daemon::h1_handler(daemon),
        }
    }

    /// POST one protocol message to `/v1/message`; `Ok(Some)` for a 200 reply
    /// message, `Ok(None)` for 204, `Err` for transport failures or
    /// unexpected statuses (chaos can kill a connection mid-flight — workers
    /// treat that as retryable trouble, not a bug).
    pub async fn try_send(
        &self,
        message: &WorkerProtocolMessage,
    ) -> Result<Option<WorkerProtocolMessage>, String> {
        let body = serde_json::to_vec(message).expect("protocol messages serialize");
        let request = H1Request {
            method: Method::Post,
            uri: "/v1/message".to_string(),
            version: skein::http::h1::Version::Http11,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body,
            trailers: Vec::new(),
            peer_addr: None,
        };
        let response = http_request(&self.spawner, &self.handler, request)
            .await
            .map_err(|error| format!("transport: {error}"))?;
        match response.status {
            200 => Ok(Some(
                serde_json::from_slice(&response.body)
                    .map_err(|error| format!("malformed daemon reply: {error}"))?,
            )),
            204 => Ok(None),
            other => Err(format!(
                "unexpected daemon response status {other}: {}",
                String::from_utf8_lossy(&response.body)
            )),
        }
    }

    /// [`Self::try_send`], panicking on failure — for fault-free scenarios
    /// where a transport error IS a bug.
    pub async fn send(&self, message: &WorkerProtocolMessage) -> Option<WorkerProtocolMessage> {
        self.try_send(message)
            .await
            .expect("sim http request succeeds")
    }
}
