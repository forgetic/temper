#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::trace) struct TraceSpoolOperationCounts {
    pub(in crate::trace) event_payload_bytes_read: u64,
    pub(in crate::trace) blob_payload_bytes_read: u64,
    pub(in crate::trace) truncations: u64,
    pub(in crate::trace) deletions: u64,
    pub(in crate::trace) permission_changes: u64,
    pub(in crate::trace) file_syncs: u64,
    pub(in crate::trace) directory_syncs: u64,
}

#[cfg(test)]
thread_local! {
    // Rust's test harness gives each test its own thread. Keeping accounting
    // thread-local lets independently running trace tests reset and inspect
    // operations without racing one another.
    static COUNTS: Cell<TraceSpoolOperationCounts> = const { Cell::new(TraceSpoolOperationCounts {
        event_payload_bytes_read: 0,
        blob_payload_bytes_read: 0,
        truncations: 0,
        deletions: 0,
        permission_changes: 0,
        file_syncs: 0,
        directory_syncs: 0,
    }) };
}

#[cfg(test)]
fn update(operation: impl FnOnce(&mut TraceSpoolOperationCounts)) {
    COUNTS.with(|counts| {
        let mut updated = counts.get();
        operation(&mut updated);
        counts.set(updated);
    });
}

#[cfg(test)]
pub(in crate::trace) fn reset_spool_operation_counts() {
    COUNTS.with(|counts| counts.set(TraceSpoolOperationCounts::default()));
}

#[cfg(test)]
pub(in crate::trace) fn spool_operation_counts() -> TraceSpoolOperationCounts {
    COUNTS.with(Cell::get)
}

#[cfg(test)]
pub(super) fn record_event_payload_bytes_read(bytes: usize) {
    update(|counts| {
        counts.event_payload_bytes_read = counts
            .event_payload_bytes_read
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    });
}

#[cfg(not(test))]
pub(super) fn record_event_payload_bytes_read(_bytes: usize) {}

#[cfg(test)]
pub(super) fn record_blob_payload_bytes_read(bytes: usize) {
    update(|counts| {
        counts.blob_payload_bytes_read = counts
            .blob_payload_bytes_read
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    });
}

#[cfg(not(test))]
pub(super) fn record_blob_payload_bytes_read(_bytes: usize) {}

macro_rules! counter {
    ($name:ident, $field:ident) => {
        #[cfg(test)]
        pub(super) fn $name() {
            update(|counts| counts.$field = counts.$field.saturating_add(1));
        }

        #[cfg(not(test))]
        pub(super) fn $name() {}
    };
}

counter!(record_truncation, truncations);
counter!(record_deletion, deletions);
counter!(record_permission_change, permission_changes);
counter!(record_file_sync, file_syncs);
counter!(record_directory_sync, directory_syncs);
