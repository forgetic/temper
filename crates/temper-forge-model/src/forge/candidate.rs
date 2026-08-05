use super::{ForgeError, ForgeResult, ItemListDetails};
use crate::{Issue, IssueId, ItemNumber, PullRequest, PullRequestId, RepositoryId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::ops::Deref;

/// Default fixed ceiling for a periodic terminal-discovery page.
pub const DEFAULT_TERMINAL_CANDIDATE_PAGE_SIZE: usize = 100;

/// Largest candidate page a backend is permitted to return.
pub const MAX_CANDIDATE_PAGE_SIZE: usize = 1_000;

/// Lifecycle bucket used by consolidated candidate discovery.
///
/// `Terminal` means closed issues and both closed and merged pull requests.
#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateLifecycle {
    Open,
    Terminal,
}

/// Descriptive alias for [`CandidateLifecycle`].
pub type CandidateLifecycleBucket = CandidateLifecycle;

/// Label selection used by consolidated candidate discovery.
///
/// Unlike the conjunctive `labels` fields on [`super::IssueQuery`] and
/// [`super::PullRequestQuery`], `AnyOf` is disjunctive. Candidate APIs reject an
/// empty `AnyOf`; use [`Self::Unfiltered`] for a lifecycle-bounded unlabelled
/// read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateLabelSelection {
    Unfiltered,
    AnyOf(Vec<String>),
}

impl<'de> Deserialize<'de> for CandidateLabelSelection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Repr {
            Unfiltered,
            AnyOf(Vec<String>),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Unfiltered => Ok(Self::Unfiltered),
            Repr::AnyOf(labels) => Self::any_of(labels).map_err(serde::de::Error::custom),
        }
    }
}

/// Concise alias for [`CandidateLabelSelection`].
pub type CandidateLabels = CandidateLabelSelection;

impl CandidateLabelSelection {
    /// Builds a normalized non-empty any-label selection.
    pub fn any_of<I, S>(labels: I) -> ForgeResult<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let labels = normalize_candidate_labels(labels.into_iter().map(Into::into).collect())?;
        Ok(Self::AnyOf(labels))
    }

    /// Returns a validated, sorted, deduplicated selection for backend adapters.
    pub fn normalized(&self) -> ForgeResult<Option<Vec<String>>> {
        match self {
            Self::Unfiltered => Ok(None),
            Self::AnyOf(labels) => Ok(Some(normalize_candidate_labels(labels.clone())?)),
        }
    }
}

impl Default for CandidateLabelSelection {
    fn default() -> Self {
        Self::Unfiltered
    }
}

fn normalize_candidate_labels(mut labels: Vec<String>) -> ForgeResult<Vec<String>> {
    if labels.iter().any(String::is_empty) {
        return Err(ForgeError::InvalidRequest(
            "candidate labels must not be empty strings".to_string(),
        ));
    }
    labels.sort();
    labels.dedup();
    if labels.is_empty() {
        return Err(ForgeError::InvalidRequest(
            "candidate AnyOf labels must be non-empty".to_string(),
        ));
    }
    Ok(labels)
}

/// Stable position in candidate traversal order.
///
/// Candidates are ordered oldest update first, then by repository item number,
/// then by typed stable identity. The tie-breaks make equal provider timestamps
/// deterministic and ensure an item occurs in exactly one page of a frozen
/// sweep.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(bound(deserialize = "I: Deserialize<'de>", serialize = "I: Serialize"))]
pub struct CandidatePosition<I> {
    pub updated_at: DateTime<Utc>,
    pub number: ItemNumber,
    pub id: I,
}

/// Resumable cursor for a frozen candidate sweep.
///
/// `boundary` is the high-water position captured by the first page. Rows added
/// after that boundary cannot move older rows out of later pages. Repository,
/// lifecycle, and normalized label selection are carried in the cursor so a
/// continuation cannot accidentally be reused for a different query.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(deserialize = "I: Deserialize<'de>", serialize = "I: Serialize"))]
pub struct CandidateContinuation<I> {
    pub repository_id: RepositoryId,
    pub lifecycle: CandidateLifecycle,
    pub labels: CandidateLabelSelection,
    pub boundary: CandidatePosition<I>,
    pub after: CandidatePosition<I>,
    /// Opaque backend cursor used when a provider needs more than the portable
    /// timestamp/identity position to resume a large timestamp tie.
    ///
    /// Callers preserve this value verbatim. Compatibility backends may leave
    /// it empty and rely only on `boundary` and `after`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_cursor: Option<String>,
}

