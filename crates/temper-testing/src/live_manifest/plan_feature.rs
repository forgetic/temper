use std::process::Command;
use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{ItemNumber, RepositoryId};

#[path = "plan_feature/audit.rs"]
mod audit;
#[path = "plan_feature/fake.rs"]
pub(super) mod fake;
#[path = "plan_feature/verify.rs"]
mod verify;

pub use audit::ValidationAuditEvidence;
use fake::PlanFeatureFake;
use verify::poll_plan_feature;

use super::{
    CiJobEvidence, FinalStateEvidence, IssueEvidence, LiveManifestHarness, PullRequestEvidence,
};

const PLAN_TITLE: &str = "Plan plan-centric dogfood delivery";
const FIRST_CODE_TITLE: &str = "Implement plan foundation slice";
const SECOND_CODE_TITLE: &str = "Implement validation and landing slice";
const SCENARIO_TITLE: &str = "Author the plan-centric feature scenario";
const FOLLOWUP_CODE_TITLE: &str = "Implement validation follow-up regression";
const LANDING_TITLE: &str = "Land plan-centric dogfood feature branch";
const FOLLOWUP_VALIDATION_SUMMARY: &str =
    "Requested one implementation follow-up before aggregate landing.";
const VALIDATION_SUMMARY: &str =
    "Validated all feature-branch implementations and aggregate landing readiness.";
const ASSERT_POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LivePlanFeatureEvidence {
    pub feature_branch: String,
    pub feature_issue: IssueState,
    pub plan_issue: IssueState,
    pub first_code_issue: IssueState,
    pub second_code_issue: IssueState,
    pub scenario_issue: IssueState,
    pub followup_code_issue: IssueState,
    pub first_pr: PullRequestStateEvidence,
    pub second_pr: PullRequestStateEvidence,
    pub scenario_pr: PullRequestStateEvidence,
    pub followup_pr: PullRequestStateEvidence,
    pub landing_pr: PullRequestStateEvidence,
    pub ci_jobs: Vec<PullRequestCiJobEvidence>,
    pub validation_audits: Vec<ValidationAuditEvidence>,
    pub prompt_guidance: Vec<RolePromptEvidence>,
    pub initial_main_sha: String,
    pub main_sha_before_landing: String,
    pub final_main_sha: String,
    pub observed_second_blocked: bool,
    pub observed_second_unblocked: bool,
    pub observed_scenario_blocked: bool,
    pub observed_scenario_unblocked: bool,
    pub observed_landing_open_with_parents_open: bool,
    pub validation_waited_for_implementations: bool,
    pub ci_green_before_merge: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueState {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub target_branch: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestStateEvidence {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub source_branch: String,
    pub target_branch: String,
    pub head_sha: Option<String>,
    pub base_sha: Option<String>,
    pub merged_sha: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestCiJobEvidence {
    pub pull_request_number: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolePromptEvidence {
    pub role: String,
    pub request_count: usize,
    pub role_guidance_excerpt: String,
    pub prompt_guidance_excerpt: String,
    pub tool_guidance_excerpt: String,
    pub constraint_excerpts: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn converge(
    harness: &LiveManifestHarness,
    forge: &ForgejoForge,
    repository: &RepositoryId,
    feature_issue: ItemNumber,
    standalone: &mut super::process::ChildGuard,
    timeout: Duration,
    initial_main_sha: &str,
    forge_url: &str,
    admin_token: &str,
    fake: &PlanFeatureFake,
) -> Result<(FinalStateEvidence, LivePlanFeatureEvidence), String> {
    let deadline = Instant::now() + timeout;
    let mut plan_feature = poll_plan_feature(
        deadline,
        standalone,
        forge,
        repository,
        feature_issue,
        &harness.scenario.repo.default_branch,
        initial_main_sha,
        forge_url,
        admin_token,
        &harness.scenario.repo.owner,
        &harness.scenario.repo.name,
    )?;
    for (role, actual, minimum) in [
        ("architect", fake.architect_requests(), 4),
        ("engineer", fake.engineer_requests(), 9),
        ("scenario_author", fake.scenario_author_requests(), 3),
        ("tester", fake.tester_requests(), 4),
    ] {
        if actual < minimum {
            return Err(format!(
                "fake LLM served only {actual} {role} requests; expected at least {minimum}\n{}",
                fake.log_tail()
            ));
        }
    }
    plan_feature.prompt_guidance = fake.prompt_guidance_evidence()?;
    plan_feature.final_main_sha = remote_branch_head(
        forge_url,
        admin_token,
        &harness.scenario.repo.owner,
        &harness.scenario.repo.name,
        &harness.scenario.repo.default_branch,
    )?;
    if plan_feature.final_main_sha
        != plan_feature
            .landing_pr
            .merged_sha
            .clone()
            .unwrap_or_default()
    {
        return Err(format!(
            "default branch did not end at the aggregate landing merge: head={} landing={:?}",
            plan_feature.final_main_sha, plan_feature.landing_pr.merged_sha
        ));
    }
    let final_state = FinalStateEvidence {
        issue: IssueEvidence {
            number: plan_feature.feature_issue.number,
            title: plan_feature.feature_issue.title.clone(),
            state: plan_feature.feature_issue.state.clone(),
            labels: plan_feature.feature_issue.labels.clone(),
        },
        pull_request: PullRequestEvidence {
            number: plan_feature.landing_pr.number,
            title: plan_feature.landing_pr.title.clone(),
            state: plan_feature.landing_pr.state.clone(),
            labels: plan_feature.landing_pr.labels.clone(),
            author: super::ENGINEER.to_string(),
            merged_by: None,
            head_branch: plan_feature.landing_pr.source_branch.clone(),
            head_sha: plan_feature.landing_pr.head_sha.clone(),
            merged_sha: plan_feature.landing_pr.merged_sha.clone(),
        },
        ci_jobs: plan_feature
            .ci_jobs
            .iter()
            .map(|job| CiJobEvidence {
                name: job.name.clone(),
                status: job.status.clone(),
                conclusion: job.conclusion.clone(),
                url: job.url.clone(),
            })
            .collect(),
    };
    Ok((final_state, plan_feature))
}

pub(super) fn local_checkout_head(checkout: &std::path::Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|error| format!("read seeded repository head: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "read seeded repository head failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .filter(|sha| !sha.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "seeded repository did not expose a HEAD SHA".to_string())
}

pub(super) fn remote_branch_head(
    base_url: &str,
    token: &str,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Result<String, String> {
    super::process::engine_block_on(async {
        let response = temper_engine_io::http::JsonClient::new()
            .send(
                "GET",
                format!("{base_url}/api/v1/repos/{owner}/{repo}/branches/{branch}"),
                Some(token),
                None,
            )
            .await
            .map_err(|error| format!("query remote branch {branch}: {error}"))?;
        if !(200..300).contains(&response.status) {
            return Err(format!(
                "query remote branch {branch} returned HTTP {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            ));
        }
        let body: serde_json::Value = serde_json::from_slice(&response.body)
            .map_err(|error| format!("parse remote branch {branch}: {error}"))?;
        body.pointer("/commit/id")
            .or_else(|| body.pointer("/commit/sha"))
            .and_then(serde_json::Value::as_str)
            .filter(|sha| !sha.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("remote branch {branch} did not expose a commit id: {body}"))
    })
}
