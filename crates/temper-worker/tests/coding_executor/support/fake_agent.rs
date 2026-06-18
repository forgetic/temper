use std::sync::{Arc, Mutex};

use super::*;

/// What the fake agent does for one turn. Each variant mirrors a behavior the
/// old shell-script fakes produced, but expressed as in-process effects:
/// capture the context, optionally write a product diff into the checkout, and
/// return a [`WorkspaceResult`] or an [`AgentRunError`].
#[derive(Clone, Copy)]
pub enum AgentBehavior {
    /// Engineer head path: write a product diff, return a summary-only result.
    Success,
    /// Engineer head path: write a product diff and return a structured plan.
    SuccessWithPlan,
    /// A transient provider error (the in-process analog of the old non-zero
    /// subprocess exit).
    TransientError,
    /// Return a summary-only result and write no diff (engineer => "no diff").
    NoDiff,
    /// Engineer that checkpoint-commits and pushes its whole product itself
    /// (phase 6b), leaving a clean working tree for the executor.
    CheckpointCommits,
    /// Engineer only creates the plan-publication empty commit; no product tree
    /// diff exists, so the final head path must be rejected.
    PlanOnlyEmptyCommit,
    /// Return a writable verdict the executor does not route (permanent).
    Verdict,
    /// Architect read-only `ready_code` verdict with a rewritten body.
    ReadOnlyVerdict,
    /// Architect `needs_breakdown` verdict with children.
    ReadOnlyBreakdownVerdict,
    /// Read-only verdict that also (wrongly) writes a diff; the executor must
    /// discard it and still route the verdict without pushing.
    ReadOnlyVerdictWithDiff,
    /// Read-only verdict outside the declared vocabulary (permanent).
    UndeclaredVerdict,
    /// Writable escalation verdict that also writes a diff to discard.
    WritableVerdict,
    /// Reviewer `approve` verdict (records the checked-out HEAD sha).
    ReviewApprove,
    /// Reviewer `changes` verdict with a review body; writes a diff to discard.
    ReviewChanges,
    /// Reviewer result with no verdict (permanent).
    ReviewMissingVerdict,
    /// Reviewer verdict outside the declared vocabulary (permanent).
    ReviewUndeclaredVerdict,
}

impl AgentBehavior {
    pub fn runner(self) -> FakeAgentRunner {
        FakeAgentRunner {
            behavior: self,
            captured: Arc::new(Mutex::new(None)),
            observed_head_sha: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Clone)]
pub struct FakeAgentRunner {
    behavior: AgentBehavior,
    captured: Arc<Mutex<Option<WorkspaceContext>>>,
    observed_head_sha: Arc<Mutex<Option<String>>>,
}

impl FakeAgentRunner {
    /// The context the executor handed the runner (panics if the runner never
    /// ran).
    pub fn captured_context(&self) -> WorkspaceContext {
        self.captured
            .lock()
            .expect("capture lock")
            .clone()
            .expect("fake agent runner captured a context")
    }

    /// The `git HEAD` sha the runner saw in the prepared checkout.
    pub fn observed_head_sha(&self) -> String {
        self.observed_head_sha
            .lock()
            .expect("head lock")
            .clone()
            .expect("fake agent runner observed HEAD")
    }

