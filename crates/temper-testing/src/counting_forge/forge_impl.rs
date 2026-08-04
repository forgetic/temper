use super::*;

#[async_trait]
impl<F: Forge> Forge for CountingForge<F> {
    fn item_number_namespace(&self) -> ItemNumberNamespace {
        self.item_number_namespace
            .unwrap_or_else(|| self.inner.item_number_namespace())
    }

    fn provider_request_count(&self) -> Option<u64> {
        self.inner
            .provider_request_count()
            .or_else(|| Some(u64::try_from(self.operations.total_count()).unwrap_or(u64::MAX)))
    }

    async fn current_user(&self) -> ForgeResult<User> {
        self.perform(CountedForgeOp::CurrentUser, self.inner.current_user())
            .await
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        self.perform(CountedForgeOp::GetUser, self.inner.get_user(id))
            .await
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        self.perform(
            CountedForgeOp::ListRepositories,
            self.inner.list_repositories(query),
        )
        .await
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        self.perform(
            CountedForgeOp::CreateRepository,
            self.inner.create_repository(input),
        )
        .await
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        self.perform(CountedForgeOp::GetRepository, self.inner.get_repository(id))
            .await
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        self.perform(
            CountedForgeOp::GetRepositoryByPath,
            self.inner.get_repository_by_path(path),
        )
        .await
    }

    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        self.perform(CountedForgeOp::ListLabels, self.inner.list_labels(repo_id))
            .await
    }

    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label> {
        self.perform(
            CountedForgeOp::UpsertLabel,
            self.inner.upsert_label(repo_id, input),
        )
        .await
    }

    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        self.record_issue_query(&query);
        self.perform(
            CountedForgeOp::ListIssues,
            self.inner.list_issues(repo_id, query),
        )
        .await
    }

    async fn list_issue_candidates(
        &self,
        repo_id: &RepositoryId,
        query: IssueCandidateQuery,
    ) -> ForgeResult<IssueCandidatePage> {
        self.record_issue_candidate_query(&query);
        self.perform(CountedForgeOp::ListIssueCandidates, async {
            let overrides = self
                .issue_candidate_overrides
                .lock()
                .expect("issue candidate overrides mutex")
                .clone();
            let mut page = self.inner.list_issue_candidates(repo_id, query).await?;
            for issue in &mut page.items {
                if let Some(projected) = overrides.get(&issue.id) {
                    *issue = projected.clone();
                }
            }
            Ok(page)
        })
        .await
    }

    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue> {
        self.perform(
            CountedForgeOp::CreateIssue,
            self.inner.create_issue(repo_id, input),
        )
        .await
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        self.record_exact_issue_read(false, ItemListDetails::full());
        self.perform(CountedForgeOp::GetIssue, self.inner.get_issue(id))
            .await
    }

    async fn get_issue_with_details(
        &self,
        id: &IssueId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        self.record_exact_issue_read(false, details);
        self.perform(
            CountedForgeOp::GetIssue,
            self.inner.get_issue_with_details(id, details),
        )
        .await
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        self.record_exact_issue_read(true, ItemListDetails::full());
        self.perform(
            CountedForgeOp::GetIssueByNumber,
            self.inner.get_issue_by_number(repo_id, number),
        )
        .await
    }

    async fn get_issue_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        self.record_exact_issue_read(true, details);
        self.perform(
            CountedForgeOp::GetIssueByNumber,
            self.inner
                .get_issue_by_number_with_details(repo_id, number, details),
        )
        .await
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        self.perform(
            CountedForgeOp::UpdateIssue,
            self.inner.update_issue(id, input),
        )
        .await
    }

    async fn update_issue_from_snapshot(
        &self,
        current: &Issue,
        input: UpdateIssue,
    ) -> ForgeResult<Issue> {
        self.perform(
            CountedForgeOp::UpdateIssue,
            self.inner.update_issue_from_snapshot(current, input),
        )
        .await
    }

    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue> {
        self.perform(
            CountedForgeOp::AddIssueDependency,
            self.inner.add_issue_dependency(id, target),
        )
        .await
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        self.perform(
            CountedForgeOp::RemoveIssueDependency,
            self.inner.remove_issue_dependency(id, target),
        )
        .await
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        self.perform(
            CountedForgeOp::ListIssueComments,
            self.inner.list_issue_comments(id),
        )
        .await
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        self.perform(
            CountedForgeOp::AddIssueComment,
            self.inner.add_issue_comment(id, input),
        )
        .await
    }

    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        self.record_pull_request_query(&query);
        self.perform(CountedForgeOp::ListPullRequests, async {
            Ok(self
                .inner
                .list_pull_requests(repo_id, query)
                .await?
                .into_iter()
                .map(|pull_request| self.project_pull_request(pull_request))
                .collect())
        })
        .await
    }

    async fn list_pull_request_candidates(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestCandidateQuery,
    ) -> ForgeResult<PullRequestCandidatePage> {
        self.record_pull_request_candidate_query(&query);
        self.perform(CountedForgeOp::ListPullRequestCandidates, async {
            let mut page = self
                .inner
                .list_pull_request_candidates(repo_id, query)
                .await?;
            for pull_request in &mut page.items {
                *pull_request = self.project_pull_request(pull_request.clone());
            }
            Ok(page)
        })
        .await
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.perform(CountedForgeOp::CreatePullRequest, async {
            self.inner
                .create_pull_request(repo_id, input)
                .await
                .map(|pull_request| self.project_pull_request(pull_request))
        })
        .await
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        self.record_exact_pull_request_read(false, ItemListDetails::full());
        self.perform(CountedForgeOp::GetPullRequest, async {
            Ok(self
                .inner
                .get_pull_request(id)
                .await?
                .map(|pull_request| self.project_pull_request(pull_request)))
        })
        .await
    }

    async fn get_pull_request_with_details(
        &self,
        id: &PullRequestId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        self.record_exact_pull_request_read(false, details);
        self.perform(CountedForgeOp::GetPullRequest, async {
            Ok(self
                .inner
                .get_pull_request_with_details(id, details)
                .await?
                .map(|pull_request| self.project_pull_request(pull_request)))
        })
        .await
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        self.record_exact_pull_request_read(true, ItemListDetails::full());
        self.perform(CountedForgeOp::GetPullRequestByNumber, async {
            Ok(self
                .inner
                .get_pull_request_by_number(repo_id, number)
                .await?
                .map(|pull_request| self.project_pull_request(pull_request)))
        })
        .await
    }

    async fn get_pull_request_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        self.record_exact_pull_request_read(true, details);
        self.perform(CountedForgeOp::GetPullRequestByNumber, async {
            Ok(self
                .inner
                .get_pull_request_by_number_with_details(repo_id, number, details)
                .await?
                .map(|pull_request| self.project_pull_request(pull_request)))
        })
        .await
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.perform(CountedForgeOp::UpdatePullRequest, async {
            let updated = self.inner.update_pull_request(id, input.clone()).await?;
            let mut projected = self.project_pull_request(updated.clone());
            if let Some(head) = self.maybe_advance_head_after_update(&input, &updated) {
                projected.head_sha = Some(head);
            }
            Ok(projected)
        })
        .await
    }

    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.perform(
            CountedForgeOp::AddPullRequestDependency,
            self.inner.add_pull_request_dependency(id, target),
        )
        .await
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.perform(
            CountedForgeOp::RemovePullRequestDependency,
            self.inner.remove_pull_request_dependency(id, target),
        )
        .await
    }

    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        self.perform(
            CountedForgeOp::RequestPullRequestReviewers,
            self.inner.request_pull_request_reviewers(id, input),
        )
        .await
    }

    async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        self.perform(
            CountedForgeOp::ListPullRequestReviews,
            self.inner.list_pull_request_reviews(id),
        )
        .await
    }

    async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        self.perform(
            CountedForgeOp::SubmitPullRequestReview,
            self.inner.submit_pull_request_review(id, input),
        )
        .await
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        self.perform(
            CountedForgeOp::ListPullRequestComments,
            self.inner.list_pull_request_comments(id),
        )
        .await
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        self.perform(
            CountedForgeOp::AddPullRequestComment,
            self.inner.add_pull_request_comment(id, input),
        )
        .await
    }

    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        let conflict = self
            .merge_conflicts
            .lock()
            .expect("merge conflicts mutex")
            .get(id)
            .cloned();
        self.perform(CountedForgeOp::MergePullRequest, async move {
            if let Some(message) = conflict {
                Err(ForgeError::Conflict(message))
            } else {
                self.inner.merge_pull_request(id, input).await
            }
        })
        .await
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        self.record_ci_job_query(&query);
        self.perform(
            CountedForgeOp::ListCiJobs,
            self.inner.list_ci_jobs(repo_id, query),
        )
        .await
    }

    async fn list_ci_jobs_with_presence(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<CiJobListing> {
        self.record_ci_job_query(&query);
        self.perform(
            CountedForgeOp::ListCiJobs,
            self.inner.list_ci_jobs_with_presence(repo_id, query),
        )
        .await
    }

    async fn retry_ci_attempt(&self, request: CiRetryRequest) -> ForgeResult<CiRetryOutcome> {
        self.record_ci_retry_request(&request);
        self.perform(
            CountedForgeOp::RetryCiAttempt,
            self.inner.retry_ci_attempt(request),
        )
        .await
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        self.perform(CountedForgeOp::GetCiJob, self.inner.get_ci_job(id))
            .await
    }
}
