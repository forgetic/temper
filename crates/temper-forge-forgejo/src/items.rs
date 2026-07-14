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
//! `current − remove + add`. Forgejo's issue-label endpoints key on the numeric
//! label id (a name array is rejected), so every label path resolves names to
//! ids through one `repo_label_ids` read.

use crate::ids::RepoCoord;
use crate::map::map_comment;
use crate::types::{CommentDto, LabelDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use std::collections::HashMap;
use temper_forge_model::{Comment, ForgeResult, ItemNumber, UserId};

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
        let key = repo.path_segment();
        if let Some(ids) = self
            .label_ids
            .lock()
            .expect("label id cache mutex poisoned")
            .get(&key)
            .cloned()
        {
            return Ok(ids);
        }

        let path = format!("/repos/{key}/labels");
        let dtos: Vec<LabelDto> = self.list_all("list labels", &path, Vec::new()).await?;
        let ids = dtos
            .into_iter()
            .map(|label| (label.name, label.id))
            .collect::<HashMap<_, _>>();
        self.label_ids
            .lock()
            .expect("label id cache mutex poisoned")
            .insert(key, ids.clone());
        Ok(ids)
    }

    /// Applies a label update through Forgejo's issue label endpoints.
    ///
    /// `set_labels` replaces the full set (`PUT`), then `remove_labels` are
    /// deleted (`DELETE`, tolerating a missing label), then `add_labels` are
    /// appended (`POST`). Artifact update paths coalesce mixed incremental
    /// changes with [`coalesce_label_update`] so workflow state flips use one
    /// provider mutation and are never externally visible half-applied.
    ///
    /// Forgejo's issue label endpoints key on the **numeric label id**, not the
    /// name (`PUT`/`POST` reject a name array with `422 cannot unmarshal … into
    /// int64`). So every path resolves names to ids through a single
    /// [`Self::repo_label_ids`] read (issued only when there is work to do) and
    /// sends ids. A name with no matching repository label is skipped: the
    /// workflow upserts its labels before applying them, so an unresolved name
    /// is a caller error rather than something to invent an id for.
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

        // Nothing to do means no label-id read at all.
        if set_labels.is_none() && add_labels.is_empty() && remove_labels.is_empty() {
            return Ok(());
        }
        let ids = self.repo_label_ids(repo).await?;

        if let Some(set_labels) = set_labels {
            let set_ids = resolve_label_ids(&ids, &set_labels);
            let payload = serde_json::json!({ "labels": set_ids }).to_string();
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
            if let Some(id) = ids.get(name) {
                let path = format!("{base}/{id}");
                // A label that is not attached returns 404; that is a no-op.
                self.request_optional("remove label", HttpMethod::Delete, &path, Vec::new(), None)
                    .await?;
            }
        }

        if !add_labels.is_empty() {
            let add_ids = resolve_label_ids(&ids, &add_labels);
            let payload = serde_json::json!({ "labels": add_ids }).to_string();
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

/// Coalesces mixed set/add/remove label operations into one replacement set.
///
/// Forgejo's separate add and remove endpoints expose intermediate label sets
/// to concurrent queue scans. Computing the final set from the artifact read by
/// the update path lets mixed workflow state flips use one `PUT`. Pure adds and
/// pure removes retain their incremental provider operation so they do not
/// overwrite unrelated concurrent label changes.
pub(crate) fn coalesce_label_update(
    current: &[String],
    set_labels: Option<Vec<String>>,
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
) -> (Option<Vec<String>>, Vec<String>, Vec<String>) {
    if set_labels.is_none() && (add_labels.is_empty() || remove_labels.is_empty()) {
        return (None, add_labels, remove_labels);
    }

    let remove_labels = sorted_dedup(remove_labels);
    let mut labels = sorted_dedup(set_labels.unwrap_or_else(|| current.to_vec()));
    labels.retain(|label| !remove_labels.contains(label));
    labels.extend(add_labels);
    (Some(sorted_dedup(labels)), Vec::new(), Vec::new())
}

fn sorted_dedup(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

/// Maps label names to their numeric provider ids, dropping unknown names.
///
/// Forgejo's `PUT`/`POST` issue-label endpoints take an `int64` id array, so a
/// name with no matching repository label cannot be expressed and is skipped
/// (the workflow upserts labels before applying them). `names` is expected to
/// already be sorted and deduplicated, so the returned ids preserve that order.
fn resolve_label_ids(ids: &HashMap<String, u64>, names: &[String]) -> Vec<u64> {
    names
        .iter()
        .filter_map(|name| ids.get(name).copied())
        .collect()
}

fn sorted_dedup_users(mut values: Vec<UserId>) -> Vec<UserId> {
    values.sort();
    values.dedup();
    values
}
