//! Provider-specific issue-index reads shared by issue and pull candidates.

use crate::ids::RepoCoord;
use crate::types::IssueDto;
use crate::{ForgejoForge, HttpClient};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use temper_forge_model::{
    CandidateContinuation, CandidateLabelSelection, CandidateLifecycle, CandidatePage,
    CandidatePageRequest, CandidatePosition, DEFAULT_TERMINAL_CANDIDATE_PAGE_SIZE, ForgeError,
    ForgeResult, ItemNumber, MAX_CANDIDATE_PAGE_SIZE, paginate_candidate_items,
};

/// Hard ceiling on provider requests made by one bounded candidate page.
///
/// The ceiling covers pagination needed to move through duplicate rows and
/// equal timestamps. It is independent of terminal-history cardinality.
pub const MAX_CANDIDATE_PROVIDER_REQUESTS: usize = 64;

/// Provider-request ceiling for one default periodic terminal candidate bucket.
///
/// In addition to bounded list traversal, every row in a retained PR page may
/// require one exact summary read when Forgejo omits its closed/merged marker.
/// Issue buckets and unambiguous PR pages remain below this worst-case bound.
pub const MAX_PERIODIC_TERMINAL_CANDIDATE_PROVIDER_REQUESTS: usize =
    MAX_CANDIDATE_PROVIDER_REQUESTS + DEFAULT_TERMINAL_CANDIDATE_PAGE_SIZE;

/// Hard ceiling on decoded provider rows considered by one bounded page.
/// One look-ahead row is enough to establish portable overflow.
pub const MAX_CANDIDATE_PROVIDER_ROWS: usize = MAX_CANDIDATE_PAGE_SIZE + 1;

