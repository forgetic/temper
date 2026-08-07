// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use temper_testing::live_manifest::{
    LiveManifestEvidence, ScenarioBundle, TemperCommand, run_live_manifest,
};

use crate::run_context::ScenarioRunFacts;
use crate::run_evidence;

#[path = "live/artifacts.rs"]
mod artifacts;
#[path = "live/ci.rs"]
mod ci;

use super::observability::capture_observability;
use artifacts::{RetainedLogPaths, copy_report_artifacts, stimulus_log_paths};
use ci::{ci_job, ci_observation};
pub(super) use ci::{ci_requests, verified_failure_proof};

const PRIMARY_TEMPER_BIN_ENV: &str = "TEMPER_SCENARIO_TEMPER_BIN";
const COMPAT_TEMPER_BIN_ENV: &str = "TEMPER_BIN";

pub(super) fn run_and_print(
    scenario: &ScenarioBundle,
    facts: &ScenarioRunFacts,
    temper_bin: Option<&Path>,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    let evidence = run_live(scenario, temper_bin)?;
    let lines = live_evidence_lines(&evidence, None);
    let artifact = live_artifact(scenario, &evidence, context, &lines, None);
    print_outcome(&lines, facts, context);
    retain_artifact_workspace(evidence);
    Ok(artifact)
}

pub(super) fn evidence_lines(
    scenario: &ScenarioBundle,
    temper_bin: Option<&Path>,
    artifact_dir: Option<&Path>,
) -> Result<Vec<String>, String> {
    let evidence = run_live(scenario, temper_bin)?;
    if let Some(artifact_dir) = artifact_dir {
        let retained_logs = copy_report_artifacts(&evidence, artifact_dir)?;
        Ok(live_evidence_lines(&evidence, Some(&retained_logs)))
    } else {
        let lines = live_evidence_lines(&evidence, None);
        retain_artifact_workspace(evidence);
        Ok(lines)
    }
}

fn run_live(
    scenario: &ScenarioBundle,
    temper_bin: Option<&Path>,
) -> Result<LiveManifestEvidence, String> {
    let temper = resolve_temper_command(temper_bin)?;
    run_live_manifest(scenario.clone(), temper)
}

fn print_outcome(
    lines: &[String],
    facts: &ScenarioRunFacts,
    context: &run_evidence::RunEvidenceContext,
) {
    println!("scenario: {}", context.scenario.name);
    facts.print_stdout();
    println!("{}", context.scenario.runner_selection);
    println!("verdict: passed");
    println!("evidence:");
    for line in lines {
        println!("  {line}");
    }
}

fn retain_artifact_workspace(evidence: LiveManifestEvidence) {
    // Live CLI output cites files under the harness workspace. The harness
    // normally deletes that temp tree when evidence is dropped (which is ideal
    // for ignored tests), but direct operator runs need the printed log/artifact
    // paths to remain readable after the CLI exits. Validation reports use
    // copy_report_artifacts instead, so their uploaded artifact directory owns
    // the cited logs and can let the temp workspace drop normally.
    std::mem::forget(evidence);
}

