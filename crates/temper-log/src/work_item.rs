// SPDX-License-Identifier: MPL-2.0

//! The canonical, repo-qualified `artifact.ref` join key (§3 of the design).
//!
//! [`WorkItemRef`] is *the* primary key shared by the two log projections: the
//! human tag a person greps for and the `artifact.ref` machine field an agent
//! filters on are the **same string**. `grep 'acme/widgets#42'` over text and
//! `where artifact.ref == "acme/widgets#42"` over JSON return the same set
//! (§3 rule 2).
//!
//! ## Bare vs provider-qualified repo
//!
//! The runner's stable identity ([`temper_forge::RepositoryId`]) is
//! *provider-qualified* — it renders `forgejo:acme/widgets`. The human/join form
//! is the **bare** `owner/repo` (`acme/widgets`), per §7's reference output.
//! [`WorkItemRef`] therefore stores the bare repo display path; the caller is
//! responsible for stripping the provider scheme. [`strip_provider_scheme`]
//! does exactly that and is what the runner-side glue (WI-2) should call when it
//! has a `RepositoryId`.
//!
//! ## No heavyweight deps
//!
//! `temper-log` is a tier-crossing leaf crate with no internal dependencies (see
//! `depgraph-rules.toml`). It therefore does **not** depend on `temper-forge` /
//! `temper-workflow`; [`WorkItemRef`] is a small self-contained value type
//! accepting plain inputs (a bare repo path string, a number, and an
//! [`ArtifactKind`]). The runner converts its coordinate types into these
//! plain inputs at the call site.
//!
//! [`temper_forge::RepositoryId`]: https://docs.rs/temper-forge

use std::fmt;

/// Whether a work item is a forge issue or a pull request.
///
/// This drives the two `artifact.ref` shapes (§3 / §7):
/// `acme/widgets#42` for an issue, `acme/widgets PR#44` for a PR.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArtifactKind {
    /// A forge issue. Renders `owner/repo#n`.
    Issue,
    /// A forge pull request. Renders `owner/repo PR#n`.
    PullRequest,
}

impl ArtifactKind {
    /// The stable token for the `artifact.kind`-style machine fields.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::PullRequest => "pull_request",
        }
    }
}

/// A repo-qualified reference to one work item: the canonical `artifact.ref`.
///
/// Construct with [`WorkItemRef::issue`] or [`WorkItemRef::pull_request`] (or the
/// generic [`WorkItemRef::new`]). The [`Display`] impl renders the canonical
/// join key:
///
/// ```
/// use temper_log::WorkItemRef;
///
/// assert_eq!(WorkItemRef::issue("acme/widgets", 42).to_string(), "acme/widgets#42");
/// assert_eq!(
///     WorkItemRef::pull_request("acme/api", 19).to_string(),
///     "acme/api PR#19"
/// );
/// ```
///
/// The same string is used for the human `[tag]` and the `artifact.ref` /
/// `pr.ref` machine fields, so the two projections share a primary key.
///
/// [`Display`]: std::fmt::Display
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkItemRef {
    /// Bare `owner/repo` display path (provider scheme already stripped).
    repo: String,
    /// Repository-local artifact number.
    number: u64,
    /// Issue vs pull request — selects the `#n` vs ` PR#n` shape.
    kind: ArtifactKind,
}

impl WorkItemRef {
    /// Builds a reference from a bare repo path, a number, and a kind.
    ///
    /// `repo` must already be the bare `owner/repo` form (no `forgejo:` scheme);
    /// call [`strip_provider_scheme`] first if you hold a provider-qualified id.
    pub fn new(repo: impl Into<String>, number: u64, kind: ArtifactKind) -> Self {
        Self {
            repo: repo.into(),
            number,
            kind,
        }
    }

    /// Builds an issue reference (`owner/repo#n`).
    pub fn issue(repo: impl Into<String>, number: u64) -> Self {
        Self::new(repo, number, ArtifactKind::Issue)
    }

    /// Builds a pull-request reference (`owner/repo PR#n`).
    pub fn pull_request(repo: impl Into<String>, number: u64) -> Self {
        Self::new(repo, number, ArtifactKind::PullRequest)
    }

    /// The bare `owner/repo` repository path (used for the `repo=` field).
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// The repository-local artifact number.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// The artifact kind (issue vs pull request).
    pub fn kind(&self) -> ArtifactKind {
        self.kind
    }

