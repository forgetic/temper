// SPDX-License-Identifier: MPL-2.0

//! Step-progress emission: the framed JSON-lines stream the worker parses off
//! the agent's stdout.

use std::io::Write;

use temper_protocol_agent::StepProgress;

/// Writes one step-progress record as a single JSON line to stdout and flushes,
/// so the worker sees markers live rather than buffered to process exit.
pub(crate) fn emit(progress: &StepProgress) {
    match progress.to_line() {
        Ok(line) => {
            let mut stdout = std::io::stdout().lock();
            // A failed write to the worker's pipe must not abort the run; the
            // run's product (the result file + pushed commits) is what matters.
            let _ = writeln!(stdout, "{line}");
            let _ = stdout.flush();
        }
        Err(error) => {
            tracing::warn!(target: "temper::agent", %error, "serialize step-progress")
        }
    }
}
