//! Runtime-bound transition-completion audit ordering and recovery coverage.

mod support;

use support::crash::{CrashForge, Fault, ForgeOp};
use support::{TestRoot, block_on, create_issue, new_repo};
use temper_forge::{
    BranchRef, CreatePullRequest, CreateRepository, Forge, Issue, IssueQuery, PullRequestQuery,
    RepositoryId, UserId,
};
use temper_workflow::{
    ArtifactRef, ArtifactSource, CreateIssuesChild, ExecutionContext, RawWorkflowSpec, RoleId,
    TransitionCompletionAudit, TransitionId, WorkflowMetadata, parse_metadata_block,
};

const WORKFLOW: &str = r#"{
  "name":"completion-audits",
  "roles":[{"id":"engineer"},{"id":"tester"}],
  "labels":[
    {"id":"code"},{"id":"ready"},{"id":"completed"},{"id":"intake"},
    {"id":"planned"},{"id":"blocked"}
  ],
  "artifact_kinds":[
    {"id":"code","target":"issue","identifying_labels":["code"]},
    {"id":"plan","target":"issue","identifying_labels":["intake"]}
  ],
  "state_dimensions":[{"id":"code_lifecycle","exclusive":true,"states":[
    {"id":"ready","label":"ready","artifacts":["code"]},
    {"id":"completed","label":"completed","artifacts":["code"]}
  ]}],
  "transitions":[
    {"id":"open_pr","artifact":"code","roles":["engineer"],"effects":[
      {"kind":"create_pull_request","correlation_key":"implementation-job-7"},
      {"kind":"remove_label","label":"ready"},
      {"kind":"add_label","label":"completed"}
    ]},
    {"id":"needs_followup","artifact":"plan","roles":["tester"],"effects":[
      {"kind":"create_issues","correlation_key":"validation-job-9","record_parent_dependencies":true},
      {"kind":"add_label","label":"planned"}
    ]}
  ]
}"#;

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn audit(marker: &str, outcome: &str) -> TransitionCompletionAudit {
    TransitionCompletionAudit::new(
        marker,
        format!("## Plan validation\n\nOutcome: `{outcome}`\n\nSafe summary."),
    )
}

fn pr_input(repo: &RepositoryId) -> CreatePullRequest {
    CreatePullRequest {
        title: "Implementation".into(),
        body: "Implementation body".into(),
        source: BranchRef {
            repository_id: repo.clone(),
            branch: "feature/audit".into(),
        },
        target: BranchRef {
            repository_id: repo.clone(),
            branch: "main".into(),
        },
        labels: Vec::new(),
        assignees: Vec::<UserId>::new(),
    }
}

fn issue(forge: &impl Forge, repo: &RepositoryId, number: temper_forge::ItemNumber) -> Issue {
    block_on(forge.get_issue_by_number(repo, number))
        .expect("issue lookup succeeds")
        .expect("issue exists")
}

fn metadata(issue: &Issue) -> WorkflowMetadata {
    parse_metadata_block(&issue.body)
        .expect("workflow metadata parses")
        .expect("workflow metadata exists")
}

fn comments(forge: &impl Forge, issue: &Issue) -> Vec<String> {
    block_on(forge.list_issue_comments(&issue.id))
        .expect("comment lookup succeeds")
        .into_iter()
        .map(|comment| comment.body)
        .collect()
}

#[test]
fn pull_request_is_ensured_before_audit_and_source_completion() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let source = create_issue(&forge, &repo, &["code", "ready"], "source");
    let transition = TransitionId::new("open_pr");
    let marker = "<!-- temper:comment-key=plan-validation:job-7 -->";
    let context = ExecutionContext::new()
        .with_pull_request_create(transition.clone(), pr_input(&repo))
        .with_transition_completion_audit(audit(marker, "validated"));
    let crashing = CrashForge::new(
        forge.clone(),
        vec![Fault::after(ForgeOp::AddIssueComment, 1)],
    );
    let executor = workflow.executor_with_context(&crashing, context);

    block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: source },
        &transition,
        &RoleId::new("engineer"),
    ))
    .expect_err("uncertain comment response interrupts source completion");

    assert_eq!(
        block_on(forge.list_pull_requests(&repo, PullRequestQuery::default()))
            .expect("pull request inventory")
            .len(),
        1,
        "the landing pull request exists before audit publication"
    );
    let interrupted = issue(&forge, &repo, source);
    assert_eq!(interrupted.labels, vec!["code", "ready"]);
    assert_eq!(comments(&forge, &interrupted).len(), 1);

    block_on(executor.execute(
        &repo,
        ArtifactSource::Issue { number: source },
        &transition,
        &RoleId::new("engineer"),
    ))
    .expect("exact replay reuses the PR and uncertain comment");

    assert_eq!(
        block_on(forge.list_pull_requests(&repo, PullRequestQuery::default()))
            .expect("pull request inventory")
            .len(),
        1
    );
    let completed = issue(&forge, &repo, source);
    assert_eq!(completed.labels, vec!["code", "completed"]);
    let audit_comments = comments(&forge, &completed);
    assert_eq!(audit_comments.len(), 1);
    assert!(audit_comments[0].contains(marker));
}

