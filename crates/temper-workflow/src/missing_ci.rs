// SPDX-License-Identifier: MPL-2.0

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Durable identity of an interrupted missing-current-head CI parking pass.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MissingCiRecoveryState {
    /// Exact pull-request head whose missing CI was finally validated.
    pub head_sha: String,
    /// First successful missing observation retained for the actionable audit.
    pub first_observed_at: DateTime<Utc>,
}
