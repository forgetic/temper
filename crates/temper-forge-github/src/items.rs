//! Shared issue/pull-request item helpers built on GitHub's issue endpoints.
//!
//! GitHub models pull requests as issues, so comments, labels, and assignees on
//! a pull request use the same `/issues/{number}` endpoints as issues. These
//! helpers are keyed by [`ItemNumber`] and shared by [`crate::pulls`] and
//! [`crate::issues`] so the label/assignee sequencing lives in one place.
//!
//! Sequencing follows the portable contract (see
//! `docs/reference/forge-interface.md`): label updates apply `set_labels`, then
//! removals, then additions; assignee changes are a set computed as
//! `current − remove + add`. Unlike Forgejo, GitHub's issue-label endpoints key
//! on the label **name**, so no name→id resolution read is needed.

use crate::ids::RepoCoord;
use crate::map::map_comment;
use crate::types::CommentDto;
use crate::{GitHubForge, HttpClient, HttpMethod};
use temper_forge::{Comment, ForgeResult, ItemNumber, UserId};

impl<C: HttpClient> GitHubForge<C> {
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

    /// Applies a label update through GitHub's issue label endpoints.
    ///
    /// `set_labels` replaces the full set (`PUT`), then `remove_labels` are
    /// deleted (`DELETE …/labels/{name}`, tolerating a label that is not
    /// attached), then `add_labels` are appended (`POST`). GitHub keys these
    /// endpoints on the label name, and `PUT`/`POST` auto-create labels that do
    /// not yet exist in the repository, so no name→id resolution is required.
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

        let set_labels = set_labels.map(sorted_dedup);
        let add_labels = sorted_dedup(add_labels);
        let remove_labels = sorted_dedup(remove_labels);

        if let Some(set_labels) = set_labels {
            let payload = serde_json::json!({ "labels": set_labels }).to_string();
            self.request_checked(
                "set labels",
                HttpMethod::Put,
                &base,
                Vec::new(),
                Some(payload),
            )
            .await?;
        }

        for name in &remove_labels {
            let path = format!("{base}/{name}");
            // A label that is not attached returns 404; that is a no-op.
            self.request_optional("remove label", HttpMethod::Delete, &path, Vec::new(), None)
                .await?;
        }

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
    /// GitHub's issue patch replaces the assignee set, so the new set is
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
