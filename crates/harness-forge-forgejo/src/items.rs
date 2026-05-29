//! Shared issue/pull-request item helpers built on Forgejo's issue endpoints.
//!
//! Forgejo models pull requests as issues, so comments, labels, and assignees on
//! a pull request use the same `/issues/{number}` endpoints as issues. These
//! helpers are keyed by [`ItemNumber`] and shared by [`crate::pulls`]; the
//! issues phase reuses them so the label/assignee sequencing lives in one place.
//!
//! Sequencing follows the portable contract (see
//! `docs/reference/forge-interface.md`): label updates apply `set_labels`, then
//! removals, then additions; assignee changes are a set computed as
//! `current − remove + add`. Removals go through the numeric-label-id delete
//! endpoint, so this module resolves label names to ids on demand.

use crate::ids::RepoCoord;
use crate::map::map_comment;
use crate::types::{CommentDto, LabelDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use harness_forge::{Comment, ForgeResult, ItemNumber, UserId};
use std::collections::HashMap;

impl<C: HttpClient> ForgejoForge<C> {
    /// Lists comments on an issue or pull request in chronological order.
    pub(crate) async fn list_item_comments(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Vec<Comment>> {
        let path = format!(
            "/repos/{}/issues/{}/comments",
            repo.path_segment(),
            number.get()
        );
        let dtos: Vec<CommentDto> = self.list_all("list comments", &path, Vec::new()).await?;
        let mut comments: Vec<Comment> =
            dtos.into_iter().map(|dto| map_comment(repo, dto)).collect();
        comments.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(comments)
    }

    /// Adds a comment to an issue or pull request and returns the mapped comment.
    pub(crate) async fn add_item_comment(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
        body: String,
    ) -> ForgeResult<Comment> {
        let path = format!(
            "/repos/{}/issues/{}/comments",
            repo.path_segment(),
            number.get()
        );
        let payload = serde_json::json!({ "body": body }).to_string();
        let response = self
            .request_checked(
                "add comment",
                HttpMethod::Post,
                &path,
                Vec::new(),
                Some(payload),
            )
            .await?;
        let dto: CommentDto = Self::decode("add comment", &response)?;
        Ok(map_comment(repo, dto))
    }

    /// Resolves repository label names to their numeric provider ids.
    pub(crate) async fn repo_label_ids(
        &self,
        repo: &RepoCoord,
    ) -> ForgeResult<HashMap<String, u64>> {
        let path = format!("/repos/{}/labels", repo.path_segment());
        let dtos: Vec<LabelDto> = self.list_all("list labels", &path, Vec::new()).await?;
        Ok(dtos
            .into_iter()
            .map(|label| (label.name, label.id))
            .collect())
    }

    /// Applies a label update through Forgejo's issue label endpoints.
    ///
    /// `set_labels` replaces the full set (`PUT`), then `remove_labels` are
    /// deleted by numeric id (`DELETE`, tolerating a missing label), then
    /// `add_labels` are appended (`POST`).
    pub(crate) async fn apply_item_label_update(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
        set_labels: Option<Vec<String>>,
        add_labels: Vec<String>,
        remove_labels: Vec<String>,
    ) -> ForgeResult<()> {
        let base = format!(
            "/repos/{}/issues/{}/labels",
            repo.path_segment(),
            number.get()
        );

        if let Some(set_labels) = set_labels {
            let payload = serde_json::json!({ "labels": sorted_dedup(set_labels) }).to_string();
            self.request_checked(
                "set labels",
                HttpMethod::Put,
                &base,
                Vec::new(),
                Some(payload),
            )
            .await?;
        }

        let remove_labels = sorted_dedup(remove_labels);
        if !remove_labels.is_empty() {
            let ids = self.repo_label_ids(repo).await?;
            for name in &remove_labels {
                if let Some(id) = ids.get(name) {
                    let path = format!("{base}/{id}");
                    // A label that is not attached returns 404; that is a no-op.
                    self.request_optional(
                        "remove label",
                        HttpMethod::Delete,
                        &path,
                        Vec::new(),
                        None,
                    )
                    .await?;
                }
            }
        }

        let add_labels = sorted_dedup(add_labels);
        if !add_labels.is_empty() {
            let payload = serde_json::json!({ "labels": add_labels }).to_string();
            self.request_checked(
                "add labels",
                HttpMethod::Post,
                &base,
                Vec::new(),
                Some(payload),
            )
            .await?;
        }

        Ok(())
    }

    /// Applies an assignee update by patching the issue with the full set.
    ///
    /// Forgejo's issue patch replaces the assignee set, so the new set is
    /// computed as `current − remove + add` (sorted, deduplicated). A no-op
    /// (no additions or removals) skips the request entirely.
    pub(crate) async fn apply_item_assignee_update(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
        current: &[UserId],
        add_assignees: Vec<UserId>,
        remove_assignees: Vec<UserId>,
    ) -> ForgeResult<()> {
        if add_assignees.is_empty() && remove_assignees.is_empty() {
            return Ok(());
        }

        let remove: Vec<UserId> = sorted_dedup_users(remove_assignees);
        let mut set: Vec<UserId> = sorted_dedup_users(current.to_vec());
        set.retain(|user| !remove.contains(user));
        for user in sorted_dedup_users(add_assignees) {
            if !set.contains(&user) {
                set.push(user);
            }
        }
        set.sort();

        let logins: Vec<&str> = set.iter().map(UserId::as_str).collect();
        let path = format!("/repos/{}/issues/{}", repo.path_segment(), number.get());
        let payload = serde_json::json!({ "assignees": logins }).to_string();
        self.request_checked(
            "update assignees",
            HttpMethod::Patch,
            &path,
            Vec::new(),
            Some(payload),
        )
        .await?;
        Ok(())
    }
}

fn sorted_dedup(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn sorted_dedup_users(mut values: Vec<UserId>) -> Vec<UserId> {
    values.sort();
    values.dedup();
    values
}
