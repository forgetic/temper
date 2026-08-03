// SPDX-License-Identifier: MPL-2.0

#[path = "run_evidence/assertions.rs"]
mod assertions;
#[path = "run_evidence/context.rs"]
mod context;
#[path = "run_evidence/io.rs"]
mod io;
#[path = "run_evidence/model.rs"]
mod model;
#[path = "run_evidence/render.rs"]
mod render;
#[path = "run_evidence/script_assertions.rs"]
mod script_assertions;
#[path = "run_evidence/validation.rs"]
mod validation;

pub(super) use assertions::{evaluate_manifest_assertions, print_assertions};
pub(super) use context::RunEvidenceContext;
pub(super) use io::load_run_evidence;
pub(super) use model::{
    ArtifactCollections, BinaryIdentityEvidence, CiJobEvidence, CiObservationEvidence,
    CiRequestEvidence, CiStateEvidence, ConvergenceEvidence, EffectiveConfigurationEvidence,
    ExecutionEvidence, FinalStateEvidence, IssueStateEvidence, ObservabilityEvidence,
    ProviderEvidence, PullRequestStateEvidence, RepositoryBranchStateEvidence,
    RepositoryStateEvidence, RunEvidenceArtifact, RunEvidenceVerdict, StimulusEvidence,
    StructuredEventEvidence, VerifiedFailureProofEvidence,
};
pub(super) use script_assertions::append_script_assertions;
