//! GitHub provider DTOs (serde).
//!
//! The DTOs are deliberately lenient: unknown fields are ignored and optional
//! fields default, so the backend tolerates provider drift between
//! `api.github.com` and GitHub Enterprise versions. GitHub serializes unset
//! text fields (issue/pull bodies, comment bodies) as JSON `null`, so those are
//! optional and mapping treats `None` as empty.
//!
//! The DTOs are grouped by domain: [`artifacts`] (users, repositories, labels,
//! comments), [`issues`], [`pulls`] (pull requests, reviews, merge result), and
//! [`ci`] (GitHub Actions runs and jobs). They are re-exported flat so callers
//! continue to use `crate::types::SomeDto`.

mod artifacts;
mod ci;
mod issues;
mod pulls;

pub(crate) use artifacts::{CommentDto, LabelDto, RepositoryDto, UserDto};
pub(crate) use ci::{WorkflowJobDto, WorkflowJobsEnvelopeDto, WorkflowRunsEnvelopeDto};
pub(crate) use issues::IssueDto;
pub(crate) use pulls::{MergeResultDto, PrBranchDto, PrRepoDto, PullRequestDto, ReviewDto};