fn live_artifact(
    scenario: &ScenarioBundle,
    evidence: &LiveManifestEvidence,
    context: &run_evidence::RunEvidenceContext,
    lines: &[String],
    retained_logs: Option<&RetainedLogPaths>,
) -> run_evidence::RunEvidenceArtifact {
    let workspace_root = retained_logs
        .map(|logs| &logs.workspace_root)
        .unwrap_or(&evidence.logs.workspace_root);
    let init_log = retained_logs
        .map(|logs| &logs.init_log)
        .unwrap_or(&evidence.logs.init_log);
    let repo_populate_log = retained_logs
        .map(|logs| &logs.repo_populate_log)
        .unwrap_or(&evidence.logs.repo_populate_log);
    let standalone_log = retained_logs
        .map(|logs| &logs.standalone_log)
        .unwrap_or(&evidence.logs.standalone_log);
    let fake_llm_log = retained_logs
        .map(|logs| &logs.fake_llm_log)
        .unwrap_or(&evidence.logs.fake_llm_log);
    let ci_diagnostics_log = retained_logs
        .map(|logs| &logs.ci_diagnostics_log)
        .unwrap_or(&evidence.logs.ci_diagnostics_log);

    let codebase_mcp_log = retained_logs
        .and_then(|logs| logs.codebase_mcp_log.as_ref())
        .or_else(|| {
            evidence
                .codebase_memory
                .as_ref()
                .map(|codebase_memory| &codebase_memory.fake_mcp_log)
        });

    let final_state = if let Some(plan) = evidence.plan_feature.as_ref() {
        super::plan_artifact::final_state(evidence, plan)
    } else if let Some(handoff) = evidence.handoff.as_ref() {
        run_evidence::FinalStateEvidence {
            issues: vec![
                run_evidence::IssueStateEvidence {
                    number: handoff.create.issue_number,
                    id: Some("create".to_string()),
                    title: None,
                    state: Some("open".to_string()),
                    labels: vec!["code".to_string(), "in-progress".to_string()],
                },
                run_evidence::IssueStateEvidence {
                    number: handoff.refresh.issue_number,
                    id: Some("refresh".to_string()),
                    title: None,
                    state: Some("open".to_string()),
                    labels: vec!["code".to_string(), "in-progress".to_string()],
                },
            ],
            pull_requests: vec![
                run_evidence::PullRequestStateEvidence {
                    number: handoff.create.pr_number,
                    id: Some("create".to_string()),
                    title: Some(handoff.create.title.clone()),
                    body: Some(handoff.create.body.clone()),
                    state: Some(handoff.create.pr_state.clone()),
                    labels: handoff.create.labels.clone(),
                    head_branch: Some(handoff.create.head_branch.clone()),
                    head_sha: handoff.create.head_sha.clone(),
                    merged_sha: None,
                },
                run_evidence::PullRequestStateEvidence {
                    number: handoff.refresh.pr_number,
                    id: Some("refresh".to_string()),
                    title: Some(handoff.refresh.title.clone()),
                    body: Some(handoff.refresh.body.clone()),
                    state: Some(handoff.refresh.pr_state.clone()),
                    labels: handoff.refresh.labels.clone(),
                    head_branch: Some(handoff.refresh.head_branch.clone()),
                    head_sha: handoff.refresh.head_sha.clone(),
                    merged_sha: None,
                },
            ],
            repositories: vec![run_evidence::RepositoryStateEvidence {
                id: Some(evidence.repo_id.clone()),
                slug: Some(evidence.repo_slug.clone()),
                branches: vec![run_evidence::RepositoryBranchStateEvidence {
                    name: evidence.repo_default_branch.clone(),
                    head_sha: None,
                    contains_engineer_diff: Some(false),
                }],
            }],
            ci: run_evidence::CiStateEvidence {
                completed_jobs: Some(0),
                jobs: Vec::new(),
                observations: Vec::new(),
                heads: Vec::new(),
                failure_evidence: None,
                requests: ci_requests(evidence),
                request_capture_dropped: Some(evidence.ci_request_capture_dropped),
            },
        }
    } else {
        let implementation_number = evidence.final_state.pull_request.number;
        let pull_requests = evidence
            .forge_pull_requests
            .iter()
            .map(|pull| run_evidence::PullRequestStateEvidence {
                number: pull.number,
                id: Some(if pull.number == implementation_number {
                    "implementation".to_string()
                } else {
                    format!("forge-pr-{}", pull.number)
                }),
                title: Some(pull.title.clone()),
                body: None,
                state: Some(pull.state.clone()),
                labels: pull.labels.clone(),
                head_branch: Some(pull.head_branch.clone()),
                head_sha: pull.head_sha.clone(),
                merged_sha: pull.merged_sha.clone(),
            })
            .collect::<Vec<_>>();
        // Forge's portable read surface has no list-branches operation. Every
        // published PR does carry its source branch, so retain the default
        // branch plus the complete terminal PR-source inventory. This is the
        // publication surface the recovery scenario must bound.
        let mut branches = BTreeMap::from([(
            evidence.repo_default_branch.clone(),
            run_evidence::RepositoryBranchStateEvidence {
                name: evidence.repo_default_branch.clone(),
                head_sha: evidence
                    .final_state
                    .pull_request
                    .merged_sha
                    .clone()
                    .or_else(|| evidence.final_state.pull_request.head_sha.clone()),
                contains_engineer_diff: Some(evidence.final_state.pull_request.state == "merged"),
            },
        )]);
        for pull in &evidence.forge_pull_requests {
            branches.entry(pull.head_branch.clone()).or_insert_with(|| {
                run_evidence::RepositoryBranchStateEvidence {
                    name: pull.head_branch.clone(),
                    head_sha: pull.head_sha.clone(),
                    contains_engineer_diff: Some(true),
                }
            });
        }
        run_evidence::FinalStateEvidence {
            issues: vec![run_evidence::IssueStateEvidence {
                number: evidence.final_state.issue.number,
                id: Some(
                    if evidence.codebase_memory.is_some() {
                        "source"
                    } else {
                        "intake"
                    }
                    .to_string(),
                ),
                title: Some(evidence.final_state.issue.title.clone()),
                state: Some(evidence.final_state.issue.state.clone()),
                labels: evidence.final_state.issue.labels.clone(),
            }],
            pull_requests,
            repositories: vec![run_evidence::RepositoryStateEvidence {
                id: Some(evidence.repo_id.clone()),
                slug: Some(evidence.repo_slug.clone()),
                branches: branches.into_values().collect(),
            }],
            ci: run_evidence::CiStateEvidence {
                completed_jobs: Some(evidence.final_state.ci_jobs.len()),
                jobs: evidence
                    .final_state
                    .ci_jobs
                    .iter()
                    .map(|job| ci_job(job, evidence.final_state.pull_request.number))
                    .collect(),
                observations: evidence
                    .final_state
                    .ci_observations
                    .iter()
                    .map(|observation| {
                        ci_observation(observation, evidence.final_state.pull_request.number)
                    })
                    .collect(),
                heads: evidence
                    .final_state
                    .ci_heads
                    .iter()
                    .map(|head| run_evidence::CiHeadEvidence {
                        phase: head.phase.clone(),
                        head_sha: head.head_sha.clone(),
                        observed_after_ms: head.observed_after_ms,
                        jobs: head
                            .jobs
                            .iter()
                            .map(|job| ci_job(job, evidence.final_state.pull_request.number))
                            .collect(),
                        observations: head
                            .observations
                            .iter()
                            .map(|observation| {
                                ci_observation(
                                    observation,
                                    evidence.final_state.pull_request.number,
                                )
                            })
                            .collect(),
                    })
                    .collect(),
                failure_evidence: evidence.ci_failure_evidence.as_ref().map(|service| {
                    run_evidence::CiFailureEvidenceServiceEvidence {
                        endpoint_path: service.endpoint_path.clone(),
                        issuer: service.issuer.clone(),
                        protected_producers: service.protected_producers.clone(),
                        published_proofs: service.published_proofs,
                    }
                }),
                requests: ci_requests(evidence),
                request_capture_dropped: Some(evidence.ci_request_capture_dropped),
            },
        }
    };
    let mut artifact = context.artifact(final_state);
    artifact.effective_configuration = Some(run_evidence::EffectiveConfigurationEvidence {
        ci_poll_cadence_secs: evidence.effective_configuration.ci_poll_cadence_secs,
        poll_cadence_secs: evidence.effective_configuration.poll_cadence_secs,
        mechanical_cadence_secs: evidence.effective_configuration.mechanical_cadence_secs,
    });
    artifact.convergence = Some(run_evidence::ConvergenceEvidence {
        startup_ms: Some(duration_ms(evidence.startup)),
        convergence_ms: Some(duration_ms(evidence.convergence)),
        total_elapsed_ms: Some(duration_ms(evidence.total_elapsed)),
        poll_backstop_ms: Some(duration_ms(evidence.poll_backstop)),
        ..run_evidence::ConvergenceEvidence::default()
    });
    artifact.binary = binary_identity(&evidence.temper_binary);
    artifact.execution = Some(run_evidence::ExecutionEvidence {
        status: "completed".to_string(),
        total_duration_ms: duration_ms(evidence.total_elapsed),
        failure: None,
    });
    let provider_request_ids = [
        ("architect", evidence.fake_llm.architect_requests),
        ("engineer", evidence.fake_llm.engineer_requests),
        ("tester", evidence.fake_llm.tester_requests),
    ]
    .into_iter()
    .flat_map(|(role, count)| (1..=count).map(move |index| format!("{role}:{index}")))
    .collect::<Vec<_>>();
    artifact.provider = Some(run_evidence::ProviderEvidence {
        forgejo_url: Some(evidence.forge_url.clone()),
        repo_slug: Some(evidence.repo_slug.clone()),
        issue_number: Some(evidence.final_state.issue.number),
        pr_number: Some(evidence.final_state.pull_request.number),
        head_branch: Some(evidence.final_state.pull_request.head_branch.clone()),
        merged_sha: evidence.final_state.pull_request.merged_sha.clone(),
        temper_binary: Some(evidence.temper_binary.display().to_string()),
        fake_llm_url: Some(evidence.fake_llm.base_url.clone()),
        jig_script_paths: vec![scenario.jig_script_path().display().to_string()],
        request_log_path: Some(fake_llm_log.display().to_string()),
        request_count: Some(provider_request_ids.len()),
        request_ids: provider_request_ids,
        request_counts_by_role: [
            (
                "architect".to_string(),
                evidence.fake_llm.architect_requests,
            ),
            ("engineer".to_string(), evidence.fake_llm.engineer_requests),
            ("tester".to_string(), evidence.fake_llm.tester_requests),
        ]
        .into_iter()
        .collect(),
    });
    let source_stimulus_logs;
    let stimulus_logs = if let Some(retained_logs) = retained_logs {
        &retained_logs.stimulus_logs
    } else {
        source_stimulus_logs =
            stimulus_log_paths(&evidence.logs.standalone_log).unwrap_or_default();
        &source_stimulus_logs
    };
    let mut observability_logs = stimulus_logs
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    observability_logs.push(standalone_log);
    artifact.observability = Some(capture_observability(evidence, &observability_logs));
    let mut log_paths = vec![
        init_log.display().to_string(),
        repo_populate_log.display().to_string(),
        standalone_log.display().to_string(),
        fake_llm_log.display().to_string(),
    ];
    if let Some(codebase_mcp_log) = codebase_mcp_log {
        log_paths.push(codebase_mcp_log.display().to_string());
    }
    log_paths.extend(stimulus_logs.iter().map(|path| path.display().to_string()));
    artifact.artifacts = run_evidence::ArtifactCollections {
        log_paths,
        artifact_paths: vec![
            workspace_root.display().to_string(),
            ci_diagnostics_log.display().to_string(),
        ],
    };
    artifact.evidence_lines = lines.to_vec();
    artifact.stimuli = evidence
        .stimuli
        .iter()
        .map(|outcome| run_evidence::StimulusEvidence {
            id: outcome.id.clone(),
            action: outcome.action.clone(),
            status: outcome.status.as_str().to_string(),
            attempts: outcome.attempts,
            timeout_ms: duration_ms(outcome.timeout),
            duration_ms: duration_ms(outcome.duration),
            details: outcome.details.clone(),
        })
        .collect();
    artifact
}

