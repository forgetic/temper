// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use super::{AgentRunEventV1, BlobAttachmentV1, InlineContentV1};

/// Current version of the operator-local transcript record.
pub const OPERATOR_TRANSCRIPT_RECORD_VERSION: u32 = 1;
/// Maximum complete operator-local transcript size accepted from one run.
pub const MAX_OPERATOR_TRANSCRIPT_BYTES: usize = 2 * 1024 * 1024;
/// Maximum records accepted from one operator-local transcript.
pub const MAX_OPERATOR_TRANSCRIPT_RECORDS: usize = 128;

/// One bounded model-visible graph result retained only in an operator-local
/// diagnostic capture. It is not an activity event and therefore cannot enter
/// normalized durable activity, graph lineage, or aggregate decision evidence.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorTranscriptToolResultV1 {
    pub version: u32,
    pub call_id: String,
    pub tool_name: String,
    pub model_result_text: InlineContentV1,
}

impl std::fmt::Debug for OperatorTranscriptToolResultV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperatorTranscriptToolResultV1")
            .field("version", &self.version)
            .field("call_id", &self.call_id)
            .field("tool_name", &self.tool_name)
            .field("model_result_bytes", &self.model_result_text.text.len())
            .field("truncated", &self.model_result_text.truncated)
            .finish()
    }
}

impl OperatorTranscriptToolResultV1 {
    /// Enforces the closed local-capture shape and the activity protocol's
    /// existing identifier/inline bounds.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != OPERATOR_TRANSCRIPT_RECORD_VERSION {
            return Err(format!(
                "unsupported operator transcript version {}; expected {OPERATOR_TRANSCRIPT_RECORD_VERSION}",
                self.version
            ));
        }
        for (name, value) in [("call_id", &self.call_id), ("tool_name", &self.tool_name)] {
            if value.is_empty() || value.len() > crate::MAX_IDENTIFIER_BYTES {
                return Err(format!(
                    "operator transcript {name} must contain 1..={} UTF-8 bytes",
                    crate::MAX_IDENTIFIER_BYTES
                ));
            }
        }
        if !self.tool_name.starts_with("codebase_memory_") {
            return Err("operator transcript tool must be a codebase-memory wrapper".to_string());
        }
        if self.model_result_text.text.is_empty()
            || self.model_result_text.text.len() > crate::MAX_INLINE_CONTENT_BYTES
        {
            return Err(format!(
                "operator transcript result must contain 1..={} UTF-8 bytes",
                crate::MAX_INLINE_CONTENT_BYTES
            ));
        }
        Ok(())
    }
}

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
    OperatorTranscriptV1 {
        version: u32,
        record: OperatorTranscriptToolResultV1,
    },
}

impl TraceExportRecordV1 {
    /// Wraps a canonical event in the current export envelope.
    pub fn event(event: AgentRunEventV1) -> Self {
        Self::AgentRunEventV1 {
            version: TRACE_EXPORT_RECORD_VERSION,
            event,
        }
    }

    /// Wraps a blob attachment in the current export envelope.
    pub fn attachment(attachment: BlobAttachmentV1) -> Self {
        Self::BlobAttachmentV1 {
            version: TRACE_EXPORT_RECORD_VERSION,
            attachment,
        }
    }

    /// Wraps an operator-local transcript item. These records may accompany a
    /// diagnostic export but are never canonical activity events.
    pub fn operator_transcript(record: OperatorTranscriptToolResultV1) -> Self {
        Self::OperatorTranscriptV1 {
            version: TRACE_EXPORT_RECORD_VERSION,
            record,
        }
    }

    /// Returns the envelope version carried by this record.
    pub const fn version(&self) -> u32 {
        match self {
            Self::AgentRunEventV1 { version, .. }
            | Self::BlobAttachmentV1 { version, .. }
            | Self::OperatorTranscriptV1 { version, .. } => *version,
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
    OperatorTranscriptV1 {
        version: u32,
        record: OperatorTranscriptToolResultV1,
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
            | TraceExportRecordWire::BlobAttachmentV1 { version, .. }
            | TraceExportRecordWire::OperatorTranscriptV1 { version, .. } => *version,
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
            TraceExportRecordWire::OperatorTranscriptV1 { version, record } => {
                record.validate().map_err(D::Error::custom)?;
                Self::OperatorTranscriptV1 { version, record }
            }
        })
    }
}
