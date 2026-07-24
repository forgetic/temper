use crate::ids::RoleId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Exact, durable identity of a worker assignment.
///
/// Every member is optional so records can be extended independently and old
/// fixtures remain compatible. Runtime assignment claims populate all fields
/// available for a job.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurableAssignment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Opaque fence for one dispatch attempt. Optional for legacy metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<RoleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordination_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment_pr_head: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_claim_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_claim_assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}