fn binary_identity(path: &Path) -> Option<run_evidence::BinaryIdentityEvidence> {
    let bytes = fs::read(path).ok()?;
    let digest = Sha256::digest(&bytes);
    Some(run_evidence::BinaryIdentityEvidence {
        path: path.display().to_string(),
        sha256: format!("{digest:x}"),
        size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
    })
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn live_evidence_lines(
    evidence: &LiveManifestEvidence,
    retained_logs: Option<&RetainedLogPaths>,
) -> Vec<String> {
    let workspace_root = retained_logs
        .map(|logs| &logs.workspace_root)
        .unwrap_or(&evidence.logs.workspace_root);
    let init_log = retained_logs
        .map(|logs| &logs.init_log)
        .unwrap_or(&evidence.logs.init_log);
    let repo_populate_log = retained_logs
        .map(|logs| &logs.repo_populate_log)
        .unwrap_or(&evidence.logs.repo_populate_log);
    let standalone_log = retained_logs
        .map(|logs| &logs.standalone_log)
        .unwrap_or(&evidence.logs.standalone_log);
    let fake_llm_log = retained_logs
        .map(|logs| &logs.fake_llm_log)
        .unwrap_or(&evidence.logs.fake_llm_log);
    let ci_diagnostics_log = retained_logs
        .map(|logs| &logs.ci_diagnostics_log)
        .unwrap_or(&evidence.logs.ci_diagnostics_log);
    let codebase_mcp_log = retained_logs
        .and_then(|logs| logs.codebase_mcp_log.as_ref())
        .or_else(|| {
            evidence
                .codebase_memory
                .as_ref()
                .map(|codebase_memory| &codebase_memory.fake_mcp_log)
        });
    let mut lines = vec![
        format!("Forgejo URL: {}", evidence.forge_url),
        format!(
            "live topology: repo={} forge_cache_hit={} runner_running={}",
            evidence.repo_slug, evidence.forge_cache_hit, evidence.runner_running
        ),
        format!("temper binary: {}", evidence.temper_binary.display()),
        format!(
            "source issue: #{} \"{}\" state={} labels={:?}",
            evidence.final_state.issue.number,
            evidence.final_state.issue.title,
            evidence.final_state.issue.state,
            evidence.final_state.issue.labels
        ),
        format!(
            "implementation PR: #{} \"{}\" state={} author={} merged_by={:?} head={} sha={:?} merged_sha={:?} labels={:?}",
            evidence.final_state.pull_request.number,
            evidence.final_state.pull_request.title,
            evidence.final_state.pull_request.state,
            evidence.final_state.pull_request.author,
            evidence.final_state.pull_request.merged_by,
            evidence.final_state.pull_request.head_branch,
            evidence.final_state.pull_request.head_sha,
            evidence.final_state.pull_request.merged_sha,
            evidence.final_state.pull_request.labels
        ),
        format!(
            "convergence: {:?} before poll_backstop {:?} (startup {:?}, total {:?})",
            evidence.convergence, evidence.poll_backstop, evidence.startup, evidence.total_elapsed
        ),
        format!(
            "effective standalone configuration: ci_poll_cadence_secs={} poll_cadence_secs={} mechanical_cadence_secs={}",
            evidence.effective_configuration.ci_poll_cadence_secs,
            evidence.effective_configuration.poll_cadence_secs,
            evidence.effective_configuration.mechanical_cadence_secs
        ),
        format!(
            "fake LLM: url={} architect_requests={} engineer_requests={} tester_requests={} log={}",
            evidence.fake_llm.base_url,
            evidence.fake_llm.architect_requests,
            evidence.fake_llm.engineer_requests,
            evidence.fake_llm.tester_requests,
            fake_llm_log.display()
        ),
    ];
    if let Some(codebase_memory) = evidence.codebase_memory.as_ref() {
        lines.extend([
            format!(
                "codebase-memory tools: exposed {:?}; hidden {:?}",
                codebase_memory.safe_tools, codebase_memory.hidden_tools
            ),
            format!(
                "codebase-memory MCP: search_graph calls={} inventory={:?} readiness_delay_ms={} forced_failure_tool={:?} log={}",
                codebase_memory.mcp_search_calls,
                codebase_memory.mcp_call_counts,
                codebase_memory.readiness_delay_ms,
                codebase_memory.forced_failure_tool,
                codebase_mcp_log
                    .unwrap_or(&codebase_memory.fake_mcp_log)
                    .display()
            ),
            format!(
                "codebase-memory diff: engineer produced {} after bounded graph evidence containing {}",
                codebase_memory.produced_file, codebase_memory.expected_result
            ),
        ]);
        if let Some(rebind) = &codebase_memory.stable_rebind {
            lines.push(format!(
                "codebase-memory stable rebind: requested_project={} confirmed_project={} confirmation_calls={} targeted_discovery={} normalized_identity={} targeted_ready_confirmation={} retained_projects={} fresh_prior_binding={} current_root_rebound={} graph_reads_use_confirmed_project={} source_reads_use_confirmed_project={} source_served_from_current_root={} global_inventory_avoided={}",
                rebind.requested_stable_project,
                rebind.confirmed_provider_project,
                rebind.confirmation_call_count,
                rebind.initial_discovery_targeted,
                rebind.normalized_provider_identity,
                rebind.targeted_ready_confirmation,
                rebind.retained_project_count,
                rebind.fresh_prior_binding,
                rebind.current_root_rebound,
                rebind.graph_reads_use_confirmed_project,
                rebind.source_reads_use_confirmed_project,
                rebind.source_served_from_current_root,
                rebind.global_inventory_avoided,
            ));
        }
    }
    if let Some(plan) = evidence.plan_feature.as_ref() {
        lines.extend(super::plan_artifact::evidence_lines(plan));
    }
    if let Some(handoff) = evidence.handoff.as_ref() {
        lines.extend([
            format!(
                "create authored PR title/body: PR #{} for issue #{} has title \"{}\" and body prefix \"{}\"",
                handoff.create.pr_number,
                handoff.create.issue_number,
                handoff.create.title,
                handoff.create.body_prefix
            ),
            format!(
                "refresh authored PR title/body: existing PR #{} for issue #{} has title \"{}\" and stale body text was cleared",
                handoff.refresh.pr_number,
                handoff.refresh.issue_number,
                handoff.refresh.title
            ),
            format!(
                "workflow metadata/source relation: create parent {} correlation {}; refresh parent {} correlation {}",
                handoff.create.source_artifact,
                handoff.create.correlation_key,
                handoff.refresh.source_artifact,
                handoff.refresh.correlation_key
            ),
            "metadata kind verified: implementation_pr".to_string(),
        ]);
    }
    if let Some(history) = evidence.terminal_history.as_ref() {
        lines.push(format!(
            "terminal history: actionable_issue=#{} actionable_pr=#{} first_irrelevant_pr=#{} target_closed_issues={} target_closed_prs={} sibling={} sibling_closed_issues={} webhook={} older={} recovered={} cold_authority_rebuilt={}",
            history.actionable_issue_number,
            history.actionable_pull_request_number,
            history.first_history_pull_request_number,
            history.target_closed_issues,
            history.target_closed_pull_requests,
            history.sibling_repo_slug,
            history.sibling_closed_issues,
            history.webhook_delivery,
            history.actionable_older_than_history,
            history.actionable_recovered,
            history.cold_authority_rebuilt,
        ));
    }
    lines.push(format!(
        "CI jobs: {} completed job(s)",
        evidence.final_state.ci_jobs.len()
    ));
    for job in &evidence.final_state.ci_jobs {
        lines.push(format!(
            "CI job: id={} run={:?} attempt={:?} commit={} name={} status={} conclusion={:?} provider_conclusion={:?} url={:?}",
            job.job_id,
            job.provider_run_id,
            job.provider_attempt,
            job.commit_sha,
            job.name,
            job.status,
            job.conclusion,
            job.provider_conclusion,
            job.url
        ));
        if let Some(proof) = &job.verified_failure {
            lines.push(format!(
                "verified CI failure proof: category={} repository={} pull_request={:?} commit={} run={} job={} attempt={} task={:?} producer={} issuer={} verification={}",
                proof.category,
                proof.repository_id,
                proof.pull_request_id,
                proof.commit_sha,
                proof.run_id,
                proof.job_id,
                proof.attempt,
                proof.task_id,
                proof.producer_id,
                proof.issuer_id,
                proof.verification
            ));
        }
    }
    for head in &evidence.final_state.ci_heads {
        lines.push(format!(
            "CI head: phase={} sha={} observed_after_ms={} observations={}",
            head.phase,
            head.head_sha,
            head.observed_after_ms,
            head.observations.len()
        ));
        for job in &head.jobs {
            lines.push(format!(
                "CI head job: phase={} id={} run={:?} attempt={:?} commit={} name={} status={} conclusion={:?} provider_conclusion={:?}",
                head.phase,
                job.job_id,
                job.provider_run_id,
                job.provider_attempt,
                job.commit_sha,
                job.name,
                job.status,
                job.conclusion,
                job.provider_conclusion
            ));
            if let Some(proof) = &job.verified_failure {
                lines.push(format!(
                    "CI head verified failure: phase={} category={} run={} job={} attempt={} task={:?} producer={} issuer={} verification={}",
                    head.phase,
                    proof.category,
                    proof.run_id,
                    proof.job_id,
                    proof.attempt,
                    proof.task_id,
                    proof.producer_id,
                    proof.issuer_id,
                    proof.verification
                ));
            }
        }
    }
    if let Some(service) = &evidence.ci_failure_evidence {
        lines.push(format!(
            "CI failure evidence service: path={} issuer={} protected_producers={:?} published_proofs={}",
            service.endpoint_path,
            service.issuer,
            service.protected_producers,
            service.published_proofs
        ));
    }
    for stimulus in &evidence.stimuli {
        lines.push(format!(
            "stimulus: id={} action={} status={} attempts={} timeout={:?} duration={:?} details={:?}",
            stimulus.id,
            stimulus.action,
            stimulus.status.as_str(),
            stimulus.attempts,
            stimulus.timeout,
            stimulus.duration,
            stimulus.details
        ));
    }
    if let Some(retained_logs) = retained_logs {
        for path in &retained_logs.stimulus_logs {
            lines.push(format!("log pre-restart standalone: {}", path.display()));
        }
    }
    lines.extend([
        format!(
            "observability: TEMPER_LOG_FORMAT={} RUST_LOG={} scenario_run_id={}",
            evidence.temper_log_format, evidence.rust_log, evidence.scenario_run_id
        ),
        format!("structured event log: {}", standalone_log.display()),
        format!("log/artifact workspace: {}", workspace_root.display()),
        format!("log init: {}", init_log.display()),
        format!("log repo_populate: {}", repo_populate_log.display()),
        format!("log standalone: {}", standalone_log.display()),
        format!("log fake_llm: {}", fake_llm_log.display()),
        format!("artifact CI diagnostics: {}", ci_diagnostics_log.display()),
    ]);
    lines
}

fn resolve_temper_command(explicit: Option<&Path>) -> Result<TemperCommand, String> {
    let binary = if let Some(path) = explicit {
        validate_temper_binary(path, "--temper-bin")?
    } else if let Some(raw) = env::var_os(PRIMARY_TEMPER_BIN_ENV) {
        validate_env_temper_binary(PRIMARY_TEMPER_BIN_ENV, raw)?
    } else if let Some(raw) = env::var_os(COMPAT_TEMPER_BIN_ENV) {
        validate_env_temper_binary(COMPAT_TEMPER_BIN_ENV, raw)?
    } else {
        find_fallback_temper_binary()?
    };
    Ok(TemperCommand::new(binary))
}

fn validate_env_temper_binary(name: &str, raw: std::ffi::OsString) -> Result<PathBuf, String> {
    if raw.as_os_str().is_empty() {
        return Err(format!(
            "{name} is set but empty; pass --temper-bin <PATH> or unset {name}"
        ));
    }
    validate_temper_binary(Path::new(&raw), name)
}

fn validate_temper_binary(path: &Path, source: &str) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!(
            "live manifest runner {source} path does not exist: {}",
            path.display()
        ));
    }
    if !path.is_file() {
        return Err(format!(
            "live manifest runner {source} path is not a file: {}",
            path.display()
        ));
    }
    fs::canonicalize(path).map_err(|error| {
        format!(
            "canonicalize live manifest runner {source} path {}: {error}",
            path.display()
        )
    })
}

