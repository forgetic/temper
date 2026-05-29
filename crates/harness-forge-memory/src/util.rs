use harness_forge::{ForgeError, ForgeResult, ItemNumber, UpsertLabel, Version};

pub(crate) fn check_expected_version(
    kind: &str,
    id: &impl std::fmt::Display,
    expected: Option<Version>,
    actual: Version,
) -> ForgeResult<()> {
    match expected {
        Some(expected) if expected != actual => Err(ForgeError::Conflict(format!(
            "{kind} {id} expected version {expected} but found {actual}"
        ))),
        _ => Ok(()),
    }
}

pub(crate) fn validate_create_repository(
    input: &harness_forge::CreateRepository,
) -> ForgeResult<()> {
    if input.owner.trim().is_empty() {
        return Err(ForgeError::InvalidRequest(
            "repository owner must not be empty".into(),
        ));
    }
    if input.name.trim().is_empty() {
        return Err(ForgeError::InvalidRequest(
            "repository name must not be empty".into(),
        ));
    }
    if input.default_branch.trim().is_empty() {
        return Err(ForgeError::InvalidRequest(
            "repository default branch must not be empty".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_upsert_label(input: &UpsertLabel) -> ForgeResult<()> {
    if input.name.trim().is_empty() {
        return Err(ForgeError::InvalidRequest(
            "label name must not be empty".into(),
        ));
    }
    Ok(())
}

pub(crate) fn next_item_number(
    existing: impl Iterator<Item = ItemNumber>,
) -> ForgeResult<ItemNumber> {
    let highest = existing.map(|number| number.get()).max().unwrap_or(0);
    let next = highest
        .checked_add(1)
        .ok_or_else(|| ForgeError::Backend("item number counter overflowed".into()))?;
    Ok(ItemNumber::new(next))
}

pub(crate) fn next_comment_number(count: usize) -> ForgeResult<u64> {
    u64::try_from(count)
        .ok()
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| ForgeError::Backend("comment number counter overflowed".into()))
}
