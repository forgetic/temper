// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use super::ChildGuard;

pub(super) fn wait_until_warm(
    standalone_log: &Path,
    standalone: &mut ChildGuard,
    role_passes: usize,
    mechanical_passes: usize,
    timeout: Duration,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let source = fs::read_to_string(standalone_log).unwrap_or_default();
        let (roles, mechanical) = warm_discovery_counts(&source);
        if roles >= role_passes && mechanical >= mechanical_passes {
            return Ok(format!(
                "observed warm discovery reuse role_passes={roles} mechanical_passes={mechanical}"
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "warm discovery reuse did not reach role={role_passes} mechanical={mechanical_passes}; observed role={roles} mechanical={mechanical}"
            ));
        }
        if let Some(status) = standalone.try_wait()? {
            return Err(format!(
                "standalone exited while waiting for warm discovery: {status:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn warm_discovery_counts(source: &str) -> (usize, usize) {
    source
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|record| record.get("fields").cloned())
        .filter(|fields| {
            fields
                .get("measurement")
                .and_then(serde_json::Value::as_str)
                == Some("candidate.discovery")
                && fields
                    .get("candidate.discovery_complete")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                && fields
                    .get("candidate.discovery_cache_reused")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
        })
        .fold((0, 0), |(roles, mechanical), fields| {
            match fields
                .get("candidate.consumer")
                .and_then(serde_json::Value::as_str)
            {
                Some("role") => (roles + 1, mechanical),
                Some("mechanical") => (roles, mechanical + 1),
                _ => (roles, mechanical),
            }
        })
}