fn find_fallback_temper_binary() -> Result<PathBuf, String> {
    let candidates = fallback_candidates();
    for candidate in &candidates {
        if candidate.is_file() {
            return fs::canonicalize(candidate).map_err(|error| {
                format!(
                    "canonicalize fallback temper binary {}: {error}",
                    candidate.display()
                )
            });
        }
    }
    let checked = candidates
        .iter()
        .map(|path| format!("  - {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "could not resolve a standalone `temper` binary for the live manifest runner; pass --temper-bin <PATH>, set {PRIMARY_TEMPER_BIN_ENV}, or run `cargo dev-scenario-run`. Checked fallback candidates:\n{checked}"
    ))
}

fn fallback_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(current_exe) = env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            push_temper_in_dir(&mut candidates, dir);
            if dir
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("deps"))
            {
                if let Some(parent) = dir.parent() {
                    push_temper_in_dir(&mut candidates, parent);
                }
            }
        }
    }
    if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        push_temper_in_dir(&mut candidates, &target_dir.join("debug"));
        push_temper_in_dir(&mut candidates, &target_dir.join("release"));
    }
    if let Ok(current_dir) = env::current_dir() {
        push_temper_in_dir(&mut candidates, &current_dir.join("target/debug"));
        push_temper_in_dir(&mut candidates, &current_dir.join("target/release"));
        if let Some(root) = find_repo_root(&current_dir) {
            push_temper_in_dir(&mut candidates, &root.join("target/debug"));
            push_temper_in_dir(&mut candidates, &root.join("target/release"));
        }
    }
    candidates
}

fn push_temper_in_dir(candidates: &mut Vec<PathBuf>, dir: &Path) {
    push_unique(
        candidates,
        dir.join(format!("temper{}", env::consts::EXE_SUFFIX)),
    );
}

fn push_unique(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    loop {
        if current.join("Cargo.toml").is_file() && current.join("scenarios").is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}
