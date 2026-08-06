// SPDX-License-Identifier: MPL-2.0

//! Non-secret codebase-memory worker/agent wire shapes.

use serde::{Deserialize, Serialize};

/// Resolved codebase-memory MCP tool settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryToolConfig {
    pub mode: CodebaseMemoryMode,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    pub index: CodebaseMemoryIndex,
    pub startup_timeout_secs: u64,
    pub index_timeout_secs: u64,
    /// Host-only cleanup policy. The agent carries this non-secret shape across
    /// the process boundary but never exposes maintenance operations to models.
    #[serde(default)]
    pub retention: CodebaseMemoryRetentionPolicy,
}

/// Worker-owned bounded maintenance policy carried in the non-secret runtime
/// protocol. Defaults are disabled for compatibility with older workers; the
/// resolved Temper config always sends its explicit effective policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryRetentionPolicy {
    pub enabled: bool,
    pub max_obsolete_projects: u32,
    pub max_age_days: u32,
    pub maintenance_interval_secs: u64,
    pub maintenance_timeout_secs: u64,
    pub inventory_page_size: u32,
    pub max_inventory_pages: u32,
    pub max_deletions_per_run: u32,
}

impl Default for CodebaseMemoryRetentionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_obsolete_projects: 64,
            max_age_days: 30,
            maintenance_interval_secs: 3_600,
            maintenance_timeout_secs: 30,
            inventory_page_size: 50,
            max_inventory_pages: 20,
            max_deletions_per_run: 16,
        }
    }
}

impl CodebaseMemoryRetentionPolicy {
    fn validate(self) -> Result<Self, String> {
        if self.max_obsolete_projects > 10_000 {
            return Err(
                "codebase_memory.retention.max_obsolete_projects must be at most 10000".to_string(),
            );
        }
        for (field, value, maximum) in [
            ("max_age_days", u64::from(self.max_age_days), 3_650),
            (
                "maintenance_interval_secs",
                self.maintenance_interval_secs,
                u64::MAX,
            ),
            (
                "maintenance_timeout_secs",
                self.maintenance_timeout_secs,
                300,
            ),
            (
                "inventory_page_size",
                u64::from(self.inventory_page_size),
                200,
            ),
            (
                "max_inventory_pages",
                u64::from(self.max_inventory_pages),
                100,
            ),
            (
                "max_deletions_per_run",
                u64::from(self.max_deletions_per_run),
                100,
            ),
        ] {
            if value == 0 || value > maximum {
                return Err(format!(
                    "codebase_memory.retention.{field} must be between 1 and {maximum}"
                ));
            }
        }
        Ok(self)
    }
}

impl CodebaseMemoryToolConfig {
    pub fn applies_to_role(&self, role: &str) -> bool {
        self.roles
            .iter()
            .any(|allowed| allowed == "*" || allowed == role)
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err("codebase_memory.command must not be empty".to_string());
        }
        if self.startup_timeout_secs == 0 {
            return Err(
                "codebase_memory.startup_timeout_secs must be greater than zero".to_string(),
            );
        }
        if self.index_timeout_secs == 0 {
            return Err("codebase_memory.index_timeout_secs must be greater than zero".to_string());
        }
        self.retention.validate()?;
        for role in &self.roles {
            if role.trim().is_empty() {
                return Err("codebase_memory.roles entries must not be empty".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodebaseMemoryMode {
    Auto,
    Required,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodebaseMemoryIndex {
    Off,
    Background,
    Blocking,
}