/// Optional bounded-page request attached to a candidate query.
///
/// `None` on the containing query preserves exhaustive, level-triggered open
/// discovery. Periodic terminal planners always supply this request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(deserialize = "I: Deserialize<'de>", serialize = "I: Serialize"))]
pub struct CandidatePageRequest<I> {
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<CandidateContinuation<I>>,
}

impl<I> CandidatePageRequest<I> {
    /// Starts a frozen sweep with `limit` rows per page.
    pub const fn first(limit: usize) -> Self {
        Self {
            limit,
            continuation: None,
        }
    }

    /// Starts a periodic terminal sweep with the portable default ceiling.
    pub const fn terminal() -> Self {
        Self::first(DEFAULT_TERMINAL_CANDIDATE_PAGE_SIZE)
    }

    /// Validates the portable row ceiling before a backend sends any request.
    pub fn validate(&self) -> ForgeResult<()> {
        if self.limit == 0 || self.limit > MAX_CANDIDATE_PAGE_SIZE {
            return Err(ForgeError::InvalidRequest(format!(
                "candidate page limit must be between 1 and {MAX_CANDIDATE_PAGE_SIZE}"
            )));
        }
        Ok(())
    }
}

/// One observable candidate page.
///
/// `raw_count` reports backend rows considered before identity deduplication and
/// page truncation; `returned_count` is exactly `items.len()`. `overflow` and
/// `exhausted` are explicit complements. An overflowing page always carries a
/// continuation and an exhausted page never does.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(bound(
    deserialize = "T: Deserialize<'de>, I: Deserialize<'de>",
    serialize = "T: Serialize, I: Serialize"
))]
pub struct CandidatePage<T, I> {
    pub items: Vec<T>,
    pub continuation: Option<CandidateContinuation<I>>,
    pub exhausted: bool,
    pub overflow: bool,
    pub raw_count: usize,
    pub returned_count: usize,
}

impl<T, I> Deref for CandidatePage<T, I> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl<T, I> IntoIterator for CandidatePage<T, I> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

/// Result type for issue candidate discovery.
pub type IssueCandidatePage = CandidatePage<Issue, IssueId>;

/// Result type for pull-request candidate discovery.
pub type PullRequestCandidatePage = CandidatePage<PullRequest, PullRequestId>;