    /// The bracketed human correlation tag, e.g. `[acme/widgets#42]`.
    ///
    /// This is the `[repo#n]` / `[repo PR#n]` subject tag the human lines carry
    /// (§7). The inner string equals the `artifact.ref` field, so the bracketed
    /// form stays greppable to the same key.
    pub fn human_tag(&self) -> String {
        format!("[{self}]")
    }
}

impl fmt::Display for WorkItemRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ArtifactKind::Issue => write!(f, "{}#{}", self.repo, self.number),
            ArtifactKind::PullRequest => write!(f, "{} PR#{}", self.repo, self.number),
        }
    }
}

/// Strips a `provider:` scheme prefix from a stable repository id.
///
/// The runner's [`temper_forge::RepositoryId`] renders provider-qualified
/// (`forgejo:acme/widgets`, `github:acme/api`); the human/join form is the bare
/// `owner/repo`. This removes a leading `scheme:` segment when present, leaving
/// `owner/repo` untouched if it is already bare.
///
/// It splits on the *first* `:` only and requires the remainder to look like a
/// path (`owner/repo`); an input with no scheme (or a `:` that is part of the
/// path, which forge repo ids never have) is returned unchanged.
///
/// ```
/// use temper_log::strip_provider_scheme;
///
/// assert_eq!(strip_provider_scheme("forgejo:acme/widgets"), "acme/widgets");
/// assert_eq!(strip_provider_scheme("github:acme/api"), "acme/api");
/// assert_eq!(strip_provider_scheme("acme/widgets"), "acme/widgets");
/// ```
///
/// [`temper_forge::RepositoryId`]: https://docs.rs/temper-forge
pub fn strip_provider_scheme(repo_id: &str) -> &str {
    match repo_id.split_once(':') {
        // A `scheme:owner/repo` form: drop the scheme, keep the path. The
        // remainder must contain a `/` (the owner/repo separator) to qualify as
        // a path; otherwise we leave the input alone rather than mangle it.
        Some((_scheme, rest)) if rest.contains('/') => rest,
        _ => repo_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_ref_uses_hash() {
        let r = WorkItemRef::issue("acme/widgets", 42);
        assert_eq!(r.to_string(), "acme/widgets#42");
        assert_eq!(r.human_tag(), "[acme/widgets#42]");
        assert_eq!(r.repo(), "acme/widgets");
        assert_eq!(r.number(), 42);
        assert_eq!(r.kind(), ArtifactKind::Issue);
    }

    #[test]
    fn pull_request_ref_uses_space_pr_hash() {
        let r = WorkItemRef::pull_request("acme/widgets", 44);
        assert_eq!(r.to_string(), "acme/widgets PR#44");
        assert_eq!(r.human_tag(), "[acme/widgets PR#44]");
        assert_eq!(r.kind(), ArtifactKind::PullRequest);
    }

    #[test]
    fn multi_repo_refs_are_distinct_and_repo_qualified() {
        // Issue numbers collide across repos; the repo qualifier disambiguates.
        let widgets = WorkItemRef::issue("acme/widgets", 7);
        let api = WorkItemRef::issue("acme/api", 7);
        assert_ne!(widgets, api);
        assert_eq!(widgets.to_string(), "acme/widgets#7");
        assert_eq!(api.to_string(), "acme/api#7");
    }

    #[test]
    fn join_key_is_identical_string_for_grep_and_filter() {
        // §3 rule 2: the human tag and the artifact.ref field are the same
        // string, so grep over text and filter over JSON return the same set.
        let r = WorkItemRef::issue("acme/widgets", 42);
        let field_value = r.to_string();
        let human = r.human_tag();
        assert!(human.contains(&field_value));
        assert_eq!(human, format!("[{field_value}]"));
    }

    #[test]
    fn strip_scheme_handles_provider_and_bare_forms() {
        assert_eq!(
            strip_provider_scheme("forgejo:acme/widgets"),
            "acme/widgets"
        );
        assert_eq!(strip_provider_scheme("github:acme/api"), "acme/api");
        // Already bare: unchanged.
        assert_eq!(strip_provider_scheme("acme/widgets"), "acme/widgets");
        // A bare single token without a path separator: left alone.
        assert_eq!(strip_provider_scheme("repo-0001"), "repo-0001");
    }

    #[test]
    fn ref_from_stripped_id_round_trips() {
        let bare = strip_provider_scheme("forgejo:acme/widgets");
        let r = WorkItemRef::issue(bare, 42);
        assert_eq!(r.to_string(), "acme/widgets#42");
    }
}
