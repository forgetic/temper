// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use super::{AgentRunEventV1, BlobAttachmentV1};

/// Current version of the self-contained trace export record envelope.
pub const TRACE_EXPORT_RECORD_VERSION: u32 = 1;

/// One line in a self-contained agent trace export.
///
/// The top-level tag and explicit version let offline consumers reject future
/// incompatible records without guessing from the enclosed DTO shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
// Export records are constructed and serialized one at a time; boxing the
// canonical event would only complicate the public DTO without reducing a collection.
#[allow(clippy::large_enum_variant)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceExportRecordV1 {
    AgentRunEventV1 {
        version: u32,
        event: AgentRunEventV1,
    },
    BlobAttachmentV1 {
        version: u32,
        attachment: BlobAttachmentV1,
    },
}

impl TraceExportRecordV1 {
    /// Wraps a canonical event in the current trace export envelope.
    pub fn event(event: AgentRunEventV1) -> Self {
        Self::AgentRunEventV1 {
            version: TRACE_EXPORT_RECORD_VERSION,
            event,
        }
    }

    /// Wraps a blob attachment in the current trace export envelope.
    pub fn attachment(attachment: BlobAttachmentV1) -> Self {
        Self::BlobAttachmentV1 {
            version: TRACE_EXPORT_RECORD_VERSION,
            attachment,
        }
    }

    /// Returns the envelope version carried by this record.
    pub const fn version(&self) -> u32 {
        match self {
            Self::AgentRunEventV1 { version, .. } | Self::BlobAttachmentV1 { version, .. } => {
                *version
            }
        }
    }
}

#[derive(Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TraceExportRecordWire {
    AgentRunEventV1 {
        version: u32,
        event: AgentRunEventV1,
    },
    BlobAttachmentV1 {
        version: u32,
        attachment: BlobAttachmentV1,
    },
}

impl<'de> Deserialize<'de> for TraceExportRecordV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let record = TraceExportRecordWire::deserialize(deserializer)?;
        let version = match &record {
            TraceExportRecordWire::AgentRunEventV1 { version, .. }
            | TraceExportRecordWire::BlobAttachmentV1 { version, .. } => *version,
        };
        if version != TRACE_EXPORT_RECORD_VERSION {
            return Err(D::Error::custom(format!(
                "unsupported trace export record version {version}; expected {TRACE_EXPORT_RECORD_VERSION}"
            )));
        }

        Ok(match record {
            TraceExportRecordWire::AgentRunEventV1 { version, event } => {
                Self::AgentRunEventV1 { version, event }
            }
            TraceExportRecordWire::BlobAttachmentV1 {
                version,
                attachment,
            } => Self::BlobAttachmentV1 {
                version,
                attachment,
            },
        })
    }
}
