//! Deep owned bytes of jig's settings rows; no history is retained by the root.
use alloc::boxed::Box;
use jig_domain_authority as authority;
use jig_domain_core::settings::{Edit, Item, Record, Value};
use jig_domain_tasks as tasks;

pub(super) fn record(row: &Record) -> Option<u64> {
    match row {
        Record::Default { default, .. } => value(&default.value),
        Record::Changed { changed, .. } => match &changed.value {
            Some(changed) => value(changed),
            None => Some(0),
        },
        Record::Revision { revision, .. } => items(&revision.items),
        Record::Proposal { value, .. } => items(&value.items),
        Record::Charter { value: version, .. } => match &version.charter {
            Some(charter) => u64::try_from(size_of::<jig_domain_core::Charter>())
                .ok()?
                .checked_add(jig_domain_core::policy_owned_bytes(&charter.policy)?),
            None => Some(0),
        },
        Record::Extent { .. } => Some(0),
    }
}
fn items(items: &[Item]) -> Option<u64> {
    let mut bytes = u64::try_from(items.len()).ok()?.checked_mul(u64::try_from(size_of::<Item>()).ok()?)?;
    for item in items {
        if let Edit::Set(set) = &item.edit {
            bytes = bytes.checked_add(value(set)?)?;
        }
    }
    Some(bytes)
}
fn value(value: &Value) -> Option<u64> {
    match value {
        Value::Charter(charter) => u64::try_from(size_of::<jig_domain_core::Charter>())
            .ok()?
            .checked_add(jig_domain_core::policy_owned_bytes(&charter.policy)?),
        Value::Template(template) => {
            let mut bytes = u64::try_from(size_of::<jig_domain_core::Template>())
                .ok()?
                .checked_add(u64::try_from(template.name.len()).ok()?)?
                .checked_add(contract(&template.contract)?)?;
            if let Some(choice) = &template.llm {
                bytes = bytes.checked_add(
                    u64::try_from(choice.subagents.len())
                        .ok()?
                        .checked_mul(u64::try_from(size_of::<tasks::ModelEffort>()).ok()?)?,
                )?;
            }
            Some(bytes)
        }
        Value::Policy(configured) => {
            let policy = &configured.policy;
            let mut bytes = u64::try_from(size_of::<jig_domain_core::ConfiguredPolicy>())
                .ok()?
                .checked_add(authority(&policy.ceiling)?)?
                .checked_add(grants(&policy.projections)?)?
                .checked_add(requirements(&policy.requirements)?)?
                .checked_add(
                    u64::try_from(policy.roles.len())
                        .ok()?
                        .checked_mul(u64::try_from(size_of::<authority::Role>()).ok()?)?,
                )?
                .checked_add(
                    u64::try_from(configured.permissions.len())
                        .ok()?
                        .checked_mul(u64::try_from(size_of::<jig_domain_people::PermissionRole>()).ok()?)?,
                )?
                .checked_add(
                    u64::try_from(configured.templates.len())
                        .ok()?
                        .checked_mul(u64::try_from(size_of::<jig_domain_people::TemplateOverride>()).ok()?)?,
                )?;
            for role in &policy.roles {
                bytes = bytes.checked_add(authority(&role.authority)?)?;
            }
            Some(bytes)
        }
        Value::Rules(rules) => u64::try_from(size_of::<authority::Rules>())
            .ok()?
            .checked_add(authority(&rules.ceiling)?)?
            .checked_add(requirements(&rules.requirements)?)?
            .checked_add(
                u64::from(rules.implies.len()).checked_mul(u64::try_from(size_of::<authority::Implication>()).ok()?)?,
            ),
    }
}
fn authority(value: &authority::Authority) -> Option<u64> {
    let mut bytes = grants(&value.grants)?
        .checked_add(
            u64::try_from(value.delegation.kinds.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<authority::Executor>()).ok()?)?,
        )?
        .checked_add(
            u64::try_from(value.note_resources.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<authority::ResourceScope>()).ok()?)?,
        )?;
    for scope in &value.note_resources {
        bytes = bytes.checked_add(pattern(&scope.pattern)?)?;
    }
    Some(bytes)
}
fn grants(grants: &[authority::Grant]) -> Option<u64> {
    let mut bytes =
        u64::try_from(grants.len()).ok()?.checked_mul(u64::try_from(size_of::<authority::Grant>()).ok()?)?;
    for grant in grants {
        bytes = bytes.checked_add(pattern(&grant.pattern)?)?;
    }
    Some(bytes)
}
fn requirements(requirements: &[authority::Requirement]) -> Option<u64> {
    let mut bytes = u64::try_from(requirements.len())
        .ok()?
        .checked_mul(u64::try_from(size_of::<authority::Requirement>()).ok()?)?;
    for requirement in requirements {
        bytes = bytes.checked_add(pattern(&requirement.pattern)?)?;
    }
    Some(bytes)
}
fn pattern(pattern: &authority::Pattern) -> Option<u64> {
    let mut bytes =
        u64::try_from(pattern.segments.len()).ok()?.checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
    for segment in &pattern.segments {
        bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
    }
    match &pattern.last {
        authority::Last::Exact(last) | authority::Last::Open(last) => {
            bytes.checked_add(u64::try_from(last.len()).ok()?)
        }
    }
}
fn contract(contract: &tasks::Contract) -> Option<u64> {
    match contract {
        tasks::Contract::Report { .. } | tasks::Contract::Change { .. } => Some(0),
        tasks::Contract::Approval { state, part } => {
            let mut bytes = u64::try_from(state.len()).ok()?;
            if let tasks::PartKey::Path(segments) = &part.key {
                bytes = bytes.checked_add(
                    u64::from(segments.capacity()).checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
                )?;
                for segment in segments {
                    bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
                }
            }
            Some(bytes)
        }
        tasks::Contract::Verdict { choices } => {
            // Each verdict owns its field rules and their names.
            let mut bytes =
                u64::try_from(choices.len()).ok()?.checked_mul(u64::try_from(size_of::<tasks::Verdict>()).ok()?)?;
            for choice in choices {
                for field in &choice.fields {
                    bytes = bytes.checked_add(u64::try_from(field.name.len()).ok()?)?;
                }
                bytes = bytes.checked_add(
                    u64::try_from(choice.fields.len())
                        .ok()?
                        .checked_mul(u64::try_from(size_of::<tasks::FieldRule>()).ok()?)?,
                )?;
            }
            Some(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approval_settings_contract_counts_its_reserved_path_and_segments() {
        let mut segments = skein_lib::List::with_capacity(3);
        segments.push(Box::from(b"forge".as_slice())).expect("first segment");
        segments.push(Box::from(b"main".as_slice())).expect("second segment");
        let approval = tasks::Contract::Approval {
            state: Box::from(b"approved".as_slice()),
            part: tasks::Part { connector: 1, kind: tasks::PartKind::Resource, key: tasks::PartKey::Path(segments) },
        };
        assert_eq!(contract(&approval), Some(u64::try_from(3 * size_of::<Box<[u8]>>() + 17).expect("small path")));
    }
}
