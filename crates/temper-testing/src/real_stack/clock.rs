use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use temper_engine::WallClock;

/// Cloneable wall clock controlled explicitly by restart tests.
///
/// Daemon replacements receive the same closure, so lease time never jumps to
/// ambient process time when a component is recreated.
#[derive(Clone, Debug)]
pub struct MutableWallClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl MutableWallClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    pub fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("mutable wall clock lock")
    }

    pub fn set(&self, now: DateTime<Utc>) {
        *self.now.lock().expect("mutable wall clock lock") = now;
    }

    pub fn advance(&self, duration: Duration) -> DateTime<Utc> {
        let mut now = self.now.lock().expect("mutable wall clock lock");
        *now += duration;
        *now
    }

    pub(crate) fn capability(&self) -> WallClock {
        let clock = self.clone();
        Arc::new(move || clock.now())
    }
}
