// SPDX-License-Identifier: MPL-2.0

//! Stable assertion template names accepted by scenario manifests.
//!
//! These templates are declarative behavior contracts. The catalog intentionally
//! exists before every template has a dedicated runner implementation so
//! checked-in scenarios can name the contract they are meant to protect.

/// One stable assertion template known to the scenario manifest checker.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AssertionTemplate {
    /// Stable manifest name used in `expect.template` or `expect.templates`.
    pub name: &'static str,
    /// Short human-readable contract summary.
    pub summary: &'static str,
}

/// Stable assertion template names accepted in scenario manifests.
pub const ASSERTION_TEMPLATE_NAMES: &[&str] = &[
    "single-pr-merged-source-closed",
    "review-requested-then-approved",
    "ci-fails-then-passes",
    "cross-repo-fanout-converges",
    "no-duplicate-prs",
    "quiescent-after-merge",
    "webhook-progress-before-poll-backstop",
];

/// Initial scenario assertion template catalog.
pub const ASSERTION_TEMPLATE_CATALOG: &[AssertionTemplate] = &[
    AssertionTemplate {
        name: "single-pr-merged-source-closed",
        summary: "one implementation PR merges and closes its source issue",
    },
    AssertionTemplate {
        name: "review-requested-then-approved",
        summary: "a review request is made before an approval unblocks landing",
    },
    AssertionTemplate {
        name: "ci-fails-then-passes",
        summary: "a failing CI signal is followed by a passing replacement signal",
    },
    AssertionTemplate {
        name: "cross-repo-fanout-converges",
        summary: "coordinated work fans out across repositories and converges",
    },
    AssertionTemplate {
        name: "no-duplicate-prs",
        summary: "repeated progress signals do not create duplicate implementation PRs",
    },
    AssertionTemplate {
        name: "quiescent-after-merge",
        summary: "no further workflow actions remain after successful merge convergence",
    },
    AssertionTemplate {
        name: "webhook-progress-before-poll-backstop",
        summary: "webhook progress is observed before any polling backstop is needed",
    },
];

/// Returns true when `name` is a known assertion template.
pub fn is_known_assertion_template(name: &str) -> bool {
    ASSERTION_TEMPLATE_CATALOG
        .iter()
        .any(|template| template.name == name)
}