/// Applies the portable ordering, deduplication, frozen boundary, and ceiling.
///
/// This is public for backend and wrapper conformance; callers normally use the
/// [`super::Forge`] candidate methods instead.
pub fn paginate_candidate_items<T, I, F>(
    items: Vec<T>,
    raw_count: usize,
    repository_id: &RepositoryId,
    lifecycle: CandidateLifecycle,
    labels: CandidateLabelSelection,
    request: Option<CandidatePageRequest<I>>,
    position: F,
) -> ForgeResult<CandidatePage<T, I>>
where
    I: Clone + Ord,
    F: Fn(&T) -> CandidatePosition<I>,
{
    let labels = match labels.normalized()? {
        Some(labels) => CandidateLabelSelection::AnyOf(labels),
        None => CandidateLabelSelection::Unfiltered,
    };
    let mut by_id = BTreeMap::<I, (CandidatePosition<I>, T)>::new();
    for item in items {
        let item_position = position(&item);
        match by_id.entry(item_position.id.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((item_position, item));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                // Provider duplicates should be identical. If they are not,
                // retaining the lowest stable position is deterministic.
                if item_position < entry.get().0 {
                    entry.insert((item_position, item));
                }
            }
        }
    }
    let mut positioned = by_id.into_values().collect::<Vec<_>>();
    positioned.sort_by(|left, right| left.0.cmp(&right.0));

    let request = request.or_else(|| {
        (lifecycle == CandidateLifecycle::Terminal).then(CandidatePageRequest::terminal)
    });
    let Some(request) = request else {
        let items = positioned
            .into_iter()
            .map(|(_, item)| item)
            .collect::<Vec<_>>();
        return Ok(CandidatePage {
            returned_count: items.len(),
            items,
            continuation: None,
            exhausted: true,
            overflow: false,
            raw_count,
        });
    };
    request.validate()?;

    if let Some(continuation) = &request.continuation {
        if continuation.repository_id != *repository_id
            || continuation.lifecycle != lifecycle
            || continuation.labels != labels
        {
            return Err(ForgeError::InvalidRequest(
                "candidate continuation does not match repository or normalized query scope"
                    .to_string(),
            ));
        }
        if continuation.after > continuation.boundary {
            return Err(ForgeError::InvalidRequest(
                "candidate continuation is beyond its frozen boundary".to_string(),
            ));
        }
    }

    let boundary = request
        .continuation
        .as_ref()
        .map(|continuation| continuation.boundary.clone())
        .or_else(|| positioned.last().map(|(position, _)| position.clone()));
    if let Some(boundary) = &boundary {
        positioned.retain(|(candidate, _)| {
            candidate <= boundary
                && request
                    .continuation
                    .as_ref()
                    .is_none_or(|continuation| candidate > &continuation.after)
        });
    } else {
        positioned.clear();
    }

    let overflow = positioned.len() > request.limit;
    positioned.truncate(request.limit);
    let after = positioned.last().map(|(position, _)| position.clone());
    let continuation = if overflow {
        Some(CandidateContinuation {
            repository_id: repository_id.clone(),
            lifecycle,
            labels,
            boundary: boundary.expect("overflowing candidate page has a boundary"),
            after: after.expect("overflowing candidate page returned at least one row"),
            backend_cursor: None,
        })
    } else {
        None
    };
    let items = positioned
        .into_iter()
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    Ok(CandidatePage {
        returned_count: items.len(),
        items,
        continuation,
        exhausted: !overflow,
        overflow,
        raw_count,
    })
}

/// Consolidated issue candidate query.
///
/// Candidate queries default to an unfiltered open lifecycle bucket and
/// summary detail. Runtime planners use `Unfiltered` only for open default-kind
/// intake; terminal planning always supplies bounded `AnyOf` interest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct IssueCandidateQuery {
    pub lifecycle: CandidateLifecycle,
    pub labels: CandidateLabelSelection,
    #[serde(default = "ItemListDetails::summary")]
    pub details: ItemListDetails,
    /// Bounded traversal. `None` preserves exhaustive open discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<CandidatePageRequest<IssueId>>,
}

impl Default for IssueCandidateQuery {
    fn default() -> Self {
        Self {
            lifecycle: CandidateLifecycle::Open,
            labels: CandidateLabelSelection::Unfiltered,
            details: ItemListDetails::summary(),
            page: None,
        }
    }
}

/// Consolidated pull-request candidate query.
///
/// `Terminal` covers closed and merged pull requests. Defaults otherwise match
/// [`IssueCandidateQuery`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PullRequestCandidateQuery {
    pub lifecycle: CandidateLifecycle,
    pub labels: CandidateLabelSelection,
    #[serde(default = "ItemListDetails::summary")]
    pub details: ItemListDetails,
    /// Bounded traversal. `None` preserves exhaustive open discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<CandidatePageRequest<PullRequestId>>,
}

