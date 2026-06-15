// SPDX-License-Identifier: MPL-2.0

use crate::{RepoAccess, WorkspaceManifest, WorkspaceRepo};

mod fixtures;
mod job_context;
mod result;
mod workspace;

fn sample_manifest() -> WorkspaceManifest {
    WorkspaceManifest {
        coordination_key: "coord-for-code-42".to_string(),
        repos: vec![
            WorkspaceRepo {
                repo: "ai/temper".to_string(),
                dir: "temper".to_string(),
                access: RepoAccess::Writable,
                default_branch: "main".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/coord-for-code-42".to_string()),
                depends_on: Vec::new(),
            },
            WorkspaceRepo {
                repo: "ai/smith".to_string(),
                dir: "smith".to_string(),
                access: RepoAccess::Writable,
                default_branch: "main".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/coord-for-code-42".to_string()),
                // smith consumes temper's protocol crate -> land after temper.
                depends_on: vec!["ai/temper".to_string()],
            },
            WorkspaceRepo {
                repo: "ai/skein".to_string(),
                dir: "skein".to_string(),
                access: RepoAccess::ReadOnly,
                default_branch: "main".to_string(),
                base_branch: "main".to_string(),
                branch_hint: None,
                depends_on: Vec::new(),
            },
        ],
    }
}
