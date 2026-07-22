use super::*;

#[async_trait]
impl<F: Forge> Forge for CrashForge<F> {
    fn item_number_namespace(&self) -> ItemNumberNamespace {
        self.inner.item_number_namespace()
    }

    async fn current_user(&self) -> ForgeResult<User> {
        self.inner.current_user().await
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        self.inner.get_user(id).await
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        self.inner.list_repositories(query).await
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        self.inner.create_repository(input).await
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        self.inner.get_repository(id).await
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        self.inner.get_repository_by_path(path).await
    }

    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        self.inner.list_labels(repo_id).await
    }

    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label> {
        self.inner.upsert_label(repo_id, input).await
    }

    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        let n = self.tick(ForgeOp::ListIssues);
        if query == IssueQuery::default() {
            self.tick(ForgeOp::ListIssuesDefault);
        }
        self.issue_queries
            .lock()
            .expect("issue queries mutex")
            .push(query.clone());
        self.guard(ForgeOp::ListIssues, n, FaultPoint::Before)?;
        let result = self.inner.list_issues(repo_id, query).await?;
        self.guard(ForgeOp::ListIssues, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn list_issue_candidates(
        &self,
        repo_id: &RepositoryId,
        query: IssueCandidateQuery,
    ) -> ForgeResult<Vec<Issue>> {
        let n = self.tick(ForgeOp::ListIssueCandidates);
        self.issue_candidate_queries
            .lock()
            .expect("issue candidate queries mutex")
            .push(query.clone());
        self.guard(ForgeOp::ListIssueCandidates, n, FaultPoint::Before)?;
        let result = self.inner.list_issue_candidates(repo_id, query).await?;
        self.guard(ForgeOp::ListIssueCandidates, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue> {
        let n = self.tick(ForgeOp::CreateIssue);
        self.guard(ForgeOp::CreateIssue, n, FaultPoint::Before)?;
        let result = self.inner.create_issue(repo_id, input).await?;
        self.guard(ForgeOp::CreateIssue, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        let n = self.tick(ForgeOp::GetIssue);
        self.issue_exact_details
            .lock()
            .expect("issue exact details mutex")
            .push(ItemListDetails::full());
        self.guard(ForgeOp::GetIssue, n, FaultPoint::Before)?;
        let result = self.inner.get_issue(id).await?;
        self.guard(ForgeOp::GetIssue, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_issue_with_details(
        &self,
        id: &IssueId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        let n = self.tick(ForgeOp::GetIssue);
        self.issue_exact_details
            .lock()
            .expect("issue exact details mutex")
            .push(details);
        self.guard(ForgeOp::GetIssue, n, FaultPoint::Before)?;
        let result = self.inner.get_issue_with_details(id, details).await?;
        self.guard(ForgeOp::GetIssue, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        let n = self.tick(ForgeOp::GetIssueByNumber);
        self.issue_exact_details
            .lock()
            .expect("issue exact details mutex")
            .push(ItemListDetails::full());
        self.guard(ForgeOp::GetIssueByNumber, n, FaultPoint::Before)?;
        let result = self.inner.get_issue_by_number(repo_id, number).await?;
        self.guard(ForgeOp::GetIssueByNumber, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_issue_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        let n = self.tick(ForgeOp::GetIssueByNumber);
        self.issue_exact_details
            .lock()
            .expect("issue exact details mutex")
            .push(details);
        self.guard(ForgeOp::GetIssueByNumber, n, FaultPoint::Before)?;
        let result = self
            .inner
            .get_issue_by_number_with_details(repo_id, number, details)
            .await?;
        self.guard(ForgeOp::GetIssueByNumber, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        let n = self.tick(ForgeOp::UpdateIssue);
        self.guard(ForgeOp::UpdateIssue, n, FaultPoint::Before)?;
        self.issue_updates
            .lock()
            .expect("issue updates mutex")
            .push(input.clone());
        let result = self.inner.update_issue(id, input).await?;
        self.guard(ForgeOp::UpdateIssue, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn update_issue_from_snapshot(
        &self,
        current: &Issue,
        input: UpdateIssue,
    ) -> ForgeResult<Issue> {
        let n = self.tick(ForgeOp::UpdateIssue);
        self.guard(ForgeOp::UpdateIssue, n, FaultPoint::Before)?;
        self.issue_updates
            .lock()
            .expect("issue updates mutex")
            .push(input.clone());
        let result = self
            .inner
            .update_issue_from_snapshot(current, input)
            .await?;
        self.guard(ForgeOp::UpdateIssue, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue> {
        self.inner.add_issue_dependency(id, target).await
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        self.inner.remove_issue_dependency(id, target).await
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        let n = self.tick(ForgeOp::ListIssueComments);
        self.guard(ForgeOp::ListIssueComments, n, FaultPoint::Before)?;
        let result = self.inner.list_issue_comments(id).await?;
        self.guard(ForgeOp::ListIssueComments, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        let n = self.tick(ForgeOp::AddIssueComment);
        self.guard(ForgeOp::AddIssueComment, n, FaultPoint::Before)?;
        let result = self.inner.add_issue_comment(id, input).await?;
        self.guard(ForgeOp::AddIssueComment, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let n = self.tick(ForgeOp::ListPullRequests);
        if query == PullRequestQuery::default() {
            self.tick(ForgeOp::ListPullRequestsDefault);
        }
        self.pull_request_queries
            .lock()
            .expect("pull request queries mutex")
            .push(query.clone());
        self.guard(ForgeOp::ListPullRequests, n, FaultPoint::Before)?;
        let result = self.inner.list_pull_requests(repo_id, query).await?;
        self.guard(ForgeOp::ListPullRequests, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn list_pull_request_candidates(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestCandidateQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let n = self.tick(ForgeOp::ListPullRequestCandidates);
        self.pull_request_candidate_queries
            .lock()
            .expect("pull request candidate queries mutex")
            .push(query.clone());
        self.guard(ForgeOp::ListPullRequestCandidates, n, FaultPoint::Before)?;
        let result = self
            .inner
            .list_pull_request_candidates(repo_id, query)
            .await?;
        self.guard(ForgeOp::ListPullRequestCandidates, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let n = self.tick(ForgeOp::CreatePullRequest);
        self.guard(ForgeOp::CreatePullRequest, n, FaultPoint::Before)?;
        let result = self.inner.create_pull_request(repo_id, input).await?;
        self.guard(ForgeOp::CreatePullRequest, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        let n = self.tick(ForgeOp::GetPullRequest);
        self.pull_request_exact_details
            .lock()
            .expect("pull request exact details mutex")
            .push(ItemListDetails::full());
        self.guard(ForgeOp::GetPullRequest, n, FaultPoint::Before)?;
        let result = self.inner.get_pull_request(id).await?;
        self.guard(ForgeOp::GetPullRequest, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_pull_request_with_details(
        &self,
        id: &PullRequestId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        let n = self.tick(ForgeOp::GetPullRequest);
        self.pull_request_exact_details
            .lock()
            .expect("pull request exact details mutex")
            .push(details);
        self.guard(ForgeOp::GetPullRequest, n, FaultPoint::Before)?;
        let result = self
            .inner
            .get_pull_request_with_details(id, details)
            .await?;
        self.guard(ForgeOp::GetPullRequest, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        let n = self.tick(ForgeOp::GetPullRequestByNumber);
        self.pull_request_exact_details
            .lock()
            .expect("pull request exact details mutex")
            .push(ItemListDetails::full());
        self.guard(ForgeOp::GetPullRequestByNumber, n, FaultPoint::Before)?;
        let result = self
            .inner
            .get_pull_request_by_number(repo_id, number)
            .await?;
        self.guard(ForgeOp::GetPullRequestByNumber, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_pull_request_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        let n = self.tick(ForgeOp::GetPullRequestByNumber);
        self.pull_request_exact_details
            .lock()
            .expect("pull request exact details mutex")
            .push(details);
        self.guard(ForgeOp::GetPullRequestByNumber, n, FaultPoint::Before)?;
        let result = self
            .inner
            .get_pull_request_by_number_with_details(repo_id, number, details)
            .await?;
        self.guard(ForgeOp::GetPullRequestByNumber, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let n = self.tick(ForgeOp::UpdatePullRequest);
        self.guard(ForgeOp::UpdatePullRequest, n, FaultPoint::Before)?;
        let result = self.inner.update_pull_request(id, input).await?;
        self.guard(ForgeOp::UpdatePullRequest, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.inner.add_pull_request_dependency(id, target).await
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        self.inner.remove_pull_request_dependency(id, target).await
    }

    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        self.inner.request_pull_request_reviewers(id, input).await
    }

    async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        let n = self.tick(ForgeOp::ListPullRequestReviews);
        self.guard(ForgeOp::ListPullRequestReviews, n, FaultPoint::Before)?;
        let result = self.inner.list_pull_request_reviews(id).await?;
        self.guard(ForgeOp::ListPullRequestReviews, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        self.inner.submit_pull_request_review(id, input).await
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        let n = self.tick(ForgeOp::ListPullRequestComments);
        self.guard(ForgeOp::ListPullRequestComments, n, FaultPoint::Before)?;
        let result = self.inner.list_pull_request_comments(id).await?;
        self.guard(ForgeOp::ListPullRequestComments, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        let n = self.tick(ForgeOp::AddPullRequestComment);
        self.guard(ForgeOp::AddPullRequestComment, n, FaultPoint::Before)?;
        let result = self.inner.add_pull_request_comment(id, input).await?;
        self.guard(ForgeOp::AddPullRequestComment, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        let n = self.tick(ForgeOp::MergePullRequest);
        self.guard(ForgeOp::MergePullRequest, n, FaultPoint::Before)?;
        self.merge_inputs
            .lock()
            .expect("merge inputs mutex")
            .push(input.clone());
        let result = self.inner.merge_pull_request(id, input).await?;
        self.guard(ForgeOp::MergePullRequest, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        let n = self.tick(ForgeOp::ListCiJobs);
        self.guard(ForgeOp::ListCiJobs, n, FaultPoint::Before)?;
        let result = self.inner.list_ci_jobs(repo_id, query).await?;
        self.guard(ForgeOp::ListCiJobs, n, FaultPoint::After)?;
        Ok(result)
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        self.inner.get_ci_job(id).await
    }
}