impl Default for PullRequestCandidateQuery {
    fn default() -> Self {
        Self {
            lifecycle: CandidateLifecycle::Open,
            labels: CandidateLabelSelection::Unfiltered,
            details: ItemListDetails::summary(),
            page: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Row {
        id: IssueId,
        number: ItemNumber,
        updated_at: DateTime<Utc>,
    }

    fn row(id: &str, number: u64, updated_at: &str) -> Row {
        Row {
            id: IssueId::new(id),
            number: ItemNumber::new(number),
            updated_at: updated_at.parse().expect("test timestamp"),
        }
    }

    fn page(
        rows: Vec<Row>,
        raw_count: usize,
        repo: &RepositoryId,
        request: CandidatePageRequest<IssueId>,
    ) -> CandidatePage<Row, IssueId> {
        paginate_candidate_items(
            rows,
            raw_count,
            repo,
            CandidateLifecycle::Terminal,
            CandidateLabelSelection::AnyOf(vec!["recover".to_string()]),
            Some(request),
            |row| CandidatePosition {
                updated_at: row.updated_at,
                number: row.number,
                id: row.id.clone(),
            },
        )
        .expect("candidate page")
    }

    #[test]
    fn continuation_is_stable_across_ties_duplicates_and_newer_additions() {
        let repo = RepositoryId::new("forge:acme/widgets");
        let tied = "2026-01-01T00:00:00Z";
        let later = "2026-01-02T00:00:00Z";
        let original = vec![
            row("issue-2", 2, tied),
            row("issue-1", 1, tied),
            row("issue-2", 2, tied),
            row("issue-3", 3, later),
        ];

        let first = page(
            original.clone(),
            original.len(),
            &repo,
            CandidatePageRequest::first(2),
        );
        assert_eq!(
            first
                .items
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["issue-1", "issue-2"]
        );
        assert_eq!(first.raw_count, 4);
        assert_eq!(first.returned_count, 2);
        assert!(first.overflow);
        assert!(!first.exhausted);

        let mut with_concurrent_newer = original;
        with_concurrent_newer.push(row("issue-4", 4, "2026-01-03T00:00:00Z"));
        let second = page(
            with_concurrent_newer.clone(),
            with_concurrent_newer.len(),
            &repo,
            CandidatePageRequest {
                limit: 2,
                continuation: first.continuation,
            },
        );
        assert_eq!(
            second
                .items
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["issue-3"]
        );
        assert_eq!(second.returned_count, 1);
        assert!(second.exhausted);
        assert!(!second.overflow);
        assert!(second.continuation.is_none());
    }

    #[test]
    fn terminal_default_is_bounded_while_open_without_page_is_exhaustive() {
        let repo = RepositoryId::new("forge:acme/widgets");
        let rows = (1..=101)
            .map(|number| row(&format!("issue-{number}"), number, "2026-01-01T00:00:00Z"))
            .collect::<Vec<_>>();
        let terminal = paginate_candidate_items(
            rows.clone(),
            rows.len(),
            &repo,
            CandidateLifecycle::Terminal,
            CandidateLabelSelection::AnyOf(vec!["recover".to_string()]),
            None,
            |row| CandidatePosition {
                updated_at: row.updated_at,
                number: row.number,
                id: row.id.clone(),
            },
        )
        .expect("default terminal page");
        assert_eq!(
            terminal.returned_count,
            DEFAULT_TERMINAL_CANDIDATE_PAGE_SIZE
        );
        assert!(terminal.overflow);
        assert!(terminal.continuation.is_some());

        let open = paginate_candidate_items(
            rows.clone(),
            rows.len(),
            &repo,
            CandidateLifecycle::Open,
            CandidateLabelSelection::AnyOf(vec!["recover".to_string()]),
            None,
            |row| CandidatePosition {
                updated_at: row.updated_at,
                number: row.number,
                id: row.id.clone(),
            },
        )
        .expect("exhaustive open candidates");
        assert_eq!(open.returned_count, rows.len());
        assert!(open.exhausted);
        assert!(!open.overflow);
    }

    #[test]
    fn continuation_is_bound_to_repository_and_normalized_query() {
        let repo = RepositoryId::new("forge:acme/widgets");
        let rows = vec![
            row("issue-1", 1, "2026-01-01T00:00:00Z"),
            row("issue-2", 2, "2026-01-02T00:00:00Z"),
        ];
        let first = page(rows.clone(), 2, &repo, CandidatePageRequest::first(1));
        let error = paginate_candidate_items(
            rows,
            2,
            &RepositoryId::new("forge:acme/other"),
            CandidateLifecycle::Terminal,
            CandidateLabelSelection::AnyOf(vec!["recover".to_string()]),
            Some(CandidatePageRequest {
                limit: 1,
                continuation: first.continuation,
            }),
            |row| CandidatePosition {
                updated_at: row.updated_at,
                number: row.number,
                id: row.id.clone(),
            },
        )
        .expect_err("cross-repository continuation must fail");
        assert!(matches!(error, ForgeError::InvalidRequest(_)));
    }
}