    fn write_diff(cwd: &Path) {
        fs::write(cwd.join("agent-output.txt"), "agent diff\n").expect("write fake agent diff");
    }
}

impl AgentRunner for FakeAgentRunner {
    async fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        _progress: Arc<dyn ProgressSink>,
    ) -> Result<WorkspaceResult, AgentRunError> {
        *self.captured.lock().expect("capture lock") = Some(context.clone());
        // The agent's cwd is the workspace root; it edits inside each repo's
        // sibling dir. This fake operates on the primary repo (ADR 0023).
        let primary_dir = context
            .primary()
            .map(|repo| repo.dir.clone())
            .unwrap_or_default();
        let repo_cwd = cwd.join(&primary_dir);
        *self.observed_head_sha.lock().expect("head lock") =
            Some(git_output(["-C", path_str(&repo_cwd), "rev-parse", "HEAD"]));

        match self.behavior {
            AgentBehavior::Success => {
                Self::write_diff(&repo_cwd);
                Ok(WorkspaceResult {
                    summary: Some("did the work".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::SuccessWithPlan => {
                Self::write_diff(&repo_cwd);
                Ok(WorkspaceResult {
                    summary: Some("did the planned work".to_string()),
                    plan: Some(ImplementationPlan {
                        phases: vec!["Write test".to_string(), "Implement fix".to_string()],
                    }),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::TransientError => Err(AgentRunError::transient(
                "LLM run failed: provider transport reset",
            )),
            AgentBehavior::NoDiff => Ok(WorkspaceResult {
                summary: Some("nothing changed".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::CheckpointCommits => {
                Self::write_diff(&repo_cwd);
                let work_branch = context
                    .primary()
                    .and_then(|repo| repo.branch_hint.clone())
                    .expect("primary repo carries a branch hint");
                git_output(["-C", path_str(&repo_cwd), "add", "-A"]);
                git_output([
                    "-C",
                    path_str(&repo_cwd),
                    "-c",
                    "user.name=Agent Checkpoint",
                    "-c",
                    "user.email=agent@example.test",
                    "commit",
                    "-m",
                    "checkpoint(step 2): push checkpoint",
                ]);
                git_output([
                    "-C",
                    path_str(&repo_cwd),
                    "push",
                    "origin",
                    &format!("HEAD:refs/heads/{work_branch}"),
                ]);
                Ok(WorkspaceResult {
                    summary: Some("checkpointed the work".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::PlanOnlyEmptyCommit => {
                git_output([
                    "-C",
                    path_str(&repo_cwd),
                    "-c",
                    "user.name=Agent Plan",
                    "-c",
                    "user.email=agent@example.test",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "Publish implementation plan for pr-for-code-7",
                ]);
                Ok(WorkspaceResult {
                    summary: Some("published a plan only".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::Verdict => Ok(WorkspaceResult {
                verdict: Some("needs_design".to_string()),
                summary: Some("cannot proceed".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::ReadOnlyVerdict => Ok(WorkspaceResult {
                verdict: Some("ready_code".to_string()),
                body: Some("rewritten".to_string()),
                summary: Some("did triage".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::ReadOnlyVerdictWithDiff => {
                Self::write_diff(&repo_cwd);
                Ok(WorkspaceResult {
                    verdict: Some("ready_code".to_string()),
                    body: Some("rewritten".to_string()),
                    summary: Some("did triage".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::ReadOnlyBreakdownVerdict => Ok(WorkspaceResult {
                verdict: Some("needs_breakdown".to_string()),
                summary: Some("planned breakdown".to_string()),
                children: vec![
                    WorkspaceResultChild {
                        slug: "api-schema".to_string(),
                        title: "Define the API schema".to_string(),
                        body: "Write the shared API schema.".to_string(),
                        labels: vec!["code".to_string(), "ready".to_string()],
                        depends_on: Vec::new(),
                        target_repo: None,
                    },
                    WorkspaceResultChild {
                        slug: "web-client".to_string(),
                        title: "Implement the web client".to_string(),
                        body: "Build the web client against the API schema.".to_string(),
                        labels: Vec::new(),
                        depends_on: vec!["api-schema".to_string()],
                        target_repo: Some("acme/other".to_string()),
                    },
                ],
                ..WorkspaceResult::default()
            }),
            AgentBehavior::UndeclaredVerdict => Ok(WorkspaceResult {
                verdict: Some("needs_breakdown".to_string()),
                summary: Some("needs splitting".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::WritableVerdict => {
                Self::write_diff(cwd);
                Ok(WorkspaceResult {
                    verdict: Some("needs_architect".to_string()),
                    body: Some("blocked".to_string()),
                    summary: Some("cannot proceed".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::ReviewApprove => Ok(WorkspaceResult {
                verdict: Some("approve".to_string()),
                summary: Some("looks good".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::ReviewChanges => {
                Self::write_diff(cwd);
                Ok(WorkspaceResult {
                    verdict: Some("changes".to_string()),
                    review_body: Some("please add error handling".to_string()),
                    summary: Some("needs error handling".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::ReviewMissingVerdict => Ok(WorkspaceResult {
                summary: Some("no opinion".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::ReviewUndeclaredVerdict => Ok(WorkspaceResult {
                verdict: Some("merge_now".to_string()),
                ..WorkspaceResult::default()
            }),
        }
    }
}
