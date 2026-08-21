use std::time::Duration;

use super::{ActionsHistorySeedFixture, ManifestAction, bounded_integer, required_table_string};

const TRANSPORT_CAP_BYTES: u64 = 16 * 1024 * 1024;
const MIN_SEEDED_RUNS: u64 = 51;
const MAX_SEEDED_RUNS: u64 = 256;
const MIN_PAYLOAD_BYTES: u64 = 64 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 96 * 1024;
const MAX_TIMEOUT_MS: u64 = 180_000;

pub(super) fn parse(table: &toml::Table, index: usize) -> Result<ManifestAction, String> {
    let field = format!("steps[{index}]");
    let seeded_runs = bounded_integer(
        table,
        "seeded_runs",
        &field,
        201,
        MIN_SEEDED_RUNS,
        MAX_SEEDED_RUNS,
    )?;
    let payload_bytes = bounded_integer(
        table,
        "payload_bytes",
        &field,
        90_000,
        MIN_PAYLOAD_BYTES,
        MAX_PAYLOAD_BYTES,
    )?;
    let timeout_ms = bounded_integer(table, "timeout_ms", &field, 120_000, 1, MAX_TIMEOUT_MS)?;
    let lower_bound = seeded_runs
        .checked_mul(payload_bytes)
        .ok_or_else(|| format!("{field} oversized Actions fixture byte lower bound overflows"))?;
    if lower_bound <= TRANSPORT_CAP_BYTES {
        return Err(format!(
            "{field}.seeded_runs multiplied by {field}.payload_bytes must exceed the {TRANSPORT_CAP_BYTES}-byte HTTP transport cap"
        ));
    }
    Ok(ManifestAction::SeedActionsHistory {
        fixture: ActionsHistorySeedFixture {
            repo_id: required_table_string(table, "repo", &field)?,
            source_issue_id: required_table_string(table, "source_issue_id", &field)?,
            seeded_runs: usize::try_from(seeded_runs).expect("bounded run count fits usize"),
            payload_bytes: usize::try_from(payload_bytes).expect("bounded payload fits usize"),
            timeout: Duration::from_millis(timeout_ms),
        },
    })
}
