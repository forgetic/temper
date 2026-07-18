//! Provider-specific issue-index reads shared by issue and pull candidates.

use crate::ids::RepoCoord;
use crate::types::IssueDto;
use crate::{ForgejoForge, HttpClient};
use temper_forge_model::ForgeResult;

impl<C: HttpClient> ForgejoForge<C> {
    /// Reads one candidate lifecycle/type bucket with Forgejo's real label
    /// semantics. Multi-label search responses are owner-scoped, so their
    /// embedded repository identity is required and checked before mapping.
    pub(crate) async fn list_candidate_issue_rows(
        &self,
        repo: &RepoCoord,
        state: &str,
        item_type: &str,
        labels: Option<&[String]>,
    ) -> ForgeResult<Vec<IssueDto>> {
        let owner_scoped = labels.is_some_and(|labels| labels.len() > 1);
        let (path, mut query) = if owner_scoped {
            (
                "/repos/issues/search".to_string(),
                vec![("owner".to_string(), repo.owner.clone())],
            )
        } else {
            (format!("/repos/{}/issues", repo.path_segment()), Vec::new())
        };
        query.push(("state".to_string(), state.to_string()));
        query.push(("type".to_string(), item_type.to_string()));
        if let Some(labels) = labels {
            query.push(("labels".to_string(), labels.join(",")));
        }
        let rows: Vec<IssueDto> = self
            .list_all("list candidate issue index", &path, query)
            .await?;
        if owner_scoped {
            Ok(rows
                .into_iter()
                .filter(|row| row.is_in_repository(repo))
                .collect())
        } else {
            Ok(rows)
        }
    }
}
