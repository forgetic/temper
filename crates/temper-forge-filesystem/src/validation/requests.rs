use temper_forge::{CreateRepository, ForgeError, ForgeResult, UpsertLabel, Version};

/// Enforces an optimistic-concurrency precondition.
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

pub(crate) fn validate_create_repository(input: &CreateRepository) -> ForgeResult<()> {
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
