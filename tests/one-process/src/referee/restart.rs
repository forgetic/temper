//! The recovered forge is a whole acknowledged prefix, with at most one uncertain commit.
use std::collections::BTreeMap;
use temper_engine_domain as root;
use temper_engine_domain_forge::{Key, Stored};

pub type Records = BTreeMap<Key, Stored>;

pub(crate) fn forge(rows: &BTreeMap<root::Key, root::Record>) -> Records {
    rows.values()
        .filter_map(|row| match row {
            root::Record::Forge { row } => Some((temper_engine_domain_forge::stored_key(row), *row.clone())),
            root::Record::Core(_) => None,
        })
        .collect()
}

/// Reject a recovered row set other than either allowed whole commit boundary.
/// # Errors
/// Names the first row that differs from both allowed boundaries.
pub fn check(acknowledged: &Records, uncertain: Option<&Records>, recovered: &Records) -> Result<(), Key> {
    if recovered == acknowledged || uncertain == Some(recovered) {
        return Ok(());
    }
    let expected = uncertain.unwrap_or(acknowledged);
    expected
        .keys()
        .chain(recovered.keys())
        .find(|key| expected.get(*key) != recovered.get(*key))
        .cloned()
        .map_or(Ok(()), Err)
}

pub(crate) fn next(rows: &BTreeMap<root::Key, root::Record>, writes: &[root::Write]) -> Records {
    let mut next = rows.clone();
    for write in writes {
        apply(&mut next, write);
    }
    forge(&next)
}

/// Apply ordered writes to the referee's durable mirror, including store-side copies.
pub(crate) fn apply(rows: &mut BTreeMap<root::Key, root::Record>, write: &root::Write) {
    use jig_domain_core::{CoreKey, CoreRecord, Key, Record, settings};
    match write {
        root::Write::Save(row) => {
            rows.insert(row.key(), row.clone());
        }
        root::Write::Erase(key) => {
            rows.remove(key);
        }
        root::Write::CopySettingsRevision(number) => {
            let held = root::Key::Core(Key::Core(CoreKey::Settings(settings::Key::Held(*number))));
            let root::Record::Core(Record::Core(CoreRecord::Settings(settings::Record::Revision { revision, .. }))) =
                rows.get(&held).expect("held revision exists before copy")
            else {
                panic!("held revision shape")
            };
            let row = root::Record::Core(Record::Core(CoreRecord::Settings(settings::Record::Revision {
                history: true,
                revision: revision.clone(),
            })));
            rows.insert(row.key(), row);
        }
    }
}
