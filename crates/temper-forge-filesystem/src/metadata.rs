use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use temper_forge_model::{ForgeError, ForgeResult, User, UserId};

const STORAGE_SCHEMA_VERSION: u32 = 1;
const DEFAULT_USER_ID: &str = "user-1";
const DEFAULT_USER_HANDLE: &str = "local";
const DEFAULT_USER_DISPLAY_NAME: &str = "Local User";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Metadata {
    pub(crate) schema_version: u32,
    pub(crate) current_user: User,
    pub(crate) next_repository_number: u64,
    pub(crate) clock_tick: u64,
}

impl Metadata {
    pub(crate) fn validate(&self) -> ForgeResult<()> {
        if self.schema_version != STORAGE_SCHEMA_VERSION {
            return Err(ForgeError::Backend(format!(
                "unsupported filesystem storage schema version {}",
                self.schema_version
            )));
        }

        if self.next_repository_number == 0 {
            return Err(ForgeError::Backend(
                "filesystem metadata next_repository_number must be greater than zero".into(),
            ));
        }

        timestamp_from_tick(self.clock_tick)?;
        Ok(())
    }
}

pub(crate) fn default_metadata() -> Metadata {
    Metadata {
        schema_version: STORAGE_SCHEMA_VERSION,
        current_user: User {
            id: UserId::new(DEFAULT_USER_ID),
            handle: DEFAULT_USER_HANDLE.into(),
            display_name: Some(DEFAULT_USER_DISPLAY_NAME.into()),
            email: None,
        },
        next_repository_number: 1,
        clock_tick: 0,
    }
}

pub(crate) fn next_timestamp(metadata: &mut Metadata) -> ForgeResult<DateTime<Utc>> {
    metadata.clock_tick = metadata
        .clock_tick
        .checked_add(1)
        .ok_or_else(|| ForgeError::Backend("filesystem logical clock overflowed".into()))?;
    timestamp_from_tick(metadata.clock_tick)
}

pub(crate) fn timestamp_from_tick(tick: u64) -> ForgeResult<DateTime<Utc>> {
    let seconds = i64::try_from(tick)
        .map_err(|_| ForgeError::Backend("filesystem logical clock is too large".into()))?;
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .ok_or_else(|| ForgeError::Backend("filesystem logical clock is out of range".into()))
}