#[test]
fn fresh_executor_publishes_a_persisted_audit_without_a_worker_result() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let parent = create_issue(&forge, &repo, &["intake"], "plan");
    let transition = TransitionId::new("needs_followup");
    let marker = "<!-- temper:comment-key=plan-validation:job-recovery -->";
    let context = ExecutionContext::new()
        .with_create_issues_at(
            transition.clone(),
            0,
            [
                CreateIssuesChild::new("followup", "Recovered follow-up", "safe body")
                    .with_labels(["code", "ready"]),
            ],
        )
        .with_transition_completion_audit(audit(marker, "needs_followup"));
    let crashing = CrashForge::new(
        forge.clone(),
        vec![Fault::before(ForgeOp::AddIssueComment, 1)],
    );

    block_on(workflow.executor_with_context(&crashing, context).execute(
        &repo,
        ArtifactSource::Issue { number: parent },
        &transition,
        &RoleId::new("tester"),
    ))
    .expect_err("failed publication leaves the durable intent incomplete");
    let interrupted = issue(&forge, &repo, parent);
    assert!(comments(&forge, &interrupted).is_empty());
    assert_eq!(interrupted.labels, vec!["intake"]);

    assert_eq!(
        block_on(
            workflow
                .executor(&forge)
                .recover_create_issue_intents(&repo)
        )
        .expect("fresh-executor recovery publishes the audit"),
        1
    );
    let completed = issue(&forge, &repo, parent);
    let child = block_on(forge.list_issues(&repo, IssueQuery::default()))
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.number != parent)
        .expect("recovered child exists");
    let recovered_comments = comments(&forge, &completed);
    assert_eq!(recovered_comments.len(), 1);
    assert!(recovered_comments[0].contains(marker));
    assert!(
        recovered_comments[0].contains(&format!("#{}", child.number.get())),
        "recovery renders the final checkpointed child number"
    );
    assert_eq!(completed.labels, vec!["intake", "planned"]);
}

#[test]
fn uncertain_cross_repo_audit_recovers_from_persisted_final_child_references() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let parent_repo = new_repo(&forge);
    let target_repo = block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "followups".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("target repository exists")
    .id;
    let parent = create_issue(&forge, &parent_repo, &["intake"], "plan");
    let transition = TransitionId::new("needs_followup");
    let marker = "<!-- temper:comment-key=plan-validation:job-9 -->";
    let children = vec![
        CreateIssuesChild::new("api", "Repair API", "api body").with_labels(["code", "ready"]),
        CreateIssuesChild::new("client", "Repair client", "client body")
            .with_labels(["code", "ready"])
            .with_target_repo(target_repo.clone())
            .with_dependencies(["api"]),
    ];
    let context = ExecutionContext::new()
        .with_create_issues_at(transition.clone(), 0, children)
        .with_transition_completion_audit(audit(marker, "needs_followup"));
    let crashing = CrashForge::new(
        forge.clone(),
        vec![Fault::after(ForgeOp::AddIssueComment, 1)],
    );

    block_on(workflow.executor_with_context(&crashing, context).execute(
        &parent_repo,
        ArtifactSource::Issue { number: parent },
        &transition,
        &RoleId::new("tester"),
    ))
    .expect_err("uncertain audit create keeps fan-out completion retryable");

    let interrupted = issue(&forge, &parent_repo, parent);
    assert_eq!(interrupted.labels, vec!["intake"]);
    let interrupted_metadata = metadata(&interrupted);
    let intent = interrupted_metadata
        .create_issue_intents
        .values()
        .next()
        .expect("fan-out intent was persisted before child creation");
    assert!(!intent.completed);
    assert_eq!(
        intent.completion.as_ref().unwrap().completion_audit,
        Some(audit(marker, "needs_followup"))
    );
    assert!(
        !interrupted.body.contains(marker),
        "the nested HTML marker is safely encoded in workflow metadata"
    );

    let parent_issues =
        block_on(forge.list_issues(&parent_repo, IssueQuery::default())).expect("parent inventory");
    let target_issues =
        block_on(forge.list_issues(&target_repo, IssueQuery::default())).expect("target inventory");
    assert_eq!(parent_issues.len(), 2);
    assert_eq!(target_issues.len(), 1);
    let api = parent_issues
        .iter()
        .find(|candidate| candidate.number != parent)
        .expect("same-repository child exists");
    let client = &target_issues[0];
    assert!(!metadata(api).staged);
    assert!(!metadata(client).staged);
    assert_eq!(
        metadata(client).dependencies,
        vec![ArtifactRef::in_repo(parent_repo.clone(), api.number)]
    );
    let landed_comment = comments(&forge, &interrupted);
    assert_eq!(landed_comment.len(), 1);
    assert!(landed_comment[0].contains(&format!("#{}", api.number.get())));
    assert!(landed_comment[0].contains(&format!("acme/followups#{}", client.number.get())));

    // A fresh executor has no worker result or runtime context. The persisted
    // audit and child identities are sufficient to find the uncertain comment
    // and atomically finish the source transition.
    assert_eq!(
        block_on(
            workflow
                .executor(&forge)
                .recover_create_issue_intents(&parent_repo)
        )
        .expect("startup recovery converges"),
        1
    );

    let completed = issue(&forge, &parent_repo, parent);
    assert_eq!(completed.labels, vec!["intake", "planned"]);
    assert!(
        metadata(&completed)
            .create_issue_intents
            .values()
            .all(|intent| intent.completed)
    );
    let final_comments = comments(&forge, &completed);
    assert_eq!(
        final_comments.len(),
        1,
        "recovery does not duplicate the audit"
    );
    assert!(final_comments[0].contains("Repair API (`api`)"));
    assert!(final_comments[0].contains("Repair client (`client`)"));
    assert_eq!(
        block_on(forge.list_issues(&parent_repo, IssueQuery::default()))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        block_on(forge.list_issues(&target_repo, IssueQuery::default()))
            .unwrap()
            .len(),
        1
    );
}
