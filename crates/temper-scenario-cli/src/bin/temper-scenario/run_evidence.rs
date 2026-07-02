// SPDX-License-Identifier: MPL-2.0

#[path = "run_evidence/context.rs"]
mod context;
#[path = "run_evidence/io.rs"]
mod io;
#[path = "run_evidence/model.rs"]
mod model;
#[path = "run_evidence/render.rs"]
mod render;
#[path = "run_evidence/validation.rs"]
mod validation;

pub(super) use context::RunEvidenceContext;
pub(super) use io::load_run_evidence;
pub(super) use model::{
    ArtifactCollections, CiJobEvidence, CiStateEvidence, ConvergenceEvidence, FinalStateEvidence,
    IssueStateEvidence, ProviderEvidence, PullRequestStateEvidence, RunEvidenceArtifact,
    WorkerTickEvidence,
};