/// Maximum any-label streams accepted by the repository-isolated protocol.
/// Each stream uses the exact repository endpoint, so a sibling repository can
/// consume neither its row budget nor its request budget.
pub const MAX_CANDIDATE_LABEL_STREAMS: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ForgejoCandidateCursor {
    streams: Vec<ForgejoCandidateStreamCursor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ForgejoCandidateStreamCursor {
    label: Option<String>,
    /// Inclusive timestamp filter against which `page` is numbered.
    /// Provider page numbers cannot be reused after this value advances.
    since: Option<DateTime<Utc>>,
    page: u32,
}

struct ProviderStream<I> {
    label: Option<String>,
    since: Option<DateTime<Utc>>,
    next_page: u32,
    exhausted: bool,
    batches: Vec<ProviderBatch<I>>,
}

struct ProviderBatch<I> {
    page: u32,
    short: bool,
    positions: Vec<CandidatePosition<I>>,
}

impl<C: HttpClient> ForgejoForge<C> {
    /// Reads one lifecycle/type bucket with deterministic provider ordering.
    ///
    /// Unbounded open discovery retains exhaustive level-triggered behavior.
    /// Every terminal or explicitly paged query uses a fixed request and row
    /// ceiling. Multi-label any-of is implemented as bounded single-label
    /// streams on the exact repository endpoint: unlike owner search, foreign
    /// repositories cannot fill a page before local filtering.
    pub(crate) async fn list_candidate_issue_rows<I, F>(
        &self,
        repo: &RepoCoord,
        lifecycle: CandidateLifecycle,
        item_type: &str,
        labels: Option<&[String]>,
        page: Option<CandidatePageRequest<I>>,
        id: F,
    ) -> ForgeResult<CandidatePage<IssueDto, I>>
    where
        I: Clone + Ord,
        F: Fn(ItemNumber) -> I + Copy,
    {
        let repo_id = crate::ids::format_repository_id(repo);
        let normalized_selection = labels.map_or(CandidateLabelSelection::Unfiltered, |labels| {
            CandidateLabelSelection::AnyOf(labels.to_vec())
        });
        let request = page.or_else(|| {
            (lifecycle == CandidateLifecycle::Terminal).then(CandidatePageRequest::terminal)
        });
        if let Some(request) = &request {
            request.validate()?;
            validate_continuation_scope(request, &repo_id, lifecycle, &normalized_selection)?;
        }

        let streams = candidate_label_streams(labels)?;
        let state = match lifecycle {
            CandidateLifecycle::Open => "open",
            CandidateLifecycle::Terminal => "closed",
        };
        let path = format!("/repos/{}/issues", repo.path_segment());

        let Some(request) = request else {
            let mut rows = Vec::new();
            for label in streams {
                let query = candidate_query(state, item_type, label.as_deref(), None, None);
                rows.extend(
                    self.list_all("list candidate issue index", &path, query)
                        .await?,
                );
            }
            let raw_count = rows.len();
            return paginate_candidate_items(
                rows,
                raw_count,
                &repo_id,
                lifecycle,
                normalized_selection,
                None,
                |row| row_position(row, id),
            );
        };

        let first_page = request.continuation.is_none();
        let sweep_boundary = request
            .continuation
            .as_ref()
            .map(|continuation| continuation.boundary.clone())
            .unwrap_or_else(|| CandidatePosition {
                // Forgejo timestamps cannot legitimately be ahead of the
                // request clock. Maximal tie-breaks include every row at this
                // instant while freezing later updates out of the sweep.
                updated_at: Utc::now(),
                number: ItemNumber::new(u64::MAX),
                id: id(ItemNumber::new(u64::MAX)),
            });
        let after = request
            .continuation
            .as_ref()
            .map(|continuation| continuation.after.clone());
        let cursor = decode_cursor(&request, &streams)?;
        let mut provider_streams = streams
            .into_iter()
            .zip(cursor)
            .map(|(label, cursor)| ProviderStream {
                label,
                since: cursor.since,
                next_page: cursor.page,
                exhausted: false,
                batches: Vec::new(),
            })
            .collect::<Vec<_>>();

        let provider_limit = (self.config().page_limit.max(1) as usize)
            .min((MAX_CANDIDATE_PROVIDER_ROWS / provider_streams.len()).max(1));
        let mut rows = Vec::<IssueDto>::new();
        let mut requests = 0usize;
        loop {
            if requests >= MAX_CANDIDATE_PROVIDER_REQUESTS
                || rows.len() >= MAX_CANDIDATE_PROVIDER_ROWS
            {
                break;
            }
            let positioned = unique_after_positions(&rows, after.as_ref(), &sweep_boundary, id);
            let kth = positioned.get(request.limit.saturating_sub(1));
            let streams_to_fetch = provider_streams
                .iter()
                .enumerate()
                .filter(|(_, stream)| !stream.exhausted)
                .filter(|(_, stream)| {
                    stream.batches.is_empty()
                        || kth.is_none_or(|kth| {
                            stream_frontier(stream).is_none_or(|frontier| frontier < kth)
                        })
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if streams_to_fetch.is_empty() {
                break;
            }

            let mut fetched = false;
            for index in streams_to_fetch {
                if requests >= MAX_CANDIDATE_PROVIDER_REQUESTS
                    || rows.len() >= MAX_CANDIDATE_PROVIDER_ROWS
                {
                    break;
                }
                let stream = &mut provider_streams[index];
                let remaining = MAX_CANDIDATE_PROVIDER_ROWS.saturating_sub(rows.len());
                let limit = provider_limit.min(remaining);
                let page_number = stream.next_page;
                let query = candidate_query(
                    state,
                    item_type,
                    stream.label.as_deref(),
                    stream.since,
                    Some(sweep_boundary.updated_at),
                );
                let mut batch: Vec<IssueDto> = self
                    .list_page(
                        "list bounded candidate issue index",
                        &path,
                        query,
                        limit,
                        page_number,
                    )
                    .await?;
                requests = requests.saturating_add(1);
                fetched = true;
                // Enforce the local decoded-row ceiling even if a provider
                // ignores its requested page size.
                batch.truncate(limit);
                let short = batch.len() < limit;
                let positions = batch.iter().map(|row| row_position(row, id)).collect();
                rows.extend(batch);
                stream.batches.push(ProviderBatch {
                    page: page_number,
                    short,
                    positions,
                });
                if short {
                    stream.exhausted = true;
                } else {
                    stream.next_page = stream.next_page.saturating_add(1);
                }
            }
            if !fetched {
                break;
            }
        }

        let provider_exhausted = provider_streams.iter().all(|stream| stream.exhausted);
        let raw_count = rows.len();
        let positioned = unique_after_positions(&rows, after.as_ref(), &sweep_boundary, id);
        let page_is_safe = if let Some(kth) = positioned.get(request.limit.saturating_sub(1)) {
            provider_streams.iter().all(|stream| {
                stream.exhausted || stream_frontier(stream).is_some_and(|frontier| frontier >= kth)
            })
        } else {
            provider_exhausted
        };
        if !page_is_safe {
            let safe_frontier = provider_streams
                .iter()
                .filter(|stream| !stream.exhausted)
                .filter_map(stream_frontier)
                .min()
                .cloned()
                .ok_or_else(|| {
                    ForgeError::Backend(
                        "bounded Forgejo candidate page exhausted its budget before every label stream advanced"
                            .to_string(),
                    )
                })?;
            rows.retain(|row| row_position(row, id) <= safe_frontier);
        }

        let mut page = paginate_candidate_items(
            rows,
            raw_count,
            &repo_id,
            lifecycle,
            normalized_selection.clone(),
            Some(request),
            |row| row_position(row, id),
        )?;

        // The portable helper sees only the bounded provider window. A full
        // stream or exhausted local budget is still overflow when provider
        // duplicates left no look-ahead unique row in that window.
        if !provider_exhausted && !page.overflow {
            let Some(last) = page.items.last().map(|row| row_position(row, id)) else {
                return Err(ForgeError::Backend(
                    "bounded Forgejo candidate page made no timestamp/identity progress"
                        .to_string(),
                ));
            };
            page.continuation = Some(CandidateContinuation {
                repository_id: repo_id.clone(),
                lifecycle,
                labels: normalized_selection,
                boundary: sweep_boundary.clone(),
                after: last,
                backend_cursor: None,
            });
            page.exhausted = false;
            page.overflow = true;
        }

        if let Some(continuation) = &mut page.continuation {
            if first_page {
                continuation.boundary = sweep_boundary;
            }
            let cursor = cursor_after(&provider_streams, &continuation.after);
            continuation.backend_cursor =
                Some(serde_json::to_string(&cursor).map_err(|error| {
                    ForgeError::Backend(format!("encode Forgejo candidate continuation: {error}"))
                })?);
        }
        Ok(page)
    }
}

fn validate_continuation_scope<I: Ord>(
    request: &CandidatePageRequest<I>,
    repository_id: &temper_forge_model::RepositoryId,
    lifecycle: CandidateLifecycle,
    labels: &CandidateLabelSelection,
) -> ForgeResult<()> {
    let Some(continuation) = &request.continuation else {
        return Ok(());
    };
    if continuation.repository_id != *repository_id
        || continuation.lifecycle != lifecycle
        || continuation.labels != *labels
        || continuation.after > continuation.boundary
    {
        return Err(ForgeError::InvalidRequest(
            "candidate continuation does not match repository or normalized query scope"
                .to_string(),
        ));
    }
    Ok(())
}

fn candidate_label_streams(labels: Option<&[String]>) -> ForgeResult<Vec<Option<String>>> {
    match labels {
        None => Ok(vec![None]),
        Some(labels) if labels.len() <= MAX_CANDIDATE_LABEL_STREAMS => {
            Ok(labels.iter().cloned().map(Some).collect())
        }
        Some(labels) => Err(ForgeError::InvalidRequest(format!(
            "Forgejo candidate any-of supports at most {MAX_CANDIDATE_LABEL_STREAMS} labels, got {}",
            labels.len()
        ))),
    }
}

fn candidate_query(
    state: &str,
    item_type: &str,
    label: Option<&str>,
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> Vec<(String, String)> {
    let mut query = vec![
        ("state".to_string(), state.to_string()),
        ("type".to_string(), item_type.to_string()),
        ("sort".to_string(), "updated".to_string()),
        ("direction".to_string(), "asc".to_string()),
    ];
    if let Some(label) = label {
        query.push(("labels".to_string(), label.to_string()));
    }
    if let Some(since) = since {
        query.push(("since".to_string(), since.to_rfc3339()));
    }
    if let Some(before) = before {
        query.push(("before".to_string(), before.to_rfc3339()));
    }
    query
}

fn row_position<I, F>(row: &IssueDto, id: F) -> CandidatePosition<I>
where
    F: Fn(ItemNumber) -> I,
{
    let number = ItemNumber::new(row.number);
    CandidatePosition {
        updated_at: row.updated_at,
        number,
        id: id(number),
    }
}

fn unique_after_positions<I, F>(
    rows: &[IssueDto],
    after: Option<&CandidatePosition<I>>,
    boundary: &CandidatePosition<I>,
    id: F,
) -> Vec<CandidatePosition<I>>
where
    I: Clone + Ord,
    F: Fn(ItemNumber) -> I + Copy,
{
    let mut unique = BTreeMap::new();
    for row in rows {
        let position = row_position(row, id);
        if &position <= boundary && after.is_none_or(|after| &position > after) {
            unique.entry(position.id.clone()).or_insert(position);
        }
    }
    let mut positioned = unique.into_values().collect::<Vec<_>>();
    positioned.sort();
    positioned
}

fn stream_frontier<I: Ord>(stream: &ProviderStream<I>) -> Option<&CandidatePosition<I>> {
    stream
        .batches
        .last()
        .and_then(|batch| batch.positions.iter().max())
}

fn decode_cursor<I>(
    request: &CandidatePageRequest<I>,
    streams: &[Option<String>],
) -> ForgeResult<Vec<ForgejoCandidateStreamCursor>> {
    let Some(encoded) = request
        .continuation
        .as_ref()
        .and_then(|continuation| continuation.backend_cursor.as_deref())
    else {
        let since = request
            .continuation
            .as_ref()
            .map(|continuation| continuation.after.updated_at);
        return Ok(streams
            .iter()
            .map(|label| ForgejoCandidateStreamCursor {
                label: label.clone(),
                since,
                page: 1,
            })
            .collect());
    };
    let cursor: ForgejoCandidateCursor = serde_json::from_str(encoded).map_err(|_| {
        ForgeError::InvalidRequest("invalid Forgejo candidate backend cursor".to_string())
    })?;
    if cursor.streams.len() != streams.len()
        || cursor.streams.iter().zip(streams).any(|(cursor, label)| {
            &cursor.label != label
                || cursor.page == 0
                || request
                    .continuation
                    .as_ref()
                    .is_none_or(|continuation| cursor.since != Some(continuation.after.updated_at))
        })
    {
        return Err(ForgeError::InvalidRequest(
            "Forgejo candidate backend cursor does not match label streams".to_string(),
        ));
    }
    Ok(cursor.streams)
}

fn cursor_after<I: Ord>(
    streams: &[ProviderStream<I>],
    after: &CandidatePosition<I>,
) -> ForgejoCandidateCursor {
    ForgejoCandidateCursor {
        streams: streams
            .iter()
            .map(|stream| {
                let since = Some(after.updated_at);
                let mut page = 1;
                // `since` changes the provider result set and therefore resets
                // page numbering. Reuse offsets only while moving through one
                // equal-timestamp tie under the same inclusive filter.
                if stream.since == since {
                    page = stream
                        .batches
                        .first()
                        .map_or(stream.next_page, |batch| batch.page);
                    for batch in &stream.batches {
                        if batch.positions.iter().all(|position| position <= after) {
                            page = batch.page.saturating_add(1);
                            if batch.short {
                                break;
                            }
                        } else {
                            page = batch.page;
                            break;
                        }
                    }
                }
                ForgejoCandidateStreamCursor {
                    label: stream.label.clone(),
                    since,
                    page,
                }
            })
            .collect(),
    }
}
