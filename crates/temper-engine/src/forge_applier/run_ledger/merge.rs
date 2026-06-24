// SPDX-License-Identifier: MPL-2.0

use std::error::Error;
use std::fmt;

use temper_workflow::{METADATA_BEGIN, METADATA_END};

use super::one_line_or;

pub(super) const LEDGER_END: &str = "<!-- /temper-run-ledger -->";

pub(super) fn ledger_marker(correlation_key: &str) -> String {
    format!(
        "<!-- temper-run-ledger correlation_key={} -->",
        one_line_or(correlation_key, "unknown")
    )
}

pub(super) fn run_ledger_finalized(body: &str, correlation_key: &str) -> bool {
    match run_ledger_span(body, correlation_key) {
        Ok(Some((start, end))) => ledger_block_finalized(&body[start..end]),
        Ok(None) | Err(_) => false,
    }
}

pub(super) fn ledger_block_finalized(block: &str) -> bool {
    block.contains("Current status: continued in ")
}

pub(super) fn ledger_worker_matches(block: &str, worker_id: &str) -> bool {
    let expected = format!("- Worker: `{}`", one_line_or(worker_id, "unknown"));
    block.lines().any(|line| line.trim() == expected)
}

pub(super) fn retryable_ledger_block(block: &str) -> Option<String> {
    const RETRY_STATUS: &str = "- Current status: queued for retry";
    const RETRY_NOTE: &str = "- Retry: released back to the ready queue after a transient failure; checkpointed branch metadata is preserved.";

    let mut lines = block.lines().map(str::to_string).collect::<Vec<_>>();
    let mut changed = false;

    let status_index = match lines
        .iter()
        .position(|line| line.trim_start().starts_with("- Current status:"))
    {
        Some(index) => index,
        None => lines
            .iter()
            .position(|line| line.trim() == LEDGER_END)
            .unwrap_or(lines.len()),
    };

    if lines
        .get(status_index)
        .is_some_and(|line| line.trim_start().starts_with("- Current status:"))
    {
        if lines[status_index] != RETRY_STATUS {
            lines[status_index] = RETRY_STATUS.to_string();
            changed = true;
        }
    } else {
        lines.insert(status_index, RETRY_STATUS.to_string());
        changed = true;
    }

    if !lines.iter().any(|line| line == RETRY_NOTE) {
        lines.insert(status_index + 1, RETRY_NOTE.to_string());
        changed = true;
    }

    if changed {
        Some(lines.join("\n"))
    } else {
        None
    }
}

pub(super) fn upsert_run_ledger_block(
    body: &str,
    correlation_key: &str,
    block: &str,
    skip_if_finalized: bool,
) -> Result<Option<String>, RunLedgerMergeError> {
    if let Some((start, end)) = run_ledger_span(body, correlation_key)? {
        let current = &body[start..end];
        if skip_if_finalized && ledger_block_finalized(current) {
            return Ok(None);
        }
        if current == block {
            return Ok(None);
        }
        let updated = format!("{}{}{}", &body[..start], block, &body[end..]);
        return if updated == body {
            Ok(None)
        } else {
            Ok(Some(updated))
        };
    }

    insert_run_ledger_block(body, block).map(Some)
}

pub(super) fn run_ledger_span(
    body: &str,
    correlation_key: &str,
) -> Result<Option<(usize, usize)>, RunLedgerMergeError> {
    let marker = ledger_marker(correlation_key);
    let Some(start) = body.find(&marker) else {
        return Ok(None);
    };
    let after_marker = start + marker.len();
    let Some(end_relative) = body[after_marker..].find(LEDGER_END) else {
        return Err(RunLedgerMergeError::UnterminatedLedger);
    };
    let end = after_marker + end_relative + LEDGER_END.len();
    Ok(Some((start, end)))
}

fn insert_run_ledger_block(body: &str, block: &str) -> Result<String, RunLedgerMergeError> {
    if let Some(index) = workflow_metadata_start(body)? {
        let before = body[..index].trim_end();
        let after = body[index..].trim_start_matches('\n');
        return if before.is_empty() {
            Ok(format!("{block}\n\n{after}"))
        } else {
            Ok(format!("{before}\n\n{block}\n\n{after}"))
        };
    }

    if body.trim().is_empty() {
        Ok(block.to_string())
    } else {
        Ok(format!("{}\n\n{block}", body.trim_end()))
    }
}

fn workflow_metadata_start(body: &str) -> Result<Option<usize>, RunLedgerMergeError> {
    let Some(start) = body.find(METADATA_BEGIN) else {
        return Ok(None);
    };
    let after_begin = start + METADATA_BEGIN.len();
    if !body[after_begin..].contains(METADATA_END) {
        return Err(RunLedgerMergeError::UnterminatedWorkflowMetadata);
    }
    Ok(Some(start))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RunLedgerMergeError {
    UnterminatedLedger,
    UnterminatedWorkflowMetadata,
}

impl fmt::Display for RunLedgerMergeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedLedger => formatter.write_str("run ledger block was not terminated"),
            Self::UnterminatedWorkflowMetadata => {
                formatter.write_str("workflow metadata block was not terminated")
            }
        }
    }
}

impl Error for RunLedgerMergeError {}
